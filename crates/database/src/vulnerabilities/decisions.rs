//! Triage decisions: recording them, changing them, and the lapse of one whose
//! advisory or exposure moved from under it.
//!
//! Spec: `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).

use std::collections::{HashMap, HashSet};

use commons_errors::{AppError, Result};
use diesel::{OptionalExtension, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
	Advisory, Outcome, Source, Status, Via, counted_by_source, items::EXPOSURE_CTE, strings,
};
use crate::{
	audit::{self, Action, Actor, RecordKind},
	osv::{Held, LapseReason},
};

// ── Entries ─────────────────────────────────────────────────────────────────

/// One outcome of a decision, for the workspace packages it names or, with none
/// named, for every other one: the catch-all.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityDecisionEntry)]
pub struct Entry {
	/// The workspace packages it covers. Absent for the catch-all, which also
	/// covers workspace packages that come to reach the advisory later.
	#[serde(default)]
	pub workspace_packages: Option<Vec<String>>,
	/// What it decides.
	pub outcome: Outcome,
	/// Why. Required for not affected and accepted risk.
	#[serde(default)]
	pub reason: Option<String>,
	/// For will fix, a link to the issue tracking the fix.
	#[serde(default)]
	pub issue_url: Option<String>,
}

impl Entry {
	pub fn is_catch_all(&self) -> bool {
		self.workspace_packages.is_none()
	}

	/// What the outcome requires: a reason for not affected and accepted risk,
	/// and for will fix an issue link that is a web address, since it is
	/// presented as a link.
	pub fn check(&self) -> Result<()> {
		let reason = self.reason.as_deref().map(str::trim).unwrap_or_default();
		let issue_url = self.issue_url.as_deref().map(str::trim).unwrap_or_default();
		match self.outcome {
			Outcome::WillFix if !issue_url.is_empty() => match url::Url::parse(issue_url) {
				Ok(url) if matches!(url.scheme(), "http" | "https") => Ok(()),
				_ => Err(AppError::BadRequest(
					"the issue link must be an http or https address".into(),
				)),
			},
			Outcome::NotAffected if reason.is_empty() => Err(AppError::BadRequest(
				"a not affected decision needs a reason".into(),
			)),
			Outcome::AcceptedRisk if reason.is_empty() => Err(AppError::BadRequest(
				"an accepted risk needs a reason".into(),
			)),
			_ => Ok(()),
		}
	}

	/// The entry as it is kept: text trimmed, blank text none, and an issue link
	/// only for will fix.
	fn normalised(mut self) -> Self {
		let trim = |text: Option<String>| {
			text.map(|text| text.trim().to_owned())
				.filter(|text| !text.is_empty())
		};
		self.reason = trim(self.reason);
		self.issue_url = trim(self.issue_url).filter(|_| self.outcome == Outcome::WillFix);
		self
	}
}

/// The status a decision's entries give: their shared outcome, or mixed.
pub fn status_of(entries: &[Entry]) -> Status {
	let mut outcomes = entries.iter().map(|entry| entry.outcome);
	match outcomes.next() {
		None => Status::Open,
		Some(first) if outcomes.all(|outcome| outcome == first) => Status::from(first),
		Some(_) => Status::Mixed,
	}
}

/// A decision to record.
#[derive(Clone, Debug)]
pub struct NewDecision {
	pub vulnerability_id: String,
	pub source: Source,
	pub entries: Vec<Entry>,
	pub actor: Actor,
	pub via: Via,
}

/// Whether `entries` are a decision for an item that `reaching` workspace
/// packages reach: each entry holds what its outcome requires, every workspace
/// package is covered, none twice, none that does not reach the advisory, at
/// most one catch-all and it last, and a Linux item only a whole decision.
pub fn validate_entries(entries: &[Entry], source: &Source, reaching: &[String]) -> Result<()> {
	let bad = |message: String| Err(AppError::BadRequest(message));
	if entries.is_empty() {
		return bad("a decision needs at least one entry".into());
	}
	for entry in entries {
		entry.check()?;
	}
	let catch_alls = entries.iter().filter(|entry| entry.is_catch_all()).count();
	if catch_alls > 1 {
		return bad("a decision has at most one catch-all entry".into());
	}
	if catch_alls == 1 && !entries.last().is_some_and(Entry::is_catch_all) {
		return bad("the catch-all entry must be last".into());
	}
	if source.is_linux() && !(entries.len() == 1 && catch_alls == 1) {
		return bad("a Linux item takes only a whole decision".into());
	}
	let mut named: HashSet<&str> = HashSet::new();
	for entry in entries {
		let Some(packages) = &entry.workspace_packages else {
			continue;
		};
		if packages.is_empty() {
			return bad(
				"an entry names at least one workspace package, or is the catch-all".into(),
			);
		}
		for package in packages {
			if !named.insert(package) {
				return bad(format!("{package} is in more than one entry"));
			}
			if !reaching.contains(package) {
				return bad(format!("{package} does not reach the advisory"));
			}
		}
	}
	if catch_alls == 0
		&& let Some(uncovered) = reaching
			.iter()
			.find(|package| !named.contains(package.as_str()))
	{
		return bad(format!("{uncovered} is not covered by the decision"));
	}
	Ok(())
}

impl NewDecision {
	pub fn validate(&self, reaching: &[String]) -> Result<()> {
		validate_entries(&self.entries, &self.source, reaching)
	}
}

// ── Decisions ───────────────────────────────────────────────────────────────

