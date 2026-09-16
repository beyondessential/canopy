use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::{Uuid, issue::ResolvedReason, server::rank::ServerRank};
use database::issues::{Incident, IncidentIssue, IncidentStats};
use database::notes::IncidentNote;
use database::server_groups::ServerGroup;
use database::tailscale_users::TailscaleUser as CachedTailscaleUser;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::fns::issues::{IssueData, enrich_issues, lookup_user};
use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 100;

/// An operational incident: a roll-up of the related issues on one target,
/// which is one of a group's environments, the group itself, or canopy.
///
/// An incident opens when an issue on its target crosses the severity
/// threshold, gathers further contributing issues while open, and closes
/// automatically once the last serious contributor clears. Operators can
/// additionally mark an incident resolved with a reason.
// spec: INC#targets
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IncidentData {
	/// Unique identifier of the incident.
	pub id: Uuid,
	/// Identifier of the group the incident belongs to, or null for
	/// a canopy-wide incident (aggregating canopy's self-alerts).
	pub server_group_id: Option<Uuid>,
	/// Display name of the group this incident rolls up to — `Canopy` for
	/// a canopy-wide incident. Empty only if the group no longer exists,
	/// which should not happen in normal operation.
	pub server_group_name: String,
	/// Which of the group's environments the incident targets. Null with a
	/// group means the group itself: its own checks, and the members of a
	/// group with no ranked application.
	pub rank: Option<ServerRank>,
	/// When the incident opened.
	pub opened_at: Timestamp,
	/// When the incident closed (all serious contributing issues cleared);
	/// null while the incident is still open.
	pub closed_at: Option<Timestamp>,
	/// When an operator marked the incident resolved; null if it has not
	/// been resolved.
	pub resolved_at: Option<Timestamp>,
	/// Login of the operator who resolved the incident, if resolved.
	pub resolved_by: Option<String>,
	/// Display name of the resolving operator, if known.
	pub resolved_by_name: Option<String>,
	/// Avatar URL of the resolving operator, if known.
	pub resolved_by_pic: Option<String>,
	/// Reason given when resolving: one of `fixed`, `wont_fix`, `expected`,
	/// `duplicate`, or `flapping`.
	pub resolved_reason: Option<String>,
	/// Number of distinct issues that have ever contributed to the incident.
	pub issue_count: i64,
	/// Combined count of notes on the incident itself plus notes on all its
	/// contributing issues.
	pub note_count: i64,
	/// When set, the "incident opened" Slack notification is being held
	/// until this time by the group's notification delay (which suppresses
	/// flapping); an incident that resolves before then never notifies.
	/// Null once the notification has been sent, cancelled, or given up on.
	/// Lets a client distinguish "open but quietly held" from "open and
	/// operators have been notified".
	pub notification_held_until: Option<Timestamp>,
	/// When set, the incident is lingering: its last effective failure
	/// recovered at this time, and the incident stays open for the group's
	/// linger window in case the failure comes back (which would continue
	/// this incident rather than open a new one). Null while a failure is
	/// live, and always null on a closed incident. Lets a client
	/// distinguish "open and failing" from "open but currently recovered".
	pub lingering_since: Option<Timestamp>,
	/// When the incident record was created.
	pub created_at: Timestamp,
	/// When the incident record was last modified.
	pub updated_at: Timestamp,
}

impl IncidentData {
	fn from_with(
		i: Incident,
		server_group_name: String,
		users: &std::collections::HashMap<String, CachedTailscaleUser>,
		stats: IncidentStats,
		notification_held_until: Option<Timestamp>,
	) -> Self {
		let (res_name, res_pic) = lookup_user(users, i.resolved_by.as_deref());
		let lingering_since = if i.closed_at.is_none() {
			i.closing_at
		} else {
			None
		};
		Self {
			id: i.id,
			server_group_id: i.server_group_id,
			server_group_name,
			rank: i.rank,
			opened_at: i.opened_at,
			closed_at: i.closed_at,
			resolved_at: i.resolved_at,
			resolved_by: i.resolved_by,
			resolved_by_name: res_name,
			resolved_by_pic: res_pic,
			resolved_reason: i.resolved_reason,
			issue_count: stats.issue_count,
			note_count: stats.note_count,
			notification_held_until,
			lingering_since,
			created_at: i.created_at,
			updated_at: i.updated_at,
		}
	}
}

fn collect_incident_user_logins(incidents: &[Incident]) -> Vec<&str> {
	let mut s: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
	for i in incidents {
		if let Some(l) = i.resolved_by.as_deref() {
			s.insert(l);
		}
	}
	s.into_iter().collect()
}

