//! Reading the audit log from the operator interface.
//!
//! Spec: `.workhorse/specs/platform/audit-log.md` (id `AUD`), "Reading the log".

use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::Uuid;
use database::audit::{self, Actor, Cursor, Entry, Filter, READ_WINDOW, Tier};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

/// How many entries a page of the log holds, absent a limit of the caller's own.
const PAGE: i64 = 50;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: list))
		.routes(routes!(read_only, silent: filters))
}

/// Which entries to list. Everything is optional; the log page reads the last
/// ninety days of everything, newest first.
#[derive(Deserialize, ToSchema)]
pub struct AuditListArgs {
	/// Entries by this kind of actor: `operator`, `device`, `token` or `canopy`.
	pub actor_kind: Option<String>,
	/// Entries by this actor: a login, a device or token identifier, or a job.
	pub actor_id: Option<String>,
	/// Entries of this action, by its stable name. Asking for an action by name
	/// shows it whether or not it is routine.
	pub action: Option<String>,
	/// Include routine actions: those taken at high volume, which are left out
	/// of the log by default.
	#[serde(default)]
	pub include_routine: bool,
	/// Entries that acted on a record with this identifier, or whose record's
	/// name contains this.
	pub record: Option<String>,
	/// Entries from this time on. Never reaches back further than ninety days.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub since: Option<Timestamp>,
	/// The last entry of the page before, to read the next: its time and its
	/// identifier, both of which place an entry in the log's order.
	pub after: Option<AuditCursor>,
	/// How many entries to return, at most five hundred.
	pub limit: Option<i64>,
}

/// Where a page of the log ended.
#[derive(Deserialize, ToSchema)]
pub struct AuditCursor {
	/// When the last entry of the page was recorded.
	#[schema(value_type = String, format = DateTime)]
	pub recorded_at: Timestamp,
	/// The last entry's identifier.
	pub id: Uuid,
}

/// List the audit log, newest first.
///
/// Reads the database alone, so only the last ninety days. A pending entry is
/// left out while it can still be settled.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "audit_list",
	tag = "audit",
	security(("tailscale-admin" = [])),
	request_body = AuditListArgs,
	responses(
		(status = 200, body = Vec<Entry>),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<AuditListArgs>,
) -> Result<Json<Vec<Entry>>> {
	let mut conn = state.db_read.get().await?;
	let oldest = Timestamp::now() - READ_WINDOW;
	let mut entries = audit::find(
		&mut conn,
		&Filter {
			actor_kind: args.actor_kind,
			actor_id: args.actor_id,
			action: args.action,
			include_routine: args.include_routine,
			acted_on: args.record,
			since: Some(args.since.map_or(oldest, |since| since.max(oldest))),
			after: args.after.map(|after| Cursor {
				recorded_at: after.recorded_at,
				id: after.id,
			}),
			limit: args.limit.unwrap_or(PAGE),
			..Default::default()
		},
	)
	.await?;
	audit::with_pictures(&mut conn, entries.iter_mut().map(|entry| &mut entry.actor)).await?;
	Ok(Json(entries))
}

/// One action a log can be filtered by.
#[derive(Serialize, ToSchema)]
pub struct AuditActionOption {
	/// The action's stable name.
	pub name: String,
	/// What it is called to an operator.
	pub label: String,
	/// Whether the log leaves it out by default.
	pub tier: Tier,
}

/// What the log's filters offer.
#[derive(Serialize, ToSchema)]
pub struct AuditFilters {
	/// Every action in the catalogue.
	pub actions: Vec<AuditActionOption>,
	/// Everyone who acted in the last ninety days.
	pub actors: Vec<Actor>,
}

/// What the log can be filtered by.
///
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/filters",
	operation_id = "audit_filters",
	tag = "audit",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, body = AuditFilters),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn filters(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(_args): Json<serde_json::Value>,
) -> Result<Json<AuditFilters>> {
	let mut conn = state.db_read.get().await?;
	let mut actions: Vec<AuditActionOption> = audit::catalogue()
		.into_iter()
		.map(|(name, label, tier)| AuditActionOption {
			name: name.to_owned(),
			label,
			tier,
		})
		.collect();
	actions.sort_by(|a, b| a.label.cmp(&b.label));
	let actors = audit::actors(&mut conn, Timestamp::now() - READ_WINDOW).await?;
	Ok(Json(AuditFilters { actions, actors }))
}