/// A triage decision. The newest one for an item is its current decision,
/// unless it was withdrawn or has lapsed.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityDecision)]
pub struct Decision {
	/// The decision's identifier.
	pub id: Uuid,
	/// The advisory's OSV identifier.
	pub vulnerability_id: String,
	/// The source it decides the advisory for.
	pub source: Source,
	/// What it decides, for which workspace packages.
	pub entries: Vec<Entry>,
	/// The status it gives the item.
	pub status: Status,
	/// Who decided it.
	pub actor: Actor,
	/// How it was decided.
	pub via: Via,
	/// When it was decided.
	pub created_at: Timestamp,
	/// The workspace packages it covered when it was made.
	pub covered_held: Vec<String>,
	/// Whether any entry's reason or issue link was edited since.
	pub edited: bool,
	/// Set when it was withdrawn.
	pub withdrawal: Option<Withdrawal>,
	#[serde(skip)]
	#[schema(ignore)]
	pub held: Option<Held>,
}

#[derive(diesel::QueryableByName)]
struct DecisionRow {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Jsonb)]
	actor: serde_json::Value,
	#[diesel(sql_type = sql_types::Text)]
	via: String,
	#[diesel(sql_type = sql_types::Text)]
	severity_held: String,
	#[diesel(sql_type = sql_types::Jsonb)]
	affected_held: serde_json::Value,
	#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
	covered_held: Vec<Option<String>>,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
	#[diesel(sql_type = sql_types::Bool)]
	edited: bool,
}

#[derive(diesel::QueryableByName)]
struct EntryRow {
	#[diesel(sql_type = sql_types::Uuid)]
	decision_id: Uuid,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Array<sql_types::Nullable<sql_types::Text>>>)]
	workspace_packages: Option<Vec<Option<String>>>,
	#[diesel(sql_type = sql_types::Text)]
	outcome: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	reason: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	issue_url: Option<String>,
}

impl EntryRow {
	fn entry(self) -> (Uuid, Entry) {
		(
			self.decision_id,
			Entry {
				workspace_packages: self.workspace_packages.map(strings),
				outcome: Outcome::parse(&self.outcome).unwrap_or(Outcome::WillFix),
				reason: self.reason,
				issue_url: self.issue_url,
			},
		)
	}
}

const DECISION_COLUMNS: &str = "d.id, d.vulnerability_id, d.source, d.actor, d.via, \
	d.severity_held, d.affected_held, d.covered_held, d.created_at, \
	EXISTS (SELECT 1 FROM vulnerability_decision_edits e WHERE e.decision_id = d.id) AS edited";

/// The newest decision of each item, whether or not it still stands.
const NEWEST: &str = "(SELECT DISTINCT ON (vulnerability_id, source) * FROM vulnerability_decisions \
	ORDER BY vulnerability_id, source, created_at DESC, id)";

/// A decision that has neither been withdrawn nor lapsed.
const STANDING: &str = "NOT EXISTS (SELECT 1 FROM vulnerability_decision_withdrawals w WHERE w.decision_id = d.id) \
	AND NOT EXISTS (SELECT 1 FROM vulnerability_lapses l WHERE l.decision_id = d.id)";

async fn hydrate(conn: &mut AsyncPgConnection, rows: Vec<DecisionRow>) -> Result<Vec<Decision>> {
	let ids: Vec<Uuid> = rows.iter().map(|row| row.id).collect();
	let entry_rows: Vec<EntryRow> = diesel::sql_query(
		"SELECT decision_id, workspace_packages, outcome, reason, issue_url \
		 FROM vulnerability_decision_entries WHERE decision_id = ANY($1) \
		 ORDER BY decision_id, position",
	)
	.bind::<sql_types::Array<sql_types::Uuid>, _>(&ids)
	.load(conn)
	.await?;
	let mut entries: HashMap<Uuid, Vec<Entry>> = HashMap::new();
	for (decision, entry) in entry_rows.into_iter().map(EntryRow::entry) {
		entries.entry(decision).or_default().push(entry);
	}
	let mut withdrawals = Withdrawal::for_decisions(conn, &ids).await?;
	Ok(rows
		.into_iter()
		.filter_map(|row| {
			let entries = entries.remove(&row.id).unwrap_or_default();
			Some(Decision {
				id: row.id,
				vulnerability_id: row.vulnerability_id,
				source: Source::parse(&row.source)?,
				status: status_of(&entries),
				entries,
				actor: serde_json::from_value(row.actor)
					.unwrap_or_else(|_| Actor::canopy("unknown")),
				via: Via::parse(&row.via).unwrap_or(Via::Ui),
				created_at: row.created_at,
				covered_held: strings(row.covered_held),
				edited: row.edited,
				withdrawal: withdrawals.remove(&row.id),
				held: Some(Held {
					severity: row.severity_held.parse().unwrap_or_default(),
					affected: serde_json::from_value(row.affected_held).unwrap_or_default(),
				}),
			})
		})
		.collect())
}

