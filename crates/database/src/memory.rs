//! Incident memory: short titled entries that carry what past incidents
//! taught to the next investigation.
//!
//! Spec: `.workhorse/specs/monitoring/incident-memory.md` (id `MEM`).
//!
//! An entry holds a title, a body and at most one [`Link`]; with none it
//! applies to the fleet as a whole. Every change is a new revision with its own
//! row in the entry's history, which keeps what each version held. Amending and
//! restoring state the revision they were made against and are refused as
//! [`Outcome::Stale`] when the entry is past it. Retiring keeps the entry in
//! full and leaves it out of finds unless asked for.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use commons_errors::{AppError, Result};
use diesel::{
	dsl::{count_star, sql},
	pg::Pg,
	prelude::*,
	sql_types::{Bool, Nullable, Text},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	issues::{Incident, IncidentStatusFilter, IncidentTarget},
	notes_log::Author,
	schema::{
		check_policies, incident_issues, incident_notes, issues, memory_entries,
		memory_entry_revisions,
	},
	server_groups::ServerGroup,
	slack_outbox::SlackOutbox,
};

/// The longest title, in characters.
pub const TITLE_LIMIT: usize = 512;
/// The longest body, in characters.
pub const BODY_LIMIT: usize = 16_384;
/// The longest reason for a retirement, in characters.
pub const REASON_LIMIT: usize = 500;
/// How many checks and groups the index lists as having had the most
/// incidents, and how many incidents with notes.
pub const INDEX_LIST_LIMIT: usize = 20;
/// The look-back window of the index when none is asked for.
pub const DEFAULT_WINDOW_DAYS: u32 = 90;
/// How many incidents of the window the index reads, the most recent first.
const INDEX_INCIDENT_LIMIT: i64 = 10_000;

/// What an entry is about. An entry with none applies to the fleet as a whole.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Link {
	Group {
		id: Uuid,
	},
	Incident {
		id: Uuid,
	},
	/// A check by its source and presented name, wherever it is filed.
	Check {
		source: String,
		name: String,
	},
}

/// A kind of link, for narrowing a find. `Fleet` is an entry with no link.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum LinkKind {
	Fleet,
	Group,
	Check,
	Incident,
}

/// A link with what it names, as it is now.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ResolvedLink {
	#[serde(flatten)]
	pub link: Link,
	/// The group's name, the incident's target and when it opened, or the
	/// check's name.
	pub name: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = memory_entries)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MemoryEntry {
	pub id: Uuid,
	pub title: String,
	pub body: String,
	pub server_group_id: Option<Uuid>,
	pub incident_id: Option<Uuid>,
	pub check_source: Option<String>,
	pub check_name: Option<String>,
	pub revision: i64,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	pub created_by: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	pub updated_by: String,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub retired_at: Option<Timestamp>,
	pub retired_by: Option<String>,
	pub retired_reason: Option<String>,
}

impl MemoryEntry {
	/// What the entry is linked to, none for a fleet-wide entry.
	pub fn link(&self) -> Option<Link> {
		link_of(
			self.server_group_id,
			self.incident_id,
			self.check_source.as_deref(),
			self.check_name.as_deref(),
		)
	}

	pub fn is_retired(&self) -> bool {
		self.retired_at.is_some()
	}
}

fn link_of(
	group: Option<Uuid>,
	incident: Option<Uuid>,
	source: Option<&str>,
	name: Option<&str>,
) -> Option<Link> {
	if let Some(id) = group {
		Some(Link::Group { id })
	} else if let Some(id) = incident {
		Some(Link::Incident { id })
	} else {
		source.zip(name).map(|(source, name)| Link::Check {
			source: source.to_owned(),
			name: name.to_owned(),
		})
	}
}

/// What kind of change a version of an entry was.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RevisionKind {
	Recorded,
	Amended,
	Retired,
	Restored,
}

impl RevisionKind {
	fn as_str(self) -> &'static str {
		match self {
			Self::Recorded => "recorded",
			Self::Amended => "amended",
			Self::Retired => "retired",
			Self::Restored => "restored",
		}
	}

	fn parse(kind: &str) -> Self {
		match kind {
			"amended" => Self::Amended,
			"retired" => Self::Retired,
			"restored" => Self::Restored,
			_ => Self::Recorded,
		}
	}
}

/// One version of an entry: what it held after a change, and who made it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MemoryRevision {
	/// The entry's revision after this change.
	pub revision: i64,
	/// What kind of change it was.
	pub kind: RevisionKind,
	/// The title the entry held after this change.
	pub title: String,
	/// The body the entry held after this change.
	pub body: String,
	/// What the entry was linked to after this change.
	pub link: Option<Link>,
	/// Who made the change: an operator's login or a token's name.
	pub actor: String,
	/// How it was made: `operator` or `token`.
	pub via: String,
	/// Why, for a retirement.
	pub reason: Option<String>,
	/// For a restoration, the version it restored.
	pub restored_from: Option<i64>,
	/// When the change was made.
	pub created_at: Timestamp,
}

