use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::{
	Uuid,
	issue::ResolvedReason,
	namespace::{Namespace, NamespaceRef},
	status::CheckResult,
};
use database::applications::Application;
use database::issues::{
	CheckFiling, Incident, Issue, IssueFilter, IssueIncidentRef, IssueListFilters, MANUAL_SOURCE,
	Scope, file_check,
};
use database::notes::IssueNote;
use database::tailscale_users::TailscaleUser as CachedTailscaleUser;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

const DEFAULT_LIMIT: i64 = 100;

/// A problem raised against a server (or a group of applications), tracking its
/// current severity, whether it's still ongoing, and how it's been handled
/// by an operator.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IssueData {
	/// Unique identifier for this issue.
	pub id: Uuid,
	/// The application this issue was raised against. Absent for an issue
	/// filed at any other grain — a machine's, a group's, or Canopy's own.
	pub application_id: Option<Uuid>,
	/// The machine this issue was raised against, for one filed at machine
	/// scope. Exactly one of this and `application_id` is set on a
	/// target-scoped issue; both are absent on a group or Canopy-wide one.
	// spec: MCP#incidents-and-issues
	pub machine_id: Option<Uuid>,
	/// Display name of the affected machine, when one is set and the issue is
	/// the machine's.
	pub machine_name: Option<String>,
	/// Display name of the affected server, when one is set. Falls back to
	/// `server_host` when absent.
	pub server_name: Option<String>,
	/// Hostname or address of the affected server.
	pub server_host: String,
	/// Id of the group the affected server belongs to, if any.
	pub server_group_id: Option<Uuid>,
	/// Display name of the group the affected server belongs to. Absent
	/// when the server isn't in a group.
	pub server_group_name: Option<String>,
	/// Id of the device that reported the underlying event, if the issue
	/// originated from a device push rather than a manual entry.
	pub device_id: Option<Uuid>,
	/// What raised the issue (for example, an automated health check or a
	/// manually submitted event).
	pub source: String,
	/// Which catalog entry the check behind this issue resolves to, when the
	/// issue is a check's. Absent for an issue that is not a check's, and for
	/// one whose namespace cannot be derived — a structured source's
	/// application-subject check filed at a grain with no application type.
	pub namespace: Option<NamespaceRef>,
	/// How the check behind this issue reads to an operator: `<type>.<check>`
	/// where it is one application type's, the bare name otherwise. Absent
	/// alongside `namespace`.
	pub qualified_name: Option<String>,
	/// Identifier used to match new incoming events to this issue; unique
	/// within its source and server.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// Whether the check's policy escalates: an effective failure notifies
	/// immediately, bypassing incident grace.
	pub escalates: bool,
	/// Short headline describing the issue, if one was given.
	pub description: Option<String>,
	/// Latest human-readable message describing the issue's state.
	pub message: String,
	/// Whether the underlying condition is still ongoing. `false` once the
	/// condition has stopped recurring.
	pub active: bool,
	/// When the issue was first raised.
	pub first_seen: Timestamp,
	/// When the most recent event for this issue was recorded.
	pub last_seen: Timestamp,
	/// When the issue was resolved by an operator, if it has been.
	pub resolved_at: Option<Timestamp>,
	/// Login of the operator who resolved the issue, if any.
	pub resolved_by: Option<String>,
	/// Display name of the operator who resolved the issue, filled in when
	/// available.
	pub resolved_by_name: Option<String>,
	/// Profile picture URL of the operator who resolved the issue, filled
	/// in when available.
	pub resolved_by_pic: Option<String>,
	/// Reason given when the issue was resolved, if any. Older records may
	/// contain a value that no longer corresponds to a recognized reason.
	pub resolved_reason: Option<String>,
	/// If set, the issue is snoozed and won't demand attention again until
	/// this time.
	pub snoozed_until: Option<Timestamp>,
	/// When this issue record was created.
	pub created_at: Timestamp,
	/// When this issue record was last updated.
	pub updated_at: Timestamp,
	/// The check this issue tracks, when it is check state (health-check
	/// issues). Absent for issues that aren't check results yet.
	pub check_name: Option<String>,
	/// The result the source reported on the latest filing, before policy.
	#[schema(value_type = Option<String>)]
	pub observed_result: Option<CheckResult>,
	/// What policy made of the latest observed result — the result canopy
	/// acts on.
	#[schema(value_type = Option<String>)]
	pub effective_result: Option<CheckResult>,
	/// The check's own fields from the latest report, verbatim.
	pub detail: Option<serde_json::Value>,
	/// Incidents this issue is or was attached to, most recent first. Empty
	/// for issues that never escalated into an incident.
	pub incidents: Vec<IssueIncidentLink>,
}