/// The workspace packages that reach an item's advisory now.
pub async fn reaching_workspace_packages(
	conn: &mut AsyncPgConnection,
	vulnerability_id: &str,
	source: &Source,
) -> Result<Vec<String>> {
	#[derive(diesel::QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Text)]
		workspace_package: String,
	}
	let rows: Vec<Row> = diesel::sql_query(format!(
		"WITH {EXPOSURE_CTE} \
		 SELECT DISTINCT p.workspace_package FROM exposure e \
		 JOIN repository_package_paths p ON p.repository = e.source AND p.ecosystem = e.ecosystem \
		   AND p.name = e.name AND p.version = e.version \
		 WHERE e.source = $3 ORDER BY 1"
	))
	.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(Some(vec![vulnerability_id]))
	.bind::<sql_types::Text, _>(commons_types::server::rank::ServerRank::Production.to_string())
	.bind::<sql_types::Text, _>(source.as_str())
	.load(conn)
	.await?;
	Ok(rows.into_iter().map(|row| row.workspace_package).collect())
}

impl Decision {
	/// Record a decision as the item's current one. The advisory's severity and
	/// affected ranges as they stand are kept with it, and the workspace
	/// packages it covers.
	pub async fn record(conn: &mut AsyncPgConnection, new: NewDecision) -> Result<Self> {
		Self::lock_item(conn, &new.vulnerability_id, &new.source).await?;
		let reaching =
			reaching_workspace_packages(conn, &new.vulnerability_id, &new.source).await?;
		new.validate(&reaching)?;
		let entries: Vec<Entry> = new.entries.into_iter().map(Entry::normalised).collect();

		let row: DecisionRow = diesel::sql_query(format!(
			"WITH d AS ( \
			   INSERT INTO vulnerability_decisions \
			   (vulnerability_id, source, actor, via, severity_held, affected_held, covered_held) \
			   SELECT $1, $2, $3, $4, a.severity, a.affected, $5 \
			   FROM vulnerabilities a WHERE a.id = $1 \
			   RETURNING *) \
			 SELECT {DECISION_COLUMNS} FROM d"
		))
		.bind::<sql_types::Text, _>(&new.vulnerability_id)
		.bind::<sql_types::Text, _>(new.source.as_str())
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(&new.actor).unwrap_or_default())
		.bind::<sql_types::Text, _>(new.via.as_str())
		.bind::<sql_types::Array<sql_types::Text>, _>(&reaching)
		.get_result(conn)
		.await?;