type RevisionRow = (
	i64,
	String,
	String,
	String,
	Option<Uuid>,
	Option<Uuid>,
	Option<String>,
	Option<String>,
	String,
	String,
	Option<String>,
	Option<i64>,
	jiff_diesel::Timestamp,
);

impl From<RevisionRow> for MemoryRevision {
	fn from(row: RevisionRow) -> Self {
		let (
			revision,
			kind,
			title,
			body,
			group,
			incident,
			source,
			name,
			actor,
			via,
			reason,
			restored_from,
			created_at,
		) = row;
		Self {
			revision,
			kind: RevisionKind::parse(&kind),
			title,
			body,
			link: link_of(group, incident, source.as_deref(), name.as_deref()),
			actor,
			via,
			reason,
			restored_from,
			created_at: created_at.into(),
		}
	}
}

/// What a write did.
#[derive(Clone, Debug)]
pub enum Outcome {
	Changed(MemoryEntry),
	/// Nothing the write asked for differed from what the entry held.
	Unchanged(MemoryEntry),
	/// The entry had changed since the revision the writer read; this is the
	/// entry as it stands.
	Stale(MemoryEntry),
}

impl Outcome {
	/// The entry, or the conflict as an error where the write was refused.
	pub fn or_conflict(self) -> Result<MemoryEntry> {
		match self {
			Self::Changed(entry) | Self::Unchanged(entry) => Ok(entry),
			Self::Stale(entry) => Err(AppError::Conflict(format!(
				"the entry changed since it was read; it is now at revision {}",
				entry.revision
			))),
		}
	}
}

/// A new entry.
#[derive(Clone, Debug, Deserialize, utoipa::ToSchema)]
pub struct NewEntry {
	/// What the entry is, in a line, at most 512 characters.
	pub title: String,
	/// What a later investigator would otherwise rediscover, at most 16,384 characters.
	pub body: String,
	/// What the entry is about; none applies it to the fleet as a whole.
	#[serde(default)]
	pub link: Option<Link>,
}

/// The changes an amendment makes. A field left out is left as it is.
#[derive(Clone, Debug, Default, Deserialize, utoipa::ToSchema)]
pub struct Amendment {
	pub title: Option<String>,
	pub body: Option<String>,
	/// A link to set, or `null` to clear it and make the entry fleet-wide.
	#[serde(default, deserialize_with = "present_option")]
	#[schema(value_type = Option<Link>)]
	pub link: Option<Option<Link>>,
}

pub fn present_option<'de, D, T>(
	deserializer: D,
) -> std::result::Result<Option<Option<T>>, D::Error>
where
	D: serde::Deserializer<'de>,
	T: Deserialize<'de>,
{
	Option::<T>::deserialize(deserializer).map(Some)
}

fn check_title(title: &str) -> Result<String> {
	let title = title.trim();
	if title.is_empty() {
		return Err(AppError::BadRequest("an entry needs a title".into()));
	}
	let length = title.chars().count();
	if length > TITLE_LIMIT {
		return Err(AppError::BadRequest(format!(
			"the title is {length} characters; the limit is {TITLE_LIMIT}"
		)));
	}
	Ok(title.to_owned())
}

fn check_body(body: &str) -> Result<String> {
	let body = body.trim();
	let length = body.chars().count();
	if length > BODY_LIMIT {
		return Err(AppError::BadRequest(format!(
			"the body is {length} characters; the limit is {BODY_LIMIT}"
		)));
	}
	Ok(body.to_owned())
}

type Columns = (Option<Uuid>, Option<Uuid>, Option<String>, Option<String>);

fn columns(link: Option<&Link>) -> Columns {
	match link {
		None => (None, None, None, None),
		Some(Link::Group { id }) => (Some(*id), None, None, None),
		Some(Link::Incident { id }) => (None, Some(*id), None, None),
		Some(Link::Check { source, name }) => {
			(None, None, Some(source.clone()), Some(name.clone()))
		}
	}
}

async fn check_link(conn: &mut AsyncPgConnection, link: &Link) -> Result<()> {
	match link {
		Link::Group { id } => {
			ServerGroup::get_by_id(conn, *id).await.map(drop)?;
		}
		Link::Incident { id } => {
			Incident::get_by_id(conn, *id).await.map(drop)?;
		}
		Link::Check { source, name } => {
			let known: i64 = check_policies::table
				.filter(check_policies::source.eq(source))
				.filter(check_policies::check_name.eq(name))
				.select(count_star())
				.first(conn)
				.await
				.map_err(AppError::from)?;
			if known == 0 {
				return Err(AppError::BadRequest(format!(
					"no check {name:?} from source {source:?}"
				)));
			}
		}
	}
	Ok(())
}

async fn lock(conn: &mut AsyncPgConnection, id: Uuid) -> Result<MemoryEntry> {
	memory_entries::table
		.filter(memory_entries::id.eq(id))
		.select(MemoryEntry::as_select())
		.for_update()
		.first(conn)
		.await
		.map_err(AppError::from)
}