async fn enrich_incidents(
	conn: &mut database::diesel_async::AsyncPgConnection,
	pool: &database::Db,
	incidents: Vec<Incident>,
) -> Result<Vec<IncidentData>> {
	let group_ids: Vec<Uuid> = incidents.iter().filter_map(|i| i.server_group_id).collect();
	let incident_ids: Vec<Uuid> = incidents.iter().map(|i| i.id).collect();
	let groups = ServerGroup::list_by_ids(conn, &group_ids).await?;
	let group_names: std::collections::HashMap<Uuid, String> =
		groups.into_iter().map(|g| (g.id, g.name)).collect();
	let user_logins = collect_incident_user_logins(&incidents);
	let users = CachedTailscaleUser::by_logins(conn, &user_logins).await?;
	let stats = Incident::stats_for(pool, &incident_ids).await?;
	let held =
		database::slack_outbox::SlackOutbox::pending_opens_until(conn, &incident_ids).await?;
	Ok(incidents
		.into_iter()
		.map(|i| {
			let name = match i.server_group_id {
				Some(gid) => group_names.get(&gid).cloned().unwrap_or_default(),
				None => "Canopy".to_string(),
			};
			let s = stats.get(&i.id).copied().unwrap_or_default();
			let held_until = held.get(&i.id).copied();
			IncidentData::from_with(i, name, &users, s, held_until)
		})
		.collect())
}

async fn enrich_incident(
	conn: &mut database::diesel_async::AsyncPgConnection,
	pool: &database::Db,
	incident: Incident,
) -> Result<IncidentData> {
	let group_name = match incident.server_group_id {
		Some(gid) => ServerGroup::get_by_id(conn, gid).await?.name,
		None => "Canopy".to_string(),
	};
	let user_logins = collect_incident_user_logins(std::slice::from_ref(&incident));
	let users = CachedTailscaleUser::by_logins(conn, &user_logins).await?;
	let mut stats = Incident::stats_for(pool, &[incident.id]).await?;
	let s = stats.remove(&incident.id).unwrap_or_default();
	let mut held =
		database::slack_outbox::SlackOutbox::pending_opens_until(conn, &[incident.id]).await?;
	let held_until = held.remove(&incident.id);
	Ok(IncidentData::from_with(
		incident, group_name, &users, s, held_until,
	))
}

/// One issue's involvement in an incident.
///
/// An issue joins an incident while it is actively contributing and leaves
/// when it stops (for example when it is resolved, snoozed, or silenced);
/// the same issue can join, leave, and rejoin over the incident's life.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IncidentIssueData {
	/// When the issue joined (started contributing to) the incident.
	pub joined_at: Timestamp,
	/// When the issue left (stopped contributing to) the incident; null
	/// while it is still contributing.
	pub left_at: Option<Timestamp>,
	/// The issue itself.
	pub issue: IssueData,
}

impl From<(IncidentIssue, IssueData)> for IncidentIssueData {
	fn from((link, issue): (IncidentIssue, IssueData)) -> Self {
		Self {
			joined_at: link.joined_at,
			left_at: link.left_at,
			issue,
		}
	}
}

/// An incident together with every issue that has contributed to it.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IncidentWithIssues {
	/// The incident.
	pub incident: IncidentData,
	/// The issues that have contributed to the incident, each with the times
	/// it joined and left.
	pub issues: Vec<IncidentIssueData>,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(list_for_server))
		.routes(routes!(list_for_group))
		.routes(routes!(list_active))
		.routes(routes!(get_incident))
		.routes(routes!(resolve))
		.routes(routes!(unresolve))
		.routes(routes!(add_note))
		.routes(routes!(list_notes))
		.routes(routes!(delete_note))
}

/// Filters for listing incidents that involve a server.
#[derive(Deserialize, ToSchema)]
pub struct IncidentListForServerArgs {
	/// Identifier of the server.
	pub server_id: Uuid,
	/// Also include closed incidents; defaults to false (open incidents only).
	#[serde(default)]
	pub include_closed: Option<bool>,
	/// Maximum number of incidents to return; defaults to 100.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List incidents involving a server.
///
/// Returns the incidents on the environment the application is in, and the
/// group's own where it belongs to no environment. By default only open
/// incidents are returned; set `include_closed` to also include closed ones.
#[utoipa::path(
	post,
	path = "/list_for_server",
	operation_id = "incident_list_for_server",
	tag = "incidents",
	security(("tailscale-user" = [])),
	request_body = IncidentListForServerArgs,
	responses(
		(status = 200, body = Vec<IncidentData>),
	),
)]
pub async fn list_for_server(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<IncidentListForServerArgs>,
) -> Result<Json<Vec<IncidentData>>> {
	let mut conn = state.db_read.get().await?;
	let incidents = Incident::list_for_server(
		&mut conn,
		args.server_id,
		args.include_closed.unwrap_or(false),
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(
		enrich_incidents(&mut conn, &state.db_read, incidents).await?,
	))
}