		for (position, entry) in entries.iter().enumerate() {
			diesel::sql_query(
				"INSERT INTO vulnerability_decision_entries \
				 (decision_id, position, workspace_packages, outcome, reason, issue_url) \
				 VALUES ($1, $2, $3, $4, $5, $6)",
			)
			.bind::<sql_types::Uuid, _>(row.id)
			.bind::<sql_types::Integer, _>(position as i32)
			.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(
				entry.workspace_packages.as_ref(),
			)
			.bind::<sql_types::Text, _>(entry.outcome.as_str())
			.bind::<sql_types::Nullable<sql_types::Text>, _>(entry.reason.as_ref())
			.bind::<sql_types::Nullable<sql_types::Text>, _>(entry.issue_url.as_ref())
			.execute(conn)
			.await?;
		}
		let mut decisions = hydrate(conn, vec![row]).await?;
		decisions
			.pop()
			.ok_or_else(|| AppError::Custom("a decision was recorded for an unknown source".into()))
	}

	/// The audit entry for having recorded this decision.
	pub fn audited(&self) -> Action {
		Action::VulnerabilityDecided {
			advisory: advisory_record(&self.vulnerability_id),
			source: self.source.as_str().to_owned(),
			entries: serde_json::to_value(&self.entries).unwrap_or_default(),
			via: self.via.as_str().to_owned(),
		}
	}

	/// Hold the item until the transaction ends, so decisions on it are
	/// recorded one at a time and a check made under the hold stays true until
	/// the decision is written. An item that does not exist is not found.
	pub async fn lock_item(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<()> {
		let held = diesel::sql_query(
			"SELECT 1 FROM vulnerability_items \
			 WHERE vulnerability_id = $1 AND source = $2 FOR UPDATE",
		)
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.execute(conn)
		.await?;
		if held == 0 {
			return Err(AppError::NotFound(format!(
				"no {} vulnerability {vulnerability_id}",
				source.as_str()
			)));
		}
		Ok(())
	}

	/// The decision that stands for an item: its newest, if that was neither
	/// withdrawn nor has lapsed.
	pub async fn standing(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<Option<Self>> {
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT {DECISION_COLUMNS} FROM {NEWEST} d \
			 WHERE d.vulnerability_id = $1 AND d.source = $2 AND {STANDING}"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await?;
		Ok(hydrate(conn, rows).await?.pop())
	}

	/// The decisions the model made, which still stand.
	pub async fn standing_by_model(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT {DECISION_COLUMNS} FROM {NEWEST} d WHERE d.via = 'model' AND {STANDING} \
			 ORDER BY d.created_at"
		))
		.load(conn)
		.await?;
		hydrate(conn, rows).await
	}

	/// The newest decision of each of `items`, whether or not it stands, so a
	/// list can say what became of it.
	pub async fn newest_of(
		conn: &mut AsyncPgConnection,
		items: &[(String, String)],
	) -> Result<HashMap<(String, Source), Self>> {
		let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
		let sources: Vec<&str> = items.iter().map(|(_, source)| source.as_str()).collect();
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT {DECISION_COLUMNS} FROM {NEWEST} d \
			 WHERE (d.vulnerability_id, d.source) IN \
			   (SELECT * FROM unnest($1::text[], $2::text[]))"
		))
		.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
		.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
		.load(conn)
		.await?;
		Ok(hydrate(conn, rows)
			.await?
			.into_iter()
			.map(|decision| {
				(
					(decision.vulnerability_id.clone(), decision.source.clone()),
					decision,
				)
			})
			.collect())
	}

	/// An item's decisions, newest first.
	pub async fn history(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<Vec<Self>> {
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT {DECISION_COLUMNS} FROM vulnerability_decisions d \
			 WHERE d.vulnerability_id = $1 AND d.source = $2 ORDER BY d.created_at DESC, d.id"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await?;
		hydrate(conn, rows).await
	}

	/// Take the item's current decision away, with a reason. The item needs
	/// triage again, with the suggestion it had.
	pub async fn withdraw(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
		reason: &str,
		actor: Actor,
		via: Via,
	) -> Result<Withdrawal> {
		Self::lock_item(conn, vulnerability_id, source).await?;
		let reason = reason.trim();
		if reason.is_empty() {
			return Err(AppError::BadRequest("a withdrawal needs a reason".into()));
		}
		let decision = Self::standing(conn, vulnerability_id, source)
			.await?
			.ok_or_else(|| {
				AppError::BadRequest(format!(
					"{vulnerability_id} has no current decision to withdraw"
				))
			})?;
		Withdrawal::insert(conn, &decision, reason, &actor, via).await
	}

	/// Correct the reason or issue link of the current decision's entry that
	/// covers `workspace_package`, or its catch-all when that is `None`. The
	/// earlier text is kept with the edit. A reason or link left `None` is left
	/// as it is, and an issue link given blank is cleared.
	#[allow(clippy::too_many_arguments)]
	pub async fn edit(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
		workspace_package: Option<&str>,
		reason: Option<&str>,
		issue_url: Option<&str>,
		actor: Actor,
		via: Via,
	) -> Result<Edit> {
		Self::lock_item(conn, vulnerability_id, source).await?;
		if reason.is_none() && issue_url.is_none() {
			return Err(AppError::BadRequest(
				"an edit changes the reason or the issue link".into(),
			));
		}
		let decision = Self::standing(conn, vulnerability_id, source)
			.await?
			.ok_or_else(|| {
				AppError::BadRequest(format!(
					"{vulnerability_id} has no current decision to edit"
				))
			})?;
		let position = decision
			.entries
			.iter()
			.position(
				|entry| match (workspace_package, &entry.workspace_packages) {
					(None, None) => true,
					(Some(package), Some(packages)) => packages.iter().any(|held| held == package),
					_ => false,
				},
			)
			.ok_or_else(|| {
				AppError::BadRequest(match workspace_package {
					Some(package) => format!("no entry of the decision covers {package}"),
					None => "the decision has no catch-all entry".into(),
				})
			})?;
		let earlier = &decision.entries[position];

		let mut entry = earlier.clone();
		if let Some(reason) = reason {
			entry.reason = Some(reason.to_owned());
		}
		if let Some(issue_url) = issue_url {
			if !issue_url.trim().is_empty() && entry.outcome != Outcome::WillFix {
				return Err(AppError::BadRequest(
					"only a will fix entry has an issue link".into(),
				));
			}
			entry.issue_url = Some(issue_url.to_owned());
		}
		entry.check()?;
		let entry = entry.normalised();
		if entry.reason == earlier.reason && entry.issue_url == earlier.issue_url {
			return Err(AppError::BadRequest("the edit changes nothing".into()));
		}

		diesel::sql_query(
			"UPDATE vulnerability_decision_entries SET reason = $3, issue_url = $4 \
			 WHERE decision_id = $1 AND position = $2",
		)
		.bind::<sql_types::Uuid, _>(decision.id)
		.bind::<sql_types::Integer, _>(position as i32)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(entry.reason.as_ref())
		.bind::<sql_types::Nullable<sql_types::Text>, _>(entry.issue_url.as_ref())
		.execute(conn)
		.await?;
		Edit::insert(conn, &decision, position, &entry, earlier, &actor, via).await
	}
}

fn advisory_record(id: &str) -> audit::Record {
	audit::Record::new(RecordKind::Advisory, id, id)
}

// ── Withdrawals ─────────────────────────────────────────────────────────────

/// The current decision taken away.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema, diesel::QueryableByName)]
#[schema(as = VulnerabilityWithdrawal)]
pub struct Withdrawal {
	/// The withdrawal's identifier.
	#[diesel(sql_type = sql_types::Uuid)]
	pub id: Uuid,
	/// The decision withdrawn.
	#[diesel(sql_type = sql_types::Uuid)]
	pub decision_id: Uuid,
	/// Why.
	#[diesel(sql_type = sql_types::Text)]
	pub reason: String,
	/// Who withdrew it.
	#[diesel(sql_type = sql_types::Jsonb, deserialize_as = ActorColumn)]
	pub actor: Actor,
	/// How it was withdrawn.
	#[diesel(sql_type = sql_types::Text, deserialize_as = ViaColumn)]
	pub via: Via,
	/// When.
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// The advisory's OSV identifier.
	#[diesel(sql_type = sql_types::Text)]
	pub vulnerability_id: String,
	/// The source of the item.
	#[diesel(sql_type = sql_types::Text, deserialize_as = SourceColumn)]
	pub source: Source,
}

/// Row decoders for the columns every history record shares.
pub struct ActorColumn(Actor);
pub struct ViaColumn(Via);
pub struct SourceColumn(Source);