async fn write_revision(
	conn: &mut AsyncPgConnection,
	entry: &MemoryEntry,
	kind: RevisionKind,
	author: Author<'_>,
	reason: Option<&str>,
	restored_from: Option<i64>,
) -> Result<()> {
	diesel::insert_into(memory_entry_revisions::table)
		.values((
			memory_entry_revisions::entry_id.eq(entry.id),
			memory_entry_revisions::revision.eq(entry.revision),
			memory_entry_revisions::kind.eq(kind.as_str()),
			memory_entry_revisions::title.eq(&entry.title),
			memory_entry_revisions::body.eq(&entry.body),
			memory_entry_revisions::server_group_id.eq(entry.server_group_id),
			memory_entry_revisions::incident_id.eq(entry.incident_id),
			memory_entry_revisions::check_source.eq(&entry.check_source),
			memory_entry_revisions::check_name.eq(&entry.check_name),
			memory_entry_revisions::actor.eq(author.name),
			memory_entry_revisions::via.eq(author.via.as_str()),
			memory_entry_revisions::reason.eq(reason),
			memory_entry_revisions::restored_from.eq(restored_from),
		))
		.execute(conn)
		.await
		.map_err(AppError::from)?;
	Ok(())
}

/// Record a new entry.
pub async fn record(
	conn: &mut AsyncPgConnection,
	new: NewEntry,
	author: Author<'_>,
) -> Result<MemoryEntry> {
	let title = check_title(&new.title)?;
	let body = check_body(&new.body)?;
	let (author_name, via) = (author.name.to_owned(), author.via);
	conn.transaction::<_, AppError, _>(async move |conn| {
		if let Some(link) = &new.link {
			check_link(conn, link).await?;
		}
		let (group, incident, source, name) = columns(new.link.as_ref());
		let entry: MemoryEntry = diesel::insert_into(memory_entries::table)
			.values((
				memory_entries::title.eq(&title),
				memory_entries::body.eq(&body),
				memory_entries::server_group_id.eq(group),
				memory_entries::incident_id.eq(incident),
				memory_entries::check_source.eq(source),
				memory_entries::check_name.eq(name),
				memory_entries::created_by.eq(&author_name),
				memory_entries::updated_by.eq(&author_name),
			))
			.returning(MemoryEntry::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)?;
		let author = Author {
			name: &author_name,
			via,
		};
		write_revision(conn, &entry, RevisionKind::Recorded, author, None, None).await?;
		Ok(entry)
	})
	.await
}

/// Amend an entry's title, body or link, against `expected_revision`.
pub async fn amend(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	expected_revision: i64,
	amendment: Amendment,
	author: Author<'_>,
) -> Result<Outcome> {
	let title = amendment.title.as_deref().map(check_title).transpose()?;
	let body = amendment.body.as_deref().map(check_body).transpose()?;
	let (author_name, via) = (author.name.to_owned(), author.via);
	conn.transaction::<_, AppError, _>(async move |conn| {
		let held = lock(conn, id).await?;
		if held.revision != expected_revision {
			return Ok(Outcome::Stale(held));
		}
		if held.is_retired() {
			return Err(AppError::BadRequest(
				"the entry is retired; restore it to change it".into(),
			));
		}
		let new_title = title.unwrap_or_else(|| held.title.clone());
		let new_body = body.unwrap_or_else(|| held.body.clone());
		let new_link = match amendment.link {
			Some(link) => link,
			None => held.link(),
		};
		if new_title == held.title && new_body == held.body && new_link == held.link() {
			return Ok(Outcome::Unchanged(held));
		}
		if let Some(link) = &new_link
			&& Some(link) != held.link().as_ref()
		{
			check_link(conn, link).await?;
		}
		let (group, incident, source, name) = columns(new_link.as_ref());
		let entry: MemoryEntry = diesel::update(memory_entries::table.find(id))
			.set((
				memory_entries::title.eq(&new_title),
				memory_entries::body.eq(&new_body),
				memory_entries::server_group_id.eq(group),
				memory_entries::incident_id.eq(incident),
				memory_entries::check_source.eq(source),
				memory_entries::check_name.eq(name),
				memory_entries::revision.eq(held.revision + 1),
				memory_entries::updated_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
				memory_entries::updated_by.eq(&author_name),
			))
			.returning(MemoryEntry::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)?;
		let author = Author {
			name: &author_name,
			via,
		};
		write_revision(conn, &entry, RevisionKind::Amended, author, None, None).await?;
		Ok(Outcome::Changed(entry))
	})
	.await
}

