//! The incident memory page: entries recording what past incidents taught.
//!
//! Spec: `.workhorse/specs/monitoring/incident-memory.md` (id `MEM`).
//!
//! Any tailnet user reads entries; recording, amending, retiring and restoring
//! are for administrators, and each is audited.

use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use database::{
	audit::{Action, Record},
	memory::{
		self, Amendment, Find, Link, LinkKind, MemoryEntryData, MemoryRevision, NewEntry, present,
		present_one,
	},
	notes_log::{Author, Via},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 100;
const MAX_LIMIT: i64 = 500;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: list))
		.routes(routes!(read_only, silent: get))
		.routes(routes!(write, audit(MemoryEntryRecorded): record))
		.routes(routes!(write, audit(MemoryEntryAmended): amend))
		.routes(routes!(write, audit(MemoryEntryRetired): retire))
		.routes(routes!(write, audit(MemoryEntryRestored): restore))
}

/// What to list. Everything is optional; the filters narrow together.
#[derive(Deserialize, ToSchema)]
pub struct ListArgs {
	/// Free text matched against titles and bodies.
	#[serde(default)]
	pub text: Option<String>,
	/// Only entries with this kind of link, or none.
	#[serde(default)]
	pub link_kind: Option<LinkKind>,
	/// Only entries linked to this group.
	#[serde(default)]
	pub group_id: Option<Uuid>,
	/// Only entries linked to the check with this source and name.
	#[serde(default)]
	pub check_source: Option<String>,
	#[serde(default)]
	pub check_name: Option<String>,
	/// Only entries linked to this incident.
	#[serde(default)]
	pub incident_id: Option<Uuid>,
	/// Only entries relevant to this incident: linked to it, to its target's
	/// group, or to a check among its issues.
	#[serde(default)]
	pub relevant_to_incident: Option<Uuid>,
	/// Include retired entries, which are otherwise left out.
	#[serde(default)]
	pub include_retired: bool,
	/// The most entries to return. Defaults to 100.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// The entries that matched.
#[derive(Serialize, ToSchema)]
pub struct MemoryList {
	/// Newest amended first.
	pub entries: Vec<MemoryEntryData>,
	/// How many entries matched in all, which exceeds `entries` when the list
	/// was cut short.
	pub total: i64,
}

/// List incident memory entries.
///
/// Newest amended first, narrowed by free text, link, and relevance to an
/// incident, and widened to retired entries on request.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "memory_list",
	tag = "memory",
	security(("tailscale-user" = [])),
	request_body = ListArgs,
	responses(
		(status = 200, body = MemoryList),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ListArgs>,
) -> Result<Json<MemoryList>> {
	let mut conn = state.db_read.get().await?;
	let found = memory::find(
		&mut conn,
		&Find {
			text: args.text,
			link_kind: args.link_kind,
			group: args.group_id,
			check: args.check_source.zip(args.check_name),
			incident: args.incident_id,
			relevant_to_incident: args.relevant_to_incident,
			include_retired: args.include_retired,
			limit: args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT),
		},
	)
	.await?;
	Ok(Json(MemoryList {
		entries: present(&mut conn, found.entries).await?,
		total: found.total,
	}))
}

/// Identifies an entry.
#[derive(Deserialize, ToSchema)]
pub struct EntryIdArgs {
	pub id: Uuid,
}

/// An entry with every version it has had.
#[derive(Serialize, ToSchema)]
pub struct MemoryEntryDetail {
	pub entry: MemoryEntryData,
	/// Newest first.
	pub history: Vec<MemoryRevision>,
}

/// Get an incident memory entry with its history.
#[utoipa::path(
	post,
	path = "/get",
	operation_id = "memory_get",
	tag = "memory",
	security(("tailscale-user" = [])),
	request_body = EntryIdArgs,
	responses(
		(status = 200, body = MemoryEntryDetail),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<EntryIdArgs>,
) -> Result<Json<MemoryEntryDetail>> {
	let mut conn = state.db_read.get().await?;
	let entry = memory::get(&mut conn, args.id).await?;
	let history = memory::history(&mut conn, args.id).await?;
	Ok(Json(MemoryEntryDetail {
		entry: present_one(&mut conn, entry).await?,
		history,
	}))
}