/// Filters for listing a server group's incidents.
#[derive(Deserialize, ToSchema)]
pub struct ListForGroupArgs {
	/// Identifier of the server group.
	pub server_group_id: Uuid,
	/// Also include closed incidents; defaults to false (open incidents only).
	#[serde(default)]
	pub include_closed: Option<bool>,
	/// Maximum number of incidents to return; defaults to 100.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List incidents for a server group.
///
/// Returns the group's own incidents and those of every environment in it. By
/// default only open incidents are returned; set `include_closed` to also
/// include closed ones.
#[utoipa::path(
	post,
	path = "/list_for_group",
	operation_id = "incident_list_for_group",
	tag = "incidents",
	security(("tailscale-user" = [])),
	request_body = ListForGroupArgs,
	responses(
		(status = 200, body = Vec<IncidentData>),
	),
)]
pub async fn list_for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ListForGroupArgs>,
) -> Result<Json<Vec<IncidentData>>> {
	let mut conn = state.db_read.get().await?;
	let incidents = Incident::list_for_group(
		&mut conn,
		args.server_group_id,
		args.include_closed.unwrap_or(false),
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(
		enrich_incidents(&mut conn, &state.db_read, incidents).await?,
	))
}

/// Filters for listing open incidents.
#[derive(Deserialize, ToSchema)]
pub struct ListActiveArgs {
	/// Maximum number of incidents to return; defaults to 100.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List open incidents across all server groups.
///
/// Returns every incident that is currently open, fleet-wide.
#[utoipa::path(
	post,
	path = "/list_active",
	tag = "incidents",
	security(("tailscale-user" = [])),
	request_body = ListActiveArgs,
	responses(
		(status = 200, body = Vec<IncidentData>),
	),
)]
pub async fn list_active(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ListActiveArgs>,
) -> Result<Json<Vec<IncidentData>>> {
	let mut conn = state.db_read.get().await?;
	let incidents = Incident::list_active(&mut conn, args.limit.unwrap_or(DEFAULT_LIMIT)).await?;
	Ok(Json(
		enrich_incidents(&mut conn, &state.db_read, incidents).await?,
	))
}

/// Identifies the incident to fetch.
#[derive(Deserialize, ToSchema)]
pub struct GetIncidentArgs {
	/// Identifier of the incident.
	pub incident_id: Uuid,
}

/// Get an incident with its contributing issues.
///
/// Returns the incident and every issue that has contributed to it, each
/// with the times it joined and (where applicable) left the incident.
/// Responds 404 if the incident does not exist.
#[utoipa::path(
	post,
	path = "/get",
	tag = "incidents",
	security(("tailscale-user" = [])),
	request_body = GetIncidentArgs,
	responses(
		(status = 200, body = IncidentWithIssues),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get_incident(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<GetIncidentArgs>,
) -> Result<Json<IncidentWithIssues>> {
	let mut conn = state.db_read.get().await?;
	let (incident, rows) = Incident::get_with_issues(&mut conn, args.incident_id).await?;
	let (links, raw_issues): (Vec<_>, Vec<_>) = rows.into_iter().unzip();
	let issue_data = enrich_issues(&mut conn, raw_issues).await?;
	let issues = links
		.into_iter()
		.zip(issue_data)
		.map(|(link, issue)| IncidentIssueData::from((link, issue)))
		.collect();
	let incident = enrich_incident(&mut conn, &state.db_read, incident).await?;
	Ok(Json(IncidentWithIssues { incident, issues }))
}

/// Identifies the incident to operate on.
#[derive(Deserialize, ToSchema)]
pub struct IncidentIdArgs {
	/// Identifier of the incident.
	pub incident_id: Uuid,
}

/// Request to mark an incident resolved.
#[derive(Deserialize, ToSchema)]
pub struct ResolveIncidentArgs {
	/// Identifier of the incident to resolve.
	pub incident_id: Uuid,
	/// Why the incident is considered resolved.
	pub reason: ResolvedReason,
}

/// Resolve an incident.
///
/// Marks the incident resolved with the given reason, recording the calling
/// operator as the resolver. Returns the updated incident. Requires the
/// caller to be on the admin allow-list.
#[utoipa::path(
	post,
	path = "/resolve",
	operation_id = "incident_resolve",
	tag = "incidents",
	security(("tailscale-admin" = [])),
	request_body = ResolveIncidentArgs,
	responses(
		(status = 200, body = IncidentData),
	),
)]
pub async fn resolve(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<ResolveIncidentArgs>,
) -> Result<Json<IncidentData>> {
	let mut conn = state.db.get().await?;
	let incident =
		Incident::resolve(&mut conn, args.incident_id, &admin.0.login, args.reason).await?;
	Ok(Json(enrich_incident(&mut conn, &state.db, incident).await?))
}