/// Retire an entry, recording who and why.
pub async fn retire(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	reason: &str,
	author: Author<'_>,
) -> Result<MemoryEntry> {
	let reason = reason.trim().to_owned();
	if reason.is_empty() {
		return Err(AppError::BadRequest("a retirement needs a reason".into()));
	}
	let length = reason.chars().count();
	if length > REASON_LIMIT {
		return Err(AppError::BadRequest(format!(
			"the reason is {length} characters; the limit is {REASON_LIMIT}"
		)));
	}
	let (author_name, via) = (author.name.to_owned(), author.via);
	conn.transaction::<_, AppError, _>(async move |conn| {
		let held = lock(conn, id).await?;
		if held.is_retired() {
			return Err(AppError::Conflict("the entry is already retired".into()));
		}
		let now = jiff_diesel::Timestamp::from(Timestamp::now());
		let entry: MemoryEntry = diesel::update(memory_entries::table.find(id))
			.set((
				memory_entries::revision.eq(held.revision + 1),
				memory_entries::updated_at.eq(now),
				memory_entries::updated_by.eq(&author_name),
				memory_entries::retired_at.eq(Some(now)),
				memory_entries::retired_by.eq(Some(&author_name)),
				memory_entries::retired_reason.eq(Some(&reason)),
			))
			.returning(MemoryEntry::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)?;
		let author = Author {
			name: &author_name,
			via,
		};
		write_revision(
			conn,
			&entry,
			RevisionKind::Retired,
			author,
			Some(&reason),
			None,
		)
		.await?;
		Ok(entry)
	})
	.await
}

/// Restore an entry to the version `to_revision` of itself, against
/// `expected_revision`. The restoration is a new version, and makes a retired
/// entry current again.
pub async fn restore(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	expected_revision: i64,
	to_revision: i64,
	author: Author<'_>,
) -> Result<Outcome> {
	let (author_name, via) = (author.name.to_owned(), author.via);
	conn.transaction::<_, AppError, _>(async move |conn| {
		let held = lock(conn, id).await?;
		if held.revision != expected_revision {
			return Ok(Outcome::Stale(held));
		}
		let version: Option<MemoryRevision> = memory_entry_revisions::table
			.filter(memory_entry_revisions::entry_id.eq(id))
			.filter(memory_entry_revisions::revision.eq(to_revision))
			.select(revision_columns())
			.first::<RevisionRow>(conn)
			.await
			.optional()
			.map_err(AppError::from)?
			.map(MemoryRevision::from);
		let version = version.ok_or_else(|| {
			AppError::NotFound(format!("the entry has no revision {to_revision}"))
		})?;
		if version.title == held.title
			&& version.body == held.body
			&& version.link == held.link()
			&& !held.is_retired()
		{
			return Ok(Outcome::Unchanged(held));
		}
		if let Some(link) = &version.link
			&& Some(link) != held.link().as_ref()
		{
			check_link(conn, link).await?;
		}
		let (group, incident, source, name) = columns(version.link.as_ref());
		let entry: MemoryEntry = diesel::update(memory_entries::table.find(id))
			.set((
				memory_entries::title.eq(&version.title),
				memory_entries::body.eq(&version.body),
				memory_entries::server_group_id.eq(group),
				memory_entries::incident_id.eq(incident),
				memory_entries::check_source.eq(source),
				memory_entries::check_name.eq(name),
				memory_entries::revision.eq(held.revision + 1),
				memory_entries::updated_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
				memory_entries::updated_by.eq(&author_name),
				memory_entries::retired_at.eq(None::<jiff_diesel::Timestamp>),
				memory_entries::retired_by.eq(None::<String>),
				memory_entries::retired_reason.eq(None::<String>),
			))
			.returning(MemoryEntry::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)?;
		let author = Author {
			name: &author_name,
			via,
		};
		write_revision(
			conn,
			&entry,
			RevisionKind::Restored,
			author,
			None,
			Some(to_revision),
		)
		.await?;
		Ok(Outcome::Changed(entry))
	})
	.await
}

fn revision_columns() -> (
	memory_entry_revisions::revision,
	memory_entry_revisions::kind,
	memory_entry_revisions::title,
	memory_entry_revisions::body,
	memory_entry_revisions::server_group_id,
	memory_entry_revisions::incident_id,
	memory_entry_revisions::check_source,
	memory_entry_revisions::check_name,
	memory_entry_revisions::actor,
	memory_entry_revisions::via,
	memory_entry_revisions::reason,
	memory_entry_revisions::restored_from,
	memory_entry_revisions::created_at,
) {
	(
		memory_entry_revisions::revision,
		memory_entry_revisions::kind,
		memory_entry_revisions::title,
		memory_entry_revisions::body,
		memory_entry_revisions::server_group_id,
		memory_entry_revisions::incident_id,
		memory_entry_revisions::check_source,
		memory_entry_revisions::check_name,
		memory_entry_revisions::actor,
		memory_entry_revisions::via,
		memory_entry_revisions::reason,
		memory_entry_revisions::restored_from,
		memory_entry_revisions::created_at,
	)
}

/// An entry, retired or not.
pub async fn get(conn: &mut AsyncPgConnection, id: Uuid) -> Result<MemoryEntry> {
	memory_entries::table
		.find(id)
		.select(MemoryEntry::as_select())
		.first(conn)
		.await
		.map_err(AppError::from)
}