impl diesel::deserialize::FromSql<sql_types::Jsonb, diesel::pg::Pg> for ActorColumn {
	fn from_sql(bytes: diesel::pg::PgValue<'_>) -> diesel::deserialize::Result<Self> {
		let value = <serde_json::Value as diesel::deserialize::FromSql<
			sql_types::Jsonb,
			diesel::pg::Pg,
		>>::from_sql(bytes)?;
		Ok(Self(
			serde_json::from_value(value).unwrap_or_else(|_| Actor::canopy("unknown")),
		))
	}
}
impl From<ActorColumn> for Actor {
	fn from(column: ActorColumn) -> Self {
		column.0
	}
}
impl diesel::deserialize::FromSql<sql_types::Text, diesel::pg::Pg> for ViaColumn {
	fn from_sql(bytes: diesel::pg::PgValue<'_>) -> diesel::deserialize::Result<Self> {
		let text =
			<String as diesel::deserialize::FromSql<sql_types::Text, diesel::pg::Pg>>::from_sql(
				bytes,
			)?;
		Ok(Self(Via::parse(&text).unwrap_or(Via::Ui)))
	}
}
impl From<ViaColumn> for Via {
	fn from(column: ViaColumn) -> Self {
		column.0
	}
}
impl diesel::deserialize::FromSql<sql_types::Text, diesel::pg::Pg> for SourceColumn {
	fn from_sql(bytes: diesel::pg::PgValue<'_>) -> diesel::deserialize::Result<Self> {
		let text =
			<String as diesel::deserialize::FromSql<sql_types::Text, diesel::pg::Pg>>::from_sql(
				bytes,
			)?;
		Ok(Self(Source::parse(&text).unwrap_or(Source::Linux)))
	}
}
impl From<SourceColumn> for Source {
	fn from(column: SourceColumn) -> Self {
		column.0
	}
}

const WITHDRAWAL_COLUMNS: &str = "w.id, w.decision_id, w.reason, w.actor, w.via, w.created_at, \
	d.vulnerability_id, d.source";