/// A reference to an incident that a given issue is or was part of, enough
/// to link to that incident and show whether it's still open.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IssueIncidentLink {
	/// Id of the referenced incident.
	pub incident_id: Uuid,
	/// When the incident was opened.
	pub opened_at: Timestamp,
	/// When the incident was closed, if it has been.
	pub closed_at: Option<Timestamp>,
}

impl From<IssueIncidentRef> for IssueIncidentLink {
	fn from(r: IssueIncidentRef) -> Self {
		Self {
			incident_id: r.incident_id,
			opened_at: r.opened_at,
			closed_at: r.closed_at,
		}
	}
}

/// All the non-Issue extras we tuck into an `IssueData`.
struct IssueEnrichment<'a> {
	/// The namespace of the check behind this issue, derived from its source,
	/// name, and its target's application type. Issues carry no namespace
	/// columns of their own: a check-state's namespace is a function of what
	/// it is filed against, and deriving it through the one function is what
	/// keeps it agreeing with the catalog.
	namespace: Option<Namespace>,
	server_name: Option<String>,
	server_host: String,
	machine_name: Option<String>,
	server_group_id: Option<Uuid>,
	server_group_name: Option<String>,
	users: &'a std::collections::HashMap<String, CachedTailscaleUser>,
	incidents: Vec<IssueIncidentLink>,
}

impl IssueData {
	fn from_with(i: Issue, e: IssueEnrichment<'_>) -> Self {
		let (res_name, res_pic) = lookup_user(e.users, i.resolved_by.as_deref());
		Self {
			id: i.id,
			application_id: i.application_id,
			machine_id: i.machine_id,
			machine_name: e.machine_name,
			server_name: e.server_name,
			server_host: e.server_host,
			server_group_id: e.server_group_id,
			server_group_name: e.server_group_name,
			device_id: i.device_id,
			namespace: e.namespace.as_ref().map(NamespaceRef::from),
			qualified_name: e
				.namespace
				.as_ref()
				.zip(i.check_name.as_deref())
				.map(|(ns, check)| ns.qualified_name(check)),
			source: i.source,
			r#ref: i.r#ref,
			escalates: i.escalates,
			description: i.description,
			message: i.message,
			active: i.active,
			first_seen: i.first_seen,
			last_seen: i.last_seen,
			resolved_at: i.resolved_at,
			resolved_by: i.resolved_by,
			resolved_by_name: res_name,
			resolved_by_pic: res_pic,
			resolved_reason: i.resolved_reason,
			snoozed_until: i.snoozed_until,
			created_at: i.created_at,
			updated_at: i.updated_at,
			check_name: i.check_name,
			observed_result: i.observed_result,
			effective_result: i.effective_result,
			detail: i.detail,
			incidents: e.incidents,
		}
	}
}

pub(crate) fn lookup_user(
	users: &std::collections::HashMap<String, CachedTailscaleUser>,
	login: Option<&str>,
) -> (Option<String>, Option<String>) {
	let Some(login) = login else {
		return (None, None);
	};
	match users.get(login) {
		Some(u) => (Some(u.name.clone()), u.profile_pic.clone()),
		None => (None, None),
	}
}

fn collect_user_logins(issues: &[Issue]) -> Vec<&str> {
	let mut s: std::collections::BTreeSet<&str> = std::collections::BTreeSet::new();
	for i in issues {
		if let Some(l) = i.resolved_by.as_deref() {
			s.insert(l);
		}
	}
	s.into_iter().collect()
}