/// An entry's history, newest first.
pub async fn history(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Vec<MemoryRevision>> {
	let rows: Vec<RevisionRow> = memory_entry_revisions::table
		.filter(memory_entry_revisions::entry_id.eq(id))
		.order(memory_entry_revisions::revision.desc())
		.select(revision_columns())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(rows.into_iter().map(MemoryRevision::from).collect())
}

/// What makes an entry relevant to an incident: it is linked to the incident,
/// to the group its target is, or to a check among its issues.
// spec: MEM#the-memory-page
#[derive(Clone, Debug)]
pub struct Relevance {
	incident_id: Uuid,
	group_id: Option<Uuid>,
	checks: Vec<(String, String)>,
}

/// What entries are relevant to an incident.
pub async fn relevance(conn: &mut AsyncPgConnection, incident_id: Uuid) -> Result<Relevance> {
	let incident = Incident::get_by_id(conn, incident_id).await?;
	let group_id = match IncidentTarget::of_incident(&incident) {
		IncidentTarget::Environment(group, _) => Some(group),
		IncidentTarget::Global => None,
	};
	let checks: Vec<(String, String)> = incident_issues::table
		.inner_join(issues::table)
		.filter(incident_issues::incident_id.eq(incident_id))
		.filter(issues::check_name.is_not_null())
		.select((issues::source, issues::check_name.assume_not_null()))
		.distinct()
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(Relevance {
		incident_id,
		group_id,
		checks,
	})
}

/// What to find.
#[derive(Clone, Debug, Default)]
pub struct Find {
	/// Free text matched against titles and bodies, each word matching words it
	/// begins.
	pub text: Option<String>,
	pub link_kind: Option<LinkKind>,
	pub group: Option<Uuid>,
	pub check: Option<(String, String)>,
	pub incident: Option<Uuid>,
	pub relevant_to_incident: Option<Uuid>,
	pub include_retired: bool,
	pub limit: i64,
}

/// The entries found, and how many matched in all.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Found {
	pub entries: Vec<MemoryEntry>,
	pub total: i64,
}

type Predicate = Box<dyn BoxableExpression<memory_entries::table, Pg, SqlType = Nullable<Bool>>>;

fn text_query(text: &str) -> Option<String> {
	let words: Vec<String> = text
		.split(|c: char| !c.is_alphanumeric())
		.filter(|word| !word.is_empty())
		.map(|word| format!("{word}:*"))
		.collect();
	(!words.is_empty()).then(|| words.join(" & "))
}

fn relevant_predicate(relevance: &Relevance) -> Predicate {
	let mut predicate: Predicate = Box::new(
		memory_entries::incident_id
			.eq(relevance.incident_id)
			.nullable(),
	);
	if let Some(group) = relevance.group_id {
		predicate = Box::new(predicate.or(memory_entries::server_group_id.eq(group).nullable()));
	}
	for (source, name) in &relevance.checks {
		predicate = Box::new(
			predicate.or(memory_entries::check_source
				.eq(source.clone())
				.and(memory_entries::check_name.eq(name.clone()))
				.nullable()),
		);
	}
	predicate
}

fn find_predicate(find: &Find, relevance: Option<&Relevance>) -> Predicate {
	let mut predicate: Predicate = Box::new(sql::<Nullable<Bool>>("TRUE"));
	if !find.include_retired {
		predicate = Box::new(predicate.and(memory_entries::retired_at.is_null().nullable()));
	}
	if let Some(query) = find.text.as_deref().and_then(text_query) {
		predicate = Box::new(
			predicate.and(
				sql::<Bool>("memory_entries.search @@ to_tsquery('english', ")
					.bind::<Text, _>(query)
					.sql(")")
					.nullable(),
			),
		);
	}
	match find.link_kind {
		Some(LinkKind::Fleet) => {
			predicate = Box::new(
				predicate.and(
					memory_entries::server_group_id
						.is_null()
						.and(memory_entries::incident_id.is_null())
						.and(memory_entries::check_source.is_null())
						.nullable(),
				),
			);
		}
		Some(LinkKind::Group) => {
			predicate =
				Box::new(predicate.and(memory_entries::server_group_id.is_not_null().nullable()));
		}
		Some(LinkKind::Incident) => {
			predicate =
				Box::new(predicate.and(memory_entries::incident_id.is_not_null().nullable()));
		}
		Some(LinkKind::Check) => {
			predicate =
				Box::new(predicate.and(memory_entries::check_source.is_not_null().nullable()));
		}
		None => {}
	}
	if let Some(group) = find.group {
		predicate = Box::new(predicate.and(memory_entries::server_group_id.eq(group).nullable()));
	}
	if let Some(incident) = find.incident {
		predicate = Box::new(predicate.and(memory_entries::incident_id.eq(incident).nullable()));
	}
	if let Some((source, name)) = &find.check {
		predicate = Box::new(
			predicate.and(
				memory_entries::check_source
					.eq(source.clone())
					.and(memory_entries::check_name.eq(name.clone()))
					.nullable(),
			),
		);
	}
	if let Some(relevance) = relevance {
		predicate = Box::new(predicate.and(relevant_predicate(relevance)));
	}
	predicate
}