impl Withdrawal {
	async fn insert(
		conn: &mut AsyncPgConnection,
		decision: &Decision,
		reason: &str,
		actor: &Actor,
		via: Via,
	) -> Result<Self> {
		let row: Self = diesel::sql_query(format!(
			"WITH w AS ( \
			   INSERT INTO vulnerability_decision_withdrawals (decision_id, reason, actor, via) \
			   VALUES ($1, $2, $3, $4) RETURNING *) \
			 SELECT {WITHDRAWAL_COLUMNS} FROM w JOIN vulnerability_decisions d ON d.id = w.decision_id"
		))
		.bind::<sql_types::Uuid, _>(decision.id)
		.bind::<sql_types::Text, _>(reason)
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(actor).unwrap_or_default())
		.bind::<sql_types::Text, _>(via.as_str())
		.get_result(conn)
		.await?;
		Ok(row)
	}

	async fn for_decisions(
		conn: &mut AsyncPgConnection,
		decisions: &[Uuid],
	) -> Result<HashMap<Uuid, Self>> {
		let rows: Vec<Self> = diesel::sql_query(format!(
			"SELECT {WITHDRAWAL_COLUMNS} FROM vulnerability_decision_withdrawals w \
			 JOIN vulnerability_decisions d ON d.id = w.decision_id WHERE w.decision_id = ANY($1)"
		))
		.bind::<sql_types::Array<sql_types::Uuid>, _>(decisions)
		.load(conn)
		.await?;
		Ok(rows.into_iter().map(|row| (row.decision_id, row)).collect())
	}

	/// An item's withdrawals, newest first.
	pub async fn history(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<Vec<Self>> {
		diesel::sql_query(format!(
			"SELECT {WITHDRAWAL_COLUMNS} FROM vulnerability_decision_withdrawals w \
			 JOIN vulnerability_decisions d ON d.id = w.decision_id \
			 WHERE d.vulnerability_id = $1 AND d.source = $2 ORDER BY w.created_at DESC"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await
		.map_err(AppError::from)
	}

	/// The audit entry for having withdrawn the decision.
	pub fn audited(&self) -> Action {
		Action::VulnerabilityDecisionWithdrawn {
			advisory: advisory_record(&self.vulnerability_id),
			source: self.source.as_str().to_owned(),
			decision_id: self.decision_id.to_string(),
			reason: self.reason.clone(),
			via: self.via.as_str().to_owned(),
		}
	}
}

// ── Edits ───────────────────────────────────────────────────────────────────

/// A correction to an entry's reason or issue link.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema, diesel::QueryableByName)]
#[schema(as = VulnerabilityDecisionEdit)]
pub struct Edit {
	/// The edit's identifier.
	#[diesel(sql_type = sql_types::Uuid)]
	pub id: Uuid,
	/// The decision edited.
	#[diesel(sql_type = sql_types::Uuid)]
	pub decision_id: Uuid,
	/// The workspace packages of the entry edited, `None` for the catch-all.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Array<sql_types::Nullable<sql_types::Text>>>, deserialize_as = OptionalStrings)]
	pub workspace_packages: Option<Vec<String>>,
	/// The reason after the edit.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub reason: Option<String>,
	/// The issue link after the edit.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub issue_url: Option<String>,
	/// The reason before it.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub earlier_reason: Option<String>,
	/// The issue link before it.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub earlier_issue_url: Option<String>,
	/// Who edited it.
	#[diesel(sql_type = sql_types::Jsonb, deserialize_as = ActorColumn)]
	pub actor: Actor,
	/// How it was edited.
	#[diesel(sql_type = sql_types::Text, deserialize_as = ViaColumn)]
	pub via: Via,
	/// When.
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// The advisory's OSV identifier.
	#[diesel(sql_type = sql_types::Text)]
	pub vulnerability_id: String,
	/// The source of the item.
	#[diesel(sql_type = sql_types::Text, deserialize_as = SourceColumn)]
	pub source: Source,
}

/// A nullable text array read as strings.
pub struct OptionalStrings(Option<Vec<String>>);
impl
	diesel::deserialize::FromSql<
		sql_types::Nullable<sql_types::Array<sql_types::Nullable<sql_types::Text>>>,
		diesel::pg::Pg,
	> for OptionalStrings
{
	fn from_sql(bytes: diesel::pg::PgValue<'_>) -> diesel::deserialize::Result<Self> {
		let array = <Vec<Option<String>> as diesel::deserialize::FromSql<
			sql_types::Array<sql_types::Nullable<sql_types::Text>>,
			diesel::pg::Pg,
		>>::from_sql(bytes)?;
		Ok(Self(Some(strings(array))))
	}

	fn from_nullable_sql(
		bytes: Option<diesel::pg::PgValue<'_>>,
	) -> diesel::deserialize::Result<Self> {
		match bytes {
			Some(bytes) => Self::from_sql(bytes),
			None => Ok(Self(None)),
		}
	}
}
impl From<OptionalStrings> for Option<Vec<String>> {
	fn from(column: OptionalStrings) -> Self {
		column.0
	}
}

const EDIT_COLUMNS: &str = "e.id, e.decision_id, n.workspace_packages, e.reason, e.issue_url, \
	e.earlier_reason, e.earlier_issue_url, e.actor, e.via, e.created_at, d.vulnerability_id, d.source";

impl Edit {
	async fn insert(
		conn: &mut AsyncPgConnection,
		decision: &Decision,
		position: usize,
		entry: &Entry,
		earlier: &Entry,
		actor: &Actor,
		via: Via,
	) -> Result<Self> {
		let row: Self = diesel::sql_query(format!(
			"WITH e AS ( \
			   INSERT INTO vulnerability_decision_edits \
			   (decision_id, position, reason, issue_url, earlier_reason, earlier_issue_url, actor, via) \
			   VALUES ($1, $2, $3, $4, $5, $6, $7, $8) RETURNING *) \
			 SELECT {EDIT_COLUMNS} FROM e \
			 JOIN vulnerability_decisions d ON d.id = e.decision_id \
			 JOIN vulnerability_decision_entries n ON n.decision_id = e.decision_id AND n.position = e.position"
		))
		.bind::<sql_types::Uuid, _>(decision.id)
		.bind::<sql_types::Integer, _>(position as i32)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(entry.reason.as_ref())
		.bind::<sql_types::Nullable<sql_types::Text>, _>(entry.issue_url.as_ref())
		.bind::<sql_types::Nullable<sql_types::Text>, _>(earlier.reason.as_ref())
		.bind::<sql_types::Nullable<sql_types::Text>, _>(earlier.issue_url.as_ref())
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(actor).unwrap_or_default())
		.bind::<sql_types::Text, _>(via.as_str())
		.get_result(conn)
		.await?;
		Ok(row)
	}

	/// An item's edits, newest first.
	pub async fn history(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<Vec<Self>> {
		diesel::sql_query(format!(
			"SELECT {EDIT_COLUMNS} FROM vulnerability_decision_edits e \
			 JOIN vulnerability_decisions d ON d.id = e.decision_id \
			 JOIN vulnerability_decision_entries n ON n.decision_id = e.decision_id AND n.position = e.position \
			 WHERE d.vulnerability_id = $1 AND d.source = $2 ORDER BY e.created_at DESC"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await
		.map_err(AppError::from)
	}

	/// The audit entry for having made the edit.
	pub fn audited(&self) -> Action {
		Action::VulnerabilityDecisionEdited {
			advisory: advisory_record(&self.vulnerability_id),
			source: self.source.as_str().to_owned(),
			decision_id: self.decision_id.to_string(),
			entry: serde_json::to_value(&self.workspace_packages).unwrap_or_default(),
			reason: self.reason.clone(),
			issue_url: self.issue_url.clone(),
			earlier_reason: self.earlier_reason.clone(),
			earlier_issue_url: self.earlier_issue_url.clone(),
			via: self.via.as_str().to_owned(),
		}
	}
}

// ── Lapses ──────────────────────────────────────────────────────────────────

/// A decision that stopped standing, and why.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityLapse)]
pub struct Lapse {
	/// The lapse's identifier.
	pub id: Uuid,
	/// The advisory's OSV identifier.
	pub vulnerability_id: String,
	/// The source the item belongs to.
	pub source: Source,
	/// The decision that lapsed.
	pub decision_id: Uuid,
	/// Why, in words.
	pub why: String,
	/// Why, as a tagged reason: `severity_rose`, `range_widened`,
	/// `new_workspace_package`, or `review_due` for one recorded before review
	/// dates were dropped.
	#[schema(value_type = Object)]
	pub reason: LapseReason,
	/// When it lapsed.
	pub created_at: Timestamp,
}

#[derive(diesel::QueryableByName)]
struct LapseRow {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Uuid)]
	decision_id: Uuid,
	#[diesel(sql_type = sql_types::Jsonb)]
	reason: serde_json::Value,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
}

impl LapseRow {
	fn lapse(self) -> Option<Lapse> {
		let reason: LapseReason =
			serde_json::from_value(self.reason).unwrap_or(LapseReason::ReviewDue);
		Some(Lapse {
			id: self.id,
			vulnerability_id: self.vulnerability_id,
			source: Source::parse(&self.source)?,
			decision_id: self.decision_id,
			why: reason.describe(),
			reason,
			created_at: self.created_at,
		})
	}
}

const LAPSE_COLUMNS: &str = "id, vulnerability_id, source, decision_id, reason, created_at";

impl Lapse {
	/// Let a decision lapse. The item's suggestion goes, so the model looks at
	/// it afresh, and a decision lapses once.
	pub async fn record(
		conn: &mut AsyncPgConnection,
		decision: &Decision,
		reason: &LapseReason,
	) -> Result<Option<Self>> {
		let row: Option<LapseRow> = diesel::sql_query(format!(
			"INSERT INTO vulnerability_lapses (vulnerability_id, source, decision_id, reason) \
			 VALUES ($1, $2, $3, $4) ON CONFLICT (decision_id) DO NOTHING \
			 RETURNING {LAPSE_COLUMNS}"
		))
		.bind::<sql_types::Text, _>(&decision.vulnerability_id)
		.bind::<sql_types::Text, _>(decision.source.as_str())
		.bind::<sql_types::Uuid, _>(decision.id)
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(reason).unwrap_or_default())
		.get_result(conn)
		.await
		.optional()?;
		let Some(row) = row else { return Ok(None) };

		diesel::sql_query(
			"DELETE FROM vulnerability_suggestions WHERE vulnerability_id = $1 AND source = $2",
		)
		.bind::<sql_types::Text, _>(&decision.vulnerability_id)
		.bind::<sql_types::Text, _>(decision.source.as_str())
		.execute(conn)
		.await?;
		diesel::sql_query(
			"UPDATE vulnerability_items SET model_attempted_at = NULL \
			 WHERE vulnerability_id = $1 AND source = $2",
		)
		.bind::<sql_types::Text, _>(&decision.vulnerability_id)
		.bind::<sql_types::Text, _>(decision.source.as_str())
		.execute(conn)
		.await?;
		Ok(row.lapse())
	}

	/// Look at every standing decision, and let lapse each whose advisory has
	/// changed so that it no longer covers what its source carries, or that does
	/// not cover a workspace package that now reaches the advisory. Returns what
	/// lapsed, for the caller to audit.
	pub async fn pass(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT {DECISION_COLUMNS} FROM {NEWEST} d WHERE {STANDING}"
		))
		.load(conn)
		.await?;
		let standing = hydrate(conn, rows).await?;
		let counted = counted_by_source(conn).await?;

		let ids: Vec<String> = standing
			.iter()
			.map(|decision| decision.vulnerability_id.clone())
			.collect();
		let advisories: HashMap<String, Advisory> = Advisory::get_many(conn, &ids)
			.await?
			.into_iter()
			.map(|advisory| (advisory.id.clone(), advisory))
			.collect();

		let mut lapsed = Vec::new();
		for decision in standing {
			let counted_here = counted
				.get(&decision.source)
				.map(Vec::as_slice)
				.unwrap_or_default();
			let mut reason = match (&decision.held, advisories.get(&decision.vulnerability_id)) {
				(Some(held), Some(advisory)) => {
					held.lapsed_by(advisory.severity, &advisory.affected, counted_here)
				}
				_ => None,
			};
			if reason.is_none() && !decision.entries.iter().any(Entry::is_catch_all) {
				let reaching =
					reaching_workspace_packages(conn, &decision.vulnerability_id, &decision.source)
						.await?;
				reason = reaching
					.into_iter()
					.find(|package| !decision.covered_held.contains(package))
					.map(|name| LapseReason::NewWorkspacePackage { name });
			}
			if let Some(reason) = reason
				&& let Some(done) = Self::record(conn, &decision, &reason).await?
			{
				lapsed.push(done);
			}
		}
		Ok(lapsed)
	}

	/// The lapses of the newest decision of each of `items`.
	pub async fn of_newest(
		conn: &mut AsyncPgConnection,
		items: &[(String, String)],
	) -> Result<HashMap<(String, Source), Self>> {
		let ids: Vec<&str> = items.iter().map(|(id, _)| id.as_str()).collect();
		let sources: Vec<&str> = items.iter().map(|(_, source)| source.as_str()).collect();
		let rows: Vec<LapseRow> = diesel::sql_query(
			"SELECT l.id, l.vulnerability_id, l.source, l.decision_id, l.reason, l.created_at \
			 FROM vulnerability_lapses l \
			 JOIN (SELECT DISTINCT ON (vulnerability_id, source) id, vulnerability_id, source \
			       FROM vulnerability_decisions \
			       ORDER BY vulnerability_id, source, created_at DESC, id) d ON d.id = l.decision_id \
			 WHERE (l.vulnerability_id, l.source) IN (SELECT * FROM unnest($1::text[], $2::text[]))",
		)
		.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
		.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
		.load(conn)
		.await?;
		Ok(rows
			.into_iter()
			.filter_map(LapseRow::lapse)
			.map(|lapse| {
				(
					(lapse.vulnerability_id.clone(), lapse.source.clone()),
					lapse,
				)
			})
			.collect())
	}

	/// An item's lapses, newest first.
	pub async fn history(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: &Source,
	) -> Result<Vec<Self>> {
		let rows: Vec<LapseRow> = diesel::sql_query(format!(
			"SELECT {LAPSE_COLUMNS} FROM vulnerability_lapses \
			 WHERE vulnerability_id = $1 AND source = $2 ORDER BY created_at DESC"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await?;
		Ok(rows.into_iter().filter_map(LapseRow::lapse).collect())
	}

	/// The audit entry for the lapse, taken by Canopy.
	pub fn audited(&self) -> Action {
		Action::VulnerabilityReopened {
			advisory: advisory_record(&self.vulnerability_id),
			source: self.source.as_str().to_owned(),
			why: self.why.clone(),
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn entry(packages: Option<&[&str]>, outcome: Outcome, reason: Option<&str>) -> Entry {
		Entry {
			workspace_packages: packages.map(|p| p.iter().map(|s| (*s).to_owned()).collect()),
			outcome,
			reason: reason.map(str::to_owned),
			issue_url: None,
		}
	}

	fn tamanu() -> Source {
		Source::Repository("tamanu".into())
	}

	fn reaching() -> Vec<String> {
		vec!["mobile".into(), "server".into(), "web".into()]
	}

	fn refused(entries: &[Entry], source: &Source) -> String {
		match validate_entries(entries, source, &reaching()) {
			Err(AppError::BadRequest(message)) => message,
			other => panic!("expected a refusal, got {other:?}"),
		}
	}

	#[test]
	fn not_affected_and_accepted_risk_need_a_reason() {
		for outcome in [Outcome::NotAffected, Outcome::AcceptedRisk] {
			assert!(entry(None, outcome, None).check().is_err());
			assert!(entry(None, outcome, Some("   ")).check().is_err());
			assert!(
				entry(None, outcome, Some("behind the firewall"))
					.check()
					.is_ok()
			);
		}
		assert!(entry(None, Outcome::WillFix, None).check().is_ok());
	}

	#[test]
	fn an_issue_link_must_be_a_web_address() {
		let mut fix = entry(None, Outcome::WillFix, None);
		fix.issue_url = Some("https://github.com/beyondessential/tamanu/issues/1".into());
		assert!(fix.check().is_ok());
		for bad in ["javascript:alert(1)", "data:text/html,x", "not a link"] {
			fix.issue_url = Some(bad.into());
			assert!(fix.check().is_err(), "{bad}");
		}
	}

	#[test]
	fn a_whole_decision_is_one_catch_all() {
		let whole = [entry(None, Outcome::WillFix, None)];
		assert!(validate_entries(&whole, &tamanu(), &reaching()).is_ok());
		assert!(validate_entries(&whole, &Source::Linux, &[]).is_ok());
	}

	#[test]
	fn a_split_covers_every_workspace_package() {
		let split = [
			entry(Some(&["mobile"]), Outcome::WillFix, None),
			entry(
				Some(&["server"]),
				Outcome::AcceptedRisk,
				Some("internal only"),
			),
			entry(None, Outcome::NotAffected, Some("not used")),
		];
		assert!(validate_entries(&split, &tamanu(), &reaching()).is_ok());
		let no_catch_all = [
			entry(Some(&["mobile", "server"]), Outcome::WillFix, None),
			entry(Some(&["web"]), Outcome::WillFix, None),
		];
		assert!(validate_entries(&no_catch_all, &tamanu(), &reaching()).is_ok());
	}

	#[test]
	fn a_split_is_refused_when_it_breaks_the_rules() {
		let uncovered = [entry(Some(&["mobile"]), Outcome::WillFix, None)];
		assert!(refused(&uncovered, &tamanu()).contains("not covered"));

		let twice = [
			entry(Some(&["mobile"]), Outcome::WillFix, None),
			entry(Some(&["mobile"]), Outcome::WillFix, None),
			entry(None, Outcome::WillFix, None),
		];
		assert!(refused(&twice, &tamanu()).contains("more than one entry"));

		let unreached = [
			entry(Some(&["desktop"]), Outcome::WillFix, None),
			entry(None, Outcome::WillFix, None),
		];
		assert!(refused(&unreached, &tamanu()).contains("does not reach"));

		let two_catch_alls = [
			entry(None, Outcome::WillFix, None),
			entry(None, Outcome::WillFix, None),
		];
		assert!(refused(&two_catch_alls, &tamanu()).contains("at most one catch-all"));

		let not_last = [
			entry(None, Outcome::WillFix, None),
			entry(Some(&["mobile"]), Outcome::WillFix, None),
		];
		assert!(refused(&not_last, &tamanu()).contains("must be last"));

		let linux = [
			entry(Some(&["a"]), Outcome::WillFix, None),
			entry(None, Outcome::WillFix, None),
		];
		assert!(refused(&linux, &Source::Linux).contains("Linux"));
	}

	#[test]
	fn the_status_is_the_shared_outcome_or_mixed() {
		let same = [
			entry(Some(&["mobile"]), Outcome::WillFix, None),
			entry(None, Outcome::WillFix, None),
		];
		assert_eq!(status_of(&same), Status::WillFix);
		let mixed = [
			entry(Some(&["mobile"]), Outcome::WillFix, None),
			entry(None, Outcome::NotAffected, Some("x")),
		];
		assert_eq!(status_of(&mixed), Status::Mixed);
		assert_eq!(status_of(&[]), Status::Open);
	}
}