/// Undo an incident's resolution.
///
/// Clears the incident's resolved state (time, resolver, and reason).
/// Returns the updated incident. Requires the caller to be on the admin
/// allow-list.
#[utoipa::path(
	post,
	path = "/unresolve",
	operation_id = "incident_unresolve",
	tag = "incidents",
	security(("tailscale-admin" = [])),
	request_body = IncidentIdArgs,
	responses(
		(status = 200, body = IncidentData),
	),
)]
pub async fn unresolve(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<IncidentIdArgs>,
) -> Result<Json<IncidentData>> {
	let mut conn = state.db.get().await?;
	let incident = Incident::unresolve(&mut conn, args.incident_id).await?;
	Ok(Json(enrich_incident(&mut conn, &state.db, incident).await?))
}

/// A note attached to an incident.
///
/// Notes are immutable once written; to change one, delete it and add a
/// replacement.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IncidentNoteData {
	/// Unique identifier of the note.
	pub id: Uuid,
	/// Identifier of the incident the note is attached to.
	pub incident_id: Uuid,
	/// Login of the operator who wrote the note.
	pub author: String,
	/// The note text.
	pub body: String,
	/// When the note was written.
	pub created_at: Timestamp,
}

impl From<IncidentNote> for IncidentNoteData {
	fn from(n: IncidentNote) -> Self {
		Self {
			id: n.id,
			incident_id: n.incident_id,
			author: n.author,
			body: n.body,
			created_at: n.created_at,
		}
	}
}

/// Request to add a note to an incident.
#[derive(Deserialize, ToSchema)]
pub struct IncidentAddNoteArgs {
	/// Identifier of the incident to attach the note to.
	pub incident_id: Uuid,
	/// The note text; must not be empty or whitespace-only.
	pub body: String,
}

/// Add a note to an incident.
///
/// Records a note authored by the calling operator and returns it. Requires
/// the caller to be on the admin allow-list. Responds 400 if the note body
/// is empty or whitespace-only.
#[utoipa::path(
	post,
	path = "/add_note",
	operation_id = "incident_add_note",
	tag = "incidents",
	security(("tailscale-admin" = [])),
	request_body = IncidentAddNoteArgs,
	responses(
		(status = 200, body = IncidentNoteData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn add_note(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<IncidentAddNoteArgs>,
) -> Result<Json<IncidentNoteData>> {
	if args.body.trim().is_empty() {
		return Err(AppError::BadRequest("note body is required".into()));
	}
	let mut conn = state.db.get().await?;
	let note = IncidentNote::add(&mut conn, args.incident_id, &admin.0.login, &args.body).await?;
	Ok(Json(IncidentNoteData::from(note)))
}

/// Filters for listing an incident's notes.
#[derive(Deserialize, ToSchema)]
pub struct IncidentListNotesArgs {
	/// Identifier of the incident.
	pub incident_id: Uuid,
	/// Maximum number of notes to return; defaults to 100.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List notes on an incident.
///
/// Returns notes written on the incident itself; notes on its contributing
/// issues are not included.
#[utoipa::path(
	post,
	path = "/list_notes",
	operation_id = "incident_list_notes",
	tag = "incidents",
	security(("tailscale-user" = [])),
	request_body = IncidentListNotesArgs,
	responses(
		(status = 200, body = Vec<IncidentNoteData>),
	),
)]
pub async fn list_notes(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<IncidentListNotesArgs>,
) -> Result<Json<Vec<IncidentNoteData>>> {
	let mut conn = state.db_read.get().await?;
	let notes = IncidentNote::list_for_incident(
		&mut conn,
		args.incident_id,
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(
		notes.into_iter().map(IncidentNoteData::from).collect(),
	))
}

/// Identifies the note to delete.
#[derive(Deserialize, ToSchema)]
pub struct IncidentDeleteNoteArgs {
	/// Identifier of the note.
	pub note_id: Uuid,
}

/// Delete an incident note.
///
/// Permanently removes the note. Notes cannot be edited in place; to change
/// one, delete it and add a replacement. Requires the caller to be on the
/// admin allow-list.
#[utoipa::path(
	post,
	path = "/delete_note",
	operation_id = "incident_delete_note",
	tag = "incidents",
	security(("tailscale-admin" = [])),
	request_body = IncidentDeleteNoteArgs,
	responses(
		(status = 200),
	),
)]
pub async fn delete_note(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<IncidentDeleteNoteArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	IncidentNote::delete(&mut conn, args.note_id).await?;
	Ok(Json(()))
}