/// Find entries, newest amended first.
pub async fn find(conn: &mut AsyncPgConnection, find: &Find) -> Result<Found> {
	let relevance = match find.relevant_to_incident {
		Some(incident) => Some(self::relevance(conn, incident).await?),
		None => None,
	};
	let total = memory_entries::table
		.filter(find_predicate(find, relevance.as_ref()))
		.select(count_star())
		.first(conn)
		.await
		.map_err(AppError::from)?;
	let entries = memory_entries::table
		.filter(find_predicate(find, relevance.as_ref()))
		.order((memory_entries::updated_at.desc(), memory_entries::id))
		.limit(find.limit.max(1))
		.select(MemoryEntry::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(Found { entries, total })
}

/// How many entries are relevant to an incident, leaving retired ones out.
pub async fn relevant_count_for_incident(
	conn: &mut AsyncPgConnection,
	incident_id: Uuid,
) -> Result<i64> {
	let relevance = relevance(conn, incident_id).await?;
	let find = Find {
		relevant_to_incident: Some(incident_id),
		..Find::default()
	};
	memory_entries::table
		.filter(find_predicate(&find, Some(&relevance)))
		.select(count_star())
		.first(conn)
		.await
		.map_err(AppError::from)
}

/// The names of what entries link to, as they are now.
pub async fn resolve_links(
	conn: &mut AsyncPgConnection,
	links: impl IntoIterator<Item = Link>,
) -> Result<HashMap<Link, String>> {
	let links: HashSet<Link> = links.into_iter().collect();
	let group_ids: Vec<Uuid> = links
		.iter()
		.filter_map(|link| match link {
			Link::Group { id } => Some(*id),
			_ => None,
		})
		.collect();
	let incident_ids: Vec<Uuid> = links
		.iter()
		.filter_map(|link| match link {
			Link::Incident { id } => Some(*id),
			_ => None,
		})
		.collect();
	let mut names = HashMap::new();
	let groups = ServerGroup::names_by_ids(conn, &group_ids).await?;
	for id in &group_ids {
		if let Some(name) = groups.get(id) {
			names.insert(Link::Group { id: *id }, name.clone());
		}
	}
	if !incident_ids.is_empty() {
		use crate::schema::incidents::dsl;
		let incidents: Vec<Incident> = dsl::incidents
			.filter(dsl::id.eq_any(&incident_ids))
			.select(Incident::as_select())
			.load(conn)
			.await
			.map_err(AppError::from)?;
		let target_groups: Vec<Uuid> = incidents.iter().filter_map(|i| i.server_group_id).collect();
		let target_names = ServerGroup::names_by_ids(conn, &target_groups).await?;
		for incident in incidents {
			let target = incident
				.server_group_id
				.and_then(|group| target_names.get(&group).cloned())
				.map(|group| match incident.rank {
					Some(rank) => format!("{group} {rank}"),
					None => group,
				})
				.unwrap_or_else(|| "Canopy".to_owned());
			names.insert(
				Link::Incident { id: incident.id },
				format!("{target}, opened {}", incident.opened_at),
			);
		}
	}
	for link in links {
		if let Link::Check { name, .. } = &link {
			names.insert(link.clone(), name.clone());
		}
	}
	Ok(names)
}

/// A memory entry, with what it links to resolved to a name.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct MemoryEntryData {
	/// The entry's identifier.
	pub id: Uuid,
	/// What the entry is, in a line.
	pub title: String,
	/// What a later investigator would otherwise rediscover.
	pub body: String,
	/// What the entry is about, none for the fleet as a whole.
	pub link: Option<ResolvedLink>,
	/// Changes whenever the entry does; writes state the revision they read.
	pub revision: i64,
	/// When the entry was recorded.
	pub created_at: Timestamp,
	/// Who recorded it: an operator's login or an access token's name.
	pub created_by: String,
	/// When the entry last changed.
	pub updated_at: Timestamp,
	/// Who last changed it.
	pub updated_by: String,
	/// When the entry was retired, if it is.
	pub retired_at: Option<Timestamp>,
	/// Who retired it.
	pub retired_by: Option<String>,
	/// Why it was retired.
	pub retired_reason: Option<String>,
}

/// Entries with what they link to resolved to names.
pub async fn present(
	conn: &mut AsyncPgConnection,
	entries: Vec<MemoryEntry>,
) -> Result<Vec<MemoryEntryData>> {
	let names = resolve_links(conn, entries.iter().filter_map(MemoryEntry::link)).await?;
	Ok(entries
		.into_iter()
		.map(|entry| MemoryEntryData {
			link: entry.link().map(|link| ResolvedLink {
				name: names.get(&link).cloned().unwrap_or_default(),
				link,
			}),
			id: entry.id,
			title: entry.title,
			body: entry.body,
			revision: entry.revision,
			created_at: entry.created_at,
			created_by: entry.created_by,
			updated_at: entry.updated_at,
			updated_by: entry.updated_by,
			retired_at: entry.retired_at,
			retired_by: entry.retired_by,
			retired_reason: entry.retired_reason,
		})
		.collect())
}