/// Enrich a list of issues with their target's name and group, acker/resolver
/// display info, and the incidents each issue is attached to.
///
/// Both target grains are resolved: a machine-scoped issue names its machine
/// and that machine's group, as an application-scoped one names its
/// application's. Reading only `application_id` left every machine check —
/// maintenance, restore verification, redaction, and the machine-subject
/// checks — displaying with no name, host, or group against it.
// spec: MCP#incidents-and-issues
pub(crate) async fn enrich_issues(
	conn: &mut database::diesel_async::AsyncPgConnection,
	issues: Vec<Issue>,
) -> Result<Vec<IssueData>> {
	let server_ids: Vec<Uuid> = issues.iter().filter_map(|i| i.application_id).collect();
	let machine_ids: Vec<Uuid> = issues.iter().filter_map(|i| i.machine_id).collect();
	let names = Application::names_by_ids(conn, &server_ids).await?;
	let group_refs = Application::group_refs_by_server_ids(conn, &server_ids).await?;
	let machine_names = database::machines::Machine::names_by_ids(conn, &machine_ids).await?;
	let application_types = Application::types_by_id(conn, &server_ids).await?;
	let machine_groups = database::machines::Machine::group_refs_by_ids(conn, &machine_ids).await?;
	let user_logins = collect_user_logins(&issues);
	let users = CachedTailscaleUser::by_logins(conn, &user_logins).await?;
	let issue_ids: Vec<Uuid> = issues.iter().map(|i| i.id).collect();
	let mut incidents = Incident::for_issues(conn, &issue_ids).await?;
	Ok(issues
		.into_iter()
		.map(|i| {
			let (name, host) = i
				.application_id
				.and_then(|sid| names.get(&sid).cloned())
				.unwrap_or((None, None));
			// An application nobody has named reads as its type, the same as
			// it does everywhere else it is presented.
			// spec: FLT#naming
			let name = name.or_else(|| {
				i.application_id
					.and_then(|sid| application_types.get(&sid))
					.map(|t| t.label())
			});
			// A machine's issue answers to its machine's group, so the
			// group is named either way.
			let (group_id, group_name) = match (i.application_id, i.machine_id) {
				(Some(sid), _) => group_refs.get(&sid).cloned().unwrap_or((None, None)),
				(None, Some(mid)) => machine_groups.get(&mid).cloned().unwrap_or((None, None)),
				(None, None) => (None, None),
			};
			let machine_name = i
				.machine_id
				.and_then(|mid| machine_names.get(&mid).cloned())
				.flatten();
			let links = incidents
				.remove(&i.id)
				.unwrap_or_default()
				.into_iter()
				.map(IssueIncidentLink::from)
				.collect();
			// A check's namespace follows its target's application type, the
			// same as it does on ingest, so an issue links to the catalog
			// entry the check actually files into.
			let namespace = i.check_name.as_deref().and_then(|check| {
				Namespace::of(
					&i.source,
					check,
					i.application_id.and_then(|sid| application_types.get(&sid)),
				)
			});
			IssueData::from_with(
				i,
				IssueEnrichment {
					namespace,
					server_name: name,
					server_host: host.unwrap_or_default(),
					machine_name,
					server_group_id: group_id,
					server_group_name: group_name,
					users: &users,
					incidents: links,
				},
			)
		})
		.collect())
}

/// Same as [`enrich_issues`] but for a single issue.
///
/// Delegates rather than repeating the resolution: the two paths drifted apart
/// once already, which is how a machine-scoped issue could come back enriched
/// from one entry point and bare from the other.
pub(crate) async fn enrich_issue(
	conn: &mut database::diesel_async::AsyncPgConnection,
	issue: Issue,
) -> Result<IssueData> {
	enrich_issues(conn, vec![issue])
		.await?
		.pop()
		.ok_or_else(|| AppError::NotFound("issue".into()))
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list))
		.routes(routes!(read_only: list_for_device))
		.routes(routes!(read_only: list_for_server))
		.routes(routes!(write: submit_manual_event))
		.routes(routes!(write: resolve))
		.routes(routes!(write: unresolve))
		.routes(routes!(write: snooze))
		.routes(routes!(write: unsnooze))
		.routes(routes!(write: add_note))
		.routes(routes!(read_only: list_notes))
		.routes(routes!(write: delete_note))
}

fn filter_from(active_only: Option<bool>) -> IssueFilter {
	match active_only {
		Some(false) => IssueFilter::All,
		_ => IssueFilter::ActiveOnly,
	}
}

/// Filters for listing issues across all applications.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct IssueListArgs {
	/// When `false`, include resolved and inactive issues as well as active
	/// ones. Defaults to `true` (active issues only) when omitted.
	#[serde(default)]
	pub active_only: Option<bool>,
	/// Restrict to issues whose latest effective result is one of these.
	/// Omit to include all results.
	#[serde(default)]
	#[schema(value_type = Option<Vec<String>>)]
	pub results: Option<Vec<CheckResult>>,
	/// Restrict to issues whose server belongs to this group.
	#[serde(default)]
	pub server_group_id: Option<Uuid>,
	/// Maximum number of issues to return. Defaults to 100 when omitted.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List issues across all applications, with optional filtering.