/// Record a new incident memory entry.
///
/// Requires the caller to be an administrator. Responds 400 if the title or
/// body is over its limit or the link names nothing.
#[utoipa::path(
	post,
	path = "/record",
	operation_id = "memory_record",
	tag = "memory",
	security(("tailscale-admin" = [])),
	request_body = NewEntry,
	responses(
		(status = 200, body = MemoryEntryData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn record(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<NewEntry>,
) -> Result<Json<MemoryEntryData>> {
	let mut conn = audit.begin().await?;
	let entry = memory::record(&mut conn, args, Author::operator(&admin.0.login)).await?;
	let link = Record::memory_link(&mut conn, &entry).await?;
	conn.record(Action::MemoryEntryRecorded {
		entry: Record::memory_entry(&entry),
		link,
		revision: entry.revision,
		via: Via::Operator.as_str().to_owned(),
	})
	.await?;
	Ok(Json(present_one(&mut conn, entry).await?))
}

/// An amendment, against the revision the caller read.
#[derive(Deserialize, ToSchema)]
pub struct AmendArgs {
	pub id: Uuid,
	/// The revision of the entry the caller read.
	pub revision: i64,
	pub title: Option<String>,
	pub body: Option<String>,
	/// A link to set, or `null` to clear it.
	#[serde(default, deserialize_with = "memory::present_option")]
	#[schema(value_type = Option<Link>)]
	pub link: Option<Option<Link>>,
}

/// Amend an incident memory entry's title, body or link.
///
/// Requires the caller to be an administrator. Responds 409 if the entry has
/// changed since the revision given.
#[utoipa::path(
	post,
	path = "/amend",
	operation_id = "memory_amend",
	tag = "memory",
	security(("tailscale-admin" = [])),
	request_body = AmendArgs,
	responses(
		(status = 200, body = MemoryEntryData),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, body = ProblemDetailsSchema),
	),
)]
pub async fn amend(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<AmendArgs>,
) -> Result<Json<MemoryEntryData>> {
	let mut conn = audit.begin().await?;
	let outcome = memory::amend(
		&mut conn,
		args.id,
		args.revision,
		Amendment {
			title: args.title,
			body: args.body,
			link: args.link,
		},
		Author::operator(&admin.0.login),
	)
	.await?;
	let changed = matches!(outcome, memory::Outcome::Changed(_));
	let entry = outcome.or_conflict()?;
	if changed {
		let link = Record::memory_link(&mut conn, &entry).await?;
		conn.record(Action::MemoryEntryAmended {
			entry: Record::memory_entry(&entry),
			link,
			revision: entry.revision,
			via: Via::Operator.as_str().to_owned(),
		})
		.await?;
	} else {
		audit.unchanged();
	}
	Ok(Json(present_one(&mut conn, entry).await?))
}

/// Retiring an entry.
#[derive(Deserialize, ToSchema)]
pub struct RetireArgs {
	pub id: Uuid,
	/// Why the entry no longer holds.
	pub reason: String,
}

/// Retire an incident memory entry.
///
/// The entry is kept in full, with its history, and is left out of lists
/// unless retired entries are asked for. Requires the caller to be an
/// administrator.
#[utoipa::path(
	post,
	path = "/retire",
	operation_id = "memory_retire",
	tag = "memory",
	security(("tailscale-admin" = [])),
	request_body = RetireArgs,
	responses(
		(status = 200, body = MemoryEntryData),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, body = ProblemDetailsSchema),
	),
)]
pub async fn retire(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<RetireArgs>,
) -> Result<Json<MemoryEntryData>> {
	let mut conn = audit.begin().await?;
	let entry = memory::retire(
		&mut conn,
		args.id,
		&args.reason,
		Author::operator(&admin.0.login),
	)
	.await?;
	let link = Record::memory_link(&mut conn, &entry).await?;
	conn.record(Action::MemoryEntryRetired {
		entry: Record::memory_entry(&entry),
		link,
		revision: entry.revision,
		via: Via::Operator.as_str().to_owned(),
	})
	.await?;
	Ok(Json(present_one(&mut conn, entry).await?))
}

/// Restoring an entry to an earlier version of itself.
#[derive(Deserialize, ToSchema)]
pub struct RestoreArgs {
	pub id: Uuid,
	/// The revision of the entry the caller read.
	pub revision: i64,
	/// The earlier version to restore.
	pub to_revision: i64,
}

/// Restore an incident memory entry to an earlier version.
///
/// The restoration is a new version, and makes a retired entry current again.
/// Requires the caller to be an administrator. Responds 409 if the entry has
/// changed since the revision given.
#[utoipa::path(
	post,
	path = "/restore",
	operation_id = "memory_restore",
	tag = "memory",
	security(("tailscale-admin" = [])),
	request_body = RestoreArgs,
	responses(
		(status = 200, body = MemoryEntryData),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, body = ProblemDetailsSchema),
	),
)]
pub async fn restore(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<RestoreArgs>,
) -> Result<Json<MemoryEntryData>> {
	let mut conn = audit.begin().await?;
	let outcome = memory::restore(
		&mut conn,
		args.id,
		args.revision,
		args.to_revision,
		Author::operator(&admin.0.login),
	)
	.await?;
	let changed = matches!(outcome, memory::Outcome::Changed(_));
	let entry = outcome.or_conflict()?;
	if changed {
		let link = Record::memory_link(&mut conn, &entry).await?;
		conn.record(Action::MemoryEntryRestored {
			entry: Record::memory_entry(&entry),
			link,
			revision: entry.revision,
			restored_from: args.to_revision,
			via: Via::Operator.as_str().to_owned(),
		})
		.await?;
	} else {
		audit.unchanged();
	}
	Ok(Json(present_one(&mut conn, entry).await?))
}