/// One entry with what it links to resolved to a name.
pub async fn present_one(
	conn: &mut AsyncPgConnection,
	entry: MemoryEntry,
) -> Result<MemoryEntryData> {
	Ok(present(conn, vec![entry]).await?.remove(0))
}

/// An entry in the index: enough to choose which to read in full.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CompactEntry {
	pub id: Uuid,
	pub title: String,
	pub link: Option<ResolvedLink>,
	pub updated_at: Timestamp,
}

/// A check that has been part of incidents in the window.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct CheckTrouble {
	pub source: String,
	pub name: String,
	pub incidents: i64,
	pub published: i64,
	pub latest: Timestamp,
	/// The groups the incidents were on.
	pub groups: Vec<String>,
	/// How many entries are linked to the check. None is a gap.
	pub linked_entries: i64,
}

/// A group that has had incidents in the window.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct GroupTrouble {
	pub group_id: Uuid,
	pub name: String,
	pub incidents: i64,
	pub published: i64,
	/// How many entries are linked to the group. None is a gap.
	pub linked_entries: i64,
}

/// An incident in the window that carries notes.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct IncidentWithNotes {
	pub incident_id: Uuid,
	pub group_id: Option<Uuid>,
	pub group_name: Option<String>,
	pub rank: Option<String>,
	pub status: String,
	pub notes: i64,
	pub latest_note_at: Timestamp,
}

/// What an agent starting an investigation can read first.
// spec: MCP#incident-memory
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct Index {
	pub window_days: u32,
	pub group_id: Option<Uuid>,
	pub entries: Vec<CompactEntry>,
	pub checks: Vec<CheckTrouble>,
	pub groups: Vec<GroupTrouble>,
	pub incidents_with_notes: Vec<IncidentWithNotes>,
}

fn status_of(incident: &Incident) -> &'static str {
	if incident.resolved_at.is_some() {
		"resolved"
	} else if incident.closed_at.is_some() {
		"closed"
	} else {
		"open"
	}
}

/// What the index reads of a current entry: its body is left behind, since the
/// index lists entries for choosing which to read in full.
#[derive(Queryable, Selectable)]
#[diesel(table_name = memory_entries)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct IndexRow {
	id: Uuid,
	title: String,
	server_group_id: Option<Uuid>,
	incident_id: Option<Uuid>,
	check_source: Option<String>,
	check_name: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	updated_at: Timestamp,
}

impl IndexRow {
	fn link(&self) -> Option<Link> {
		link_of(
			self.server_group_id,
			self.incident_id,
			self.check_source.as_deref(),
			self.check_name.as_deref(),
		)
	}
}