///
/// Returns the most relevant issues fleet-wide, matching the given filters.
/// By default only currently active issues are returned; pass
/// `activeOnly: false` to also see resolved and inactive ones.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "issue_list",
	tag = "issues",
	security(("tailscale-user" = [])),
	request_body = IssueListArgs,
	responses(
		(status = 200, body = Vec<IssueData>),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<IssueListArgs>,
) -> Result<Json<Vec<IssueData>>> {
	let mut conn = state.db_read.get().await?;
	let issues = Issue::list(
		&mut conn,
		IssueListFilters {
			active_only: args.active_only.unwrap_or(true),
			results: args.results,
			server_group_id: args.server_group_id,
			application_id: None,
			machine_id: None,
			since: None,
		},
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(enrich_issues(&mut conn, issues).await?))
}

/// Filters for listing the issues raised by one device.
#[derive(Deserialize, ToSchema)]
pub struct ListForDeviceArgs {
	/// Id of the device whose issues to list.
	pub device_id: Uuid,
	/// When `false`, include resolved and inactive issues as well as active
	/// ones. Defaults to `true` (active issues only) when omitted.
	#[serde(default)]
	pub active_only: Option<bool>,
	/// Maximum number of issues to return. Defaults to 100 when omitted.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List issues raised by a specific device.
///
/// Returns the issues that the given device reported, most relevant
/// first. By default only currently active issues are returned; pass
/// `active_only: false` to also see resolved and inactive ones.
#[utoipa::path(
	post,
	path = "/list_for_device",
	tag = "issues",
	security(("tailscale-user" = [])),
	request_body = ListForDeviceArgs,
	responses(
		(status = 200, body = Vec<IssueData>),
	),
)]
pub async fn list_for_device(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ListForDeviceArgs>,
) -> Result<Json<Vec<IssueData>>> {
	let mut conn = state.db_read.get().await?;
	let issues = Issue::list_for_device(
		&mut conn,
		args.device_id,
		filter_from(args.active_only),
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(enrich_issues(&mut conn, issues).await?))
}

/// Filters for listing the issues raised against one server.
#[derive(Deserialize, ToSchema)]
pub struct IssueListForServerArgs {
	/// Id of the server whose issues to list.
	pub application_id: Uuid,
	/// When `false`, include resolved and inactive issues as well as active
	/// ones. Defaults to `true` (active issues only) when omitted.
	#[serde(default)]
	pub active_only: Option<bool>,
	/// Maximum number of issues to return. Defaults to 100 when omitted.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List issues raised against a specific server.
///
/// Returns the issues recorded against the given server, most relevant
/// first. By default only currently active issues are returned; pass
/// `active_only: false` to also see resolved and inactive ones.
#[utoipa::path(
	post,
	path = "/list_for_server",
	operation_id = "issue_list_for_server",
	tag = "issues",
	security(("tailscale-user" = [])),
	request_body = IssueListForServerArgs,
	responses(
		(status = 200, body = Vec<IssueData>),
	),
)]
pub async fn list_for_server(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<IssueListForServerArgs>,
) -> Result<Json<Vec<IssueData>>> {
	let mut conn = state.db_read.get().await?;
	let issues = Issue::list_for_server(
		&mut conn,
		args.application_id,
		filter_from(args.active_only),
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(enrich_issues(&mut conn, issues).await?))
}

/// A manually raised condition to record against a server.
#[derive(Deserialize, ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct SubmitManualEventArgs {
	/// Id of the server the condition applies to.
	pub application_id: Uuid,
	/// Identifier for the underlying condition. Reports with the same
	/// `ref` on the same server update the same issue rather than opening
	/// a new one each time; use a fresh unique value if that deduplication
	/// isn't wanted.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// The condition's result: `failed` (can open an incident) or
	/// `warning` (context only). Defaults to `failed`. `passed` records
	/// the condition as cleared, same as `active: false`.
	#[serde(default)]
	pub result: Option<CheckResult>,
	/// Whether the condition's failures should notify immediately,
	/// bypassing the incident grace period. Only consulted the first time
	/// a `ref` is seen (it seeds the condition's catalog entry); adjust
	/// later from the healthchecks catalog.
	#[serde(default)]
	pub escalates: Option<bool>,
	/// Short, single-line headline for the condition. Must not contain
	/// newlines — use `message` for multi-line detail.
	#[serde(default)]
	pub description: Option<String>,
	/// Human-readable message describing the condition. May be multi-line.
	pub message: String,
	/// Whether the underlying condition is currently active. Defaults to
	/// `true` when omitted; `false` records it as cleared regardless of
	/// `result`.
	#[serde(default)]
	pub active: Option<bool>,
}