/// The index of memory and of recent trouble, optionally narrowed to one group.
// spec: MCP#incident-memory
pub async fn index(
	conn: &mut AsyncPgConnection,
	group: Option<Uuid>,
	window_days: u32,
) -> Result<Index> {
	let since =
		Timestamp::now() - SignedDuration::from_hours(24 * i64::from(window_days.min(3650)));
	let incidents = Incident::list_open_since(
		conn,
		since,
		group,
		IncidentStatusFilter::All,
		INDEX_INCIDENT_LIMIT,
	)
	.await?;
	let ids: Vec<Uuid> = incidents.iter().map(|i| i.id).collect();
	let published = SlackOutbox::delivered_open_ids(conn, &ids).await?;
	let by_id: HashMap<Uuid, &Incident> = incidents.iter().map(|i| (i.id, i)).collect();

	let group_ids: Vec<Uuid> = incidents.iter().filter_map(|i| i.server_group_id).collect();
	let group_names = ServerGroup::names_by_ids(conn, &group_ids).await?;

	// The checks each incident's issues were filed under.
	let members: Vec<(Uuid, String, String)> = if ids.is_empty() {
		Vec::new()
	} else {
		incident_issues::table
			.inner_join(issues::table)
			.filter(incident_issues::incident_id.eq_any(&ids))
			.filter(issues::check_name.is_not_null())
			.select((
				incident_issues::incident_id,
				issues::source,
				issues::check_name.assume_not_null(),
			))
			.distinct()
			.load(conn)
			.await
			.map_err(AppError::from)?
	};

	let mut check_incidents: BTreeMap<(String, String), BTreeSet<Uuid>> = BTreeMap::new();
	for (incident, source, name) in members {
		check_incidents
			.entry((source, name))
			.or_default()
			.insert(incident);
	}

	// What is known about each check and group, among current entries.
	let current: Vec<IndexRow> = memory_entries::table
		.filter(memory_entries::retired_at.is_null())
		.order((memory_entries::updated_at.desc(), memory_entries::id))
		.select(IndexRow::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let mut linked_to_check: HashMap<(String, String), i64> = HashMap::new();
	let mut linked_to_group: HashMap<Uuid, i64> = HashMap::new();
	for entry in &current {
		match entry.link() {
			Some(Link::Check { source, name }) => {
				*linked_to_check.entry((source, name)).or_default() += 1;
			}
			Some(Link::Group { id }) => *linked_to_group.entry(id).or_default() += 1,
			_ => {}
		}
	}

	let mut checks: Vec<CheckTrouble> = check_incidents
		.iter()
		.map(|((source, name), members)| {
			let rows: Vec<&Incident> = members
				.iter()
				.filter_map(|id| by_id.get(id).copied())
				.collect();
			let mut groups: Vec<String> = rows
				.iter()
				.filter_map(|i| i.server_group_id)
				.filter_map(|id| group_names.get(&id).cloned())
				.collect::<BTreeSet<_>>()
				.into_iter()
				.collect();
			groups.sort();
			CheckTrouble {
				source: source.clone(),
				name: name.clone(),
				incidents: rows.len() as i64,
				published: rows.iter().filter(|i| published.contains(&i.id)).count() as i64,
				latest: rows
					.iter()
					.map(|i| i.opened_at)
					.max()
					.unwrap_or_else(Timestamp::now),
				groups,
				linked_entries: linked_to_check
					.get(&(source.clone(), name.clone()))
					.copied()
					.unwrap_or(0),
			}
		})
		.collect();
	checks.sort_by(|a, b| {
		b.incidents
			.cmp(&a.incidents)
			.then(b.latest.cmp(&a.latest))
			.then(a.name.cmp(&b.name))
	});
	checks.truncate(INDEX_LIST_LIMIT);

	let mut per_group: BTreeMap<Uuid, (i64, i64)> = BTreeMap::new();
	for incident in &incidents {
		if let Some(id) = incident.server_group_id {
			let counts = per_group.entry(id).or_default();
			counts.0 += 1;
			if published.contains(&incident.id) {
				counts.1 += 1;
			}
		}
	}
	let mut groups: Vec<GroupTrouble> = per_group
		.into_iter()
		.map(|(id, (incidents, published))| GroupTrouble {
			group_id: id,
			name: group_names.get(&id).cloned().unwrap_or_default(),
			incidents,
			published,
			linked_entries: linked_to_group.get(&id).copied().unwrap_or(0),
		})
		.collect();
	groups.sort_by(|a, b| b.incidents.cmp(&a.incidents).then(a.name.cmp(&b.name)));
	groups.truncate(INDEX_LIST_LIMIT);

	let notes: Vec<(Uuid, i64, jiff_diesel::Timestamp)> = if ids.is_empty() {
		Vec::new()
	} else {
		incident_notes::table
			.filter(incident_notes::incident_id.eq_any(&ids))
			.filter(incident_notes::deleted_at.is_null())
			.group_by(incident_notes::incident_id)
			.select((
				incident_notes::incident_id,
				count_star(),
				diesel::dsl::max(incident_notes::created_at).assume_not_null(),
			))
			.load(conn)
			.await
			.map_err(AppError::from)?
	};
	let mut incidents_with_notes: Vec<IncidentWithNotes> = notes
		.into_iter()
		.filter_map(|(id, count, latest)| {
			let incident = by_id.get(&id)?;
			Some(IncidentWithNotes {
				incident_id: id,
				group_id: incident.server_group_id,
				group_name: incident
					.server_group_id
					.and_then(|group| group_names.get(&group).cloned()),
				rank: incident.rank.map(|rank| rank.to_string()),
				status: status_of(incident).to_owned(),
				notes: count,
				latest_note_at: latest.into(),
			})
		})
		.collect();
	incidents_with_notes.sort_by(|a, b| b.latest_note_at.cmp(&a.latest_note_at));
	incidents_with_notes.truncate(INDEX_LIST_LIMIT);

	// Entries, narrowed to a group's: fleet-wide, linked to the group, to one of
	// its incidents, or to a check it has had incidents on.
	let group_incidents: HashSet<Uuid> = match group {
		Some(group) => {
			use crate::schema::incidents::dsl;
			dsl::incidents
				.filter(dsl::server_group_id.eq(group))
				.select(dsl::id)
				.load::<Uuid>(conn)
				.await
				.map_err(AppError::from)?
				.into_iter()
				.collect()
		}
		None => HashSet::new(),
	};
	let group_checks: HashSet<&(String, String)> = check_incidents.keys().collect();
	let kept: Vec<&IndexRow> = current
		.iter()
		.filter(|entry| match (group, entry.link()) {
			(None, _) | (Some(_), None) => true,
			(Some(group), Some(Link::Group { id })) => id == group,
			(Some(_), Some(Link::Incident { id })) => group_incidents.contains(&id),
			(Some(_), Some(Link::Check { source, name })) => group_checks.contains(&(source, name)),
		})
		.collect();
	let names = resolve_links(conn, kept.iter().filter_map(|entry| entry.link())).await?;
	let entries = kept
		.into_iter()
		.map(|entry| CompactEntry {
			id: entry.id,
			title: entry.title.clone(),
			link: entry.link().map(|link| ResolvedLink {
				name: names.get(&link).cloned().unwrap_or_default(),
				link,
			}),
			updated_at: entry.updated_at,
		})
		.collect();

	Ok(Index {
		window_days,
		group_id: group,
		entries,
		checks,
		groups,
		incidents_with_notes,
	})
}