/// Manually raise (or clear) a condition against a server.
///
/// Finds or creates an issue keyed by the server and the given `ref`
/// under the `manual` source, grades the chosen result through the
/// condition's catalog policy, and returns the resulting issue. Returns
/// 400 if `ref` is empty or if `description` contains a newline.
#[utoipa::path(
	post,
	path = "/submit_manual_event",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = SubmitManualEventArgs,
	responses(
		(status = 200, body = IssueData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn submit_manual_event(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SubmitManualEventArgs>,
) -> Result<Json<IssueData>> {
	if args.r#ref.trim().is_empty() {
		return Err(AppError::BadRequest("ref is required".into()));
	}

	let observed = if args.active == Some(false) {
		CheckResult::Passed
	} else {
		args.result.unwrap_or(CheckResult::Failed)
	};
	let mut conn = state.db.get().await?;
	let issue = file_check(
		&mut conn,
		CheckFiling {
			source: MANUAL_SOURCE,
			scope: Scope::Application(args.application_id),
			device_id: None,
			check: &args.r#ref,
			observed,
			title: args.description.as_deref(),
			message: &args.message,
			detail: None,
			// The operator's chosen result passes through ungraded; the
			// escalation choice seeds the catalog entry on first sight.
			default_ceiling: CheckResult::Failed,
			default_escalates: args.escalates.unwrap_or(false),
			documentation: None,
		},
	)
	.await?;
	Ok(Json(enrich_issue(&mut conn, issue).await?))
}

/// Identifies a single issue by id.
#[derive(Deserialize, ToSchema)]
pub struct IssueIdArgs {
	/// Id of the issue to act on.
	pub issue_id: Uuid,
}

/// Identifies an issue to resolve, with the reason for resolving it.
#[derive(Deserialize, ToSchema)]
pub struct IssueResolveArgs {
	/// Id of the issue to resolve.
	pub issue_id: Uuid,
	/// Reason the issue is being resolved.
	pub reason: ResolvedReason,
}

/// Mark an issue as resolved.
///
/// Records the calling operator as the resolver along with the given
/// reason, and returns the updated issue.
#[utoipa::path(
	post,
	path = "/resolve",
	operation_id = "issue_resolve",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = IssueResolveArgs,
	responses(
		(status = 200, body = IssueData),
	),
)]
pub async fn resolve(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<IssueResolveArgs>,
) -> Result<Json<IssueData>> {
	let mut conn = state.db.get().await?;
	let issue = Issue::resolve(&mut conn, args.issue_id, &admin.0.login, args.reason).await?;
	Ok(Json(enrich_issue(&mut conn, issue).await?))
}

/// Undo a previous resolution, marking an issue as unresolved again.
///
/// Clears the resolution timestamp, resolver, and reason, and returns the
/// updated issue.
#[utoipa::path(
	post,
	path = "/unresolve",
	operation_id = "issue_unresolve",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = IssueIdArgs,
	responses(
		(status = 200, body = IssueData),
	),
)]
pub async fn unresolve(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<IssueIdArgs>,
) -> Result<Json<IssueData>> {
	let mut conn = state.db.get().await?;
	let issue = Issue::unresolve(&mut conn, args.issue_id).await?;
	Ok(Json(enrich_issue(&mut conn, issue).await?))
}

/// Identifies an issue and the time to snooze it until.
#[derive(Deserialize, ToSchema)]
pub struct SnoozeArgs {
	/// Id of the issue to snooze.
	pub issue_id: Uuid,
	/// Time until which the issue should be snoozed.
	pub until: Timestamp,
}

/// Snooze an issue until a given time.
///
/// A snoozed issue stops demanding attention until the given time, after
/// which it surfaces again on its own. Returns the updated issue.
#[utoipa::path(
	post,
	path = "/snooze",
	operation_id = "issue_snooze",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = SnoozeArgs,
	responses(
		(status = 200, body = IssueData),
	),
)]
pub async fn snooze(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SnoozeArgs>,
) -> Result<Json<IssueData>> {
	let mut conn = state.db.get().await?;
	let issue = Issue::snooze(&mut conn, args.issue_id, args.until).await?;
	Ok(Json(enrich_issue(&mut conn, issue).await?))
}

/// Clear a snooze on an issue.
///
/// Makes a previously snoozed issue demand attention again immediately,
/// without waiting for its snooze time to pass. Returns the updated issue.
#[utoipa::path(
	post,
	path = "/unsnooze",
	operation_id = "issue_unsnooze",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = IssueIdArgs,
	responses(
		(status = 200, body = IssueData),
	),
)]
pub async fn unsnooze(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<IssueIdArgs>,
) -> Result<Json<IssueData>> {
	let mut conn = state.db.get().await?;
	let issue = Issue::unsnooze(&mut conn, args.issue_id).await?;
	Ok(Json(enrich_issue(&mut conn, issue).await?))
}

/// A free-text note left by an operator on an issue, for handoff and
/// context that doesn't belong in the issue's own message.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct IssueNoteData {
	/// Unique identifier for this note.
	pub id: Uuid,
	/// Id of the issue this note is attached to.
	pub issue_id: Uuid,
	/// Login of the operator who wrote the note.
	pub author: String,
	/// Text of the note.
	pub body: String,
	/// When the note was created.
	pub created_at: Timestamp,
}

impl From<IssueNote> for IssueNoteData {
	fn from(n: IssueNote) -> Self {
		Self {
			id: n.id,
			issue_id: n.issue_id,
			author: n.author,
			body: n.body,
			created_at: n.created_at,
		}
	}
}

/// A note to add to an issue.
#[derive(Deserialize, ToSchema)]
pub struct IssueAddNoteArgs {
	/// Id of the issue to attach the note to.
	pub issue_id: Uuid,
	/// Text of the note. Must not be empty or whitespace-only.
	pub body: String,
}

/// Add a note to an issue.
///
/// Records the calling operator as the author. Returns 400 if the note
/// body is empty or whitespace-only.
#[utoipa::path(
	post,
	path = "/add_note",
	operation_id = "issue_add_note",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = IssueAddNoteArgs,
	responses(
		(status = 200, body = IssueNoteData),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn add_note(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<IssueAddNoteArgs>,
) -> Result<Json<IssueNoteData>> {
	if args.body.trim().is_empty() {
		return Err(AppError::BadRequest("note body is required".into()));
	}
	let mut conn = state.db.get().await?;
	let note = IssueNote::add(&mut conn, args.issue_id, &admin.0.login, &args.body).await?;
	Ok(Json(IssueNoteData::from(note)))
}

/// Identifies the issue whose notes to list.
#[derive(Deserialize, ToSchema)]
pub struct IssueListNotesArgs {
	/// Id of the issue whose notes to list.
	pub issue_id: Uuid,
	/// Maximum number of notes to return. Defaults to 100 when omitted.
	#[serde(default)]
	pub limit: Option<i64>,
}

/// List the notes left on a specific issue.
///
/// Returns the issue's notes, most recent first, each with its author and
/// creation time.
#[utoipa::path(
	post,
	path = "/list_notes",
	operation_id = "issue_list_notes",
	tag = "issues",
	security(("tailscale-user" = [])),
	request_body = IssueListNotesArgs,
	responses(
		(status = 200, body = Vec<IssueNoteData>),
	),
)]
pub async fn list_notes(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<IssueListNotesArgs>,
) -> Result<Json<Vec<IssueNoteData>>> {
	let mut conn = state.db_read.get().await?;
	let notes = IssueNote::list_for_issue(
		&mut conn,
		args.issue_id,
		args.limit.unwrap_or(DEFAULT_LIMIT),
	)
	.await?;
	Ok(Json(notes.into_iter().map(IssueNoteData::from).collect()))
}

/// Identifies a single note to delete from an issue.
#[derive(Deserialize, ToSchema)]
pub struct IssueDeleteNoteArgs {
	/// Id of the note to delete.
	pub note_id: Uuid,
}

/// Permanently delete a note from an issue.
///
/// The note is removed outright; there is no undo. Deleting a note that
/// doesn't exist succeeds without effect.
#[utoipa::path(
	post,
	path = "/delete_note",
	operation_id = "issue_delete_note",
	tag = "issues",
	security(("tailscale-admin" = [])),
	request_body = IssueDeleteNoteArgs,
	responses(
		(status = 200),
	),
)]
pub async fn delete_note(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<IssueDeleteNoteArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	IssueNote::delete(&mut conn, args.note_id).await?;
	Ok(Json(()))
}
