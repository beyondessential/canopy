use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::Uuid;
use commons_types::server::app_type::ApplicationType;
use database::silenced_refs::{
	ClusterSilencedRef, MachineSilencedRef, ServerGroupSilencedRef, ServerSilencedRef,
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list_for_server))
		.routes(routes!(read_only: list_for_servers))
		.routes(routes!(read_only: list_for_machine))
		.routes(routes!(read_only: list_for_group))
		.routes(routes!(danger: silence_server))
		.routes(routes!(write: unsilence_server))
		.routes(routes!(danger: silence_machine))
		.routes(routes!(write: unsilence_machine))
		.routes(routes!(read_only: list_for_cluster))
		.routes(routes!(danger: silence_cluster))
		.routes(routes!(write: unsilence_cluster))
		.routes(routes!(danger: silence_group))
		.routes(routes!(write: unsilence_group))
}

/// Request body identifying a server to look up silences for.
#[derive(Deserialize, ToSchema)]
pub struct ServerScopeArgs {
	/// The server to look up silences for.
	pub server_id: Uuid,
}

/// Request body identifying a machine to look up silences for.
#[derive(Deserialize, ToSchema)]
pub struct MachineScopeArgs {
	/// The machine to look up silences for.
	pub machine_id: Uuid,
}

/// Request body identifying an issue to silence (or unsilence) on a single
/// machine.
#[derive(Deserialize, ToSchema)]
pub struct SilenceMachineArgs {
	/// The machine to silence the issue on.
	pub machine_id: Uuid,
	/// Identifies what raises the issue being silenced — for example a
	/// specific healthcheck.
	pub source: String,
	/// The specific issue identifier within `source` to silence.
	#[serde(rename = "ref")]
	pub r#ref: String,
}

/// Request body identifying a cluster to look up silences for.
#[derive(Deserialize, ToSchema)]
pub struct ClusterScopeArgs {
	/// The cluster to look up silences for.
	pub kubernetes_cluster_id: Uuid,
}

/// Request body identifying an issue to silence (or unsilence) on a single
/// cluster.
#[derive(Deserialize, ToSchema)]
pub struct SilenceClusterArgs {
	/// The cluster to silence the issue on.
	pub kubernetes_cluster_id: Uuid,
	/// Identifies what raises the issue being silenced.
	pub source: String,
	/// The specific issue identifier within `source` to silence.
	#[serde(rename = "ref")]
	pub r#ref: String,
}

/// Request body identifying a server group to look up silences for.
#[derive(Deserialize, ToSchema)]
pub struct GroupScopeArgs {
	/// The server group to look up silences for.
	pub server_group_id: Uuid,
}

/// Request body identifying an issue to silence (or unsilence) on a
/// single server.
#[derive(Deserialize, ToSchema)]
pub struct SilenceServerArgs {
	/// The server to silence the issue on.
	pub server_id: Uuid,
	/// Identifies what raises the issue being silenced — for example a
	/// specific healthcheck or backup pipeline.
	pub source: String,
	/// The specific issue identifier within `source` to silence.
	#[serde(rename = "ref")]
	pub r#ref: String,
}

/// Request body identifying an issue to silence (or unsilence) across a
/// whole server group.
#[derive(Deserialize, ToSchema)]
pub struct SilenceGroupArgs {
	/// The server group to silence the issue on.
	pub server_group_id: Uuid,
	/// Identifies what raises the issue being silenced — for example a
	/// specific healthcheck or backup pipeline.
	pub source: String,
	/// The specific issue identifier within `source` to silence.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// Which application type's check is meant, for a check named by an
	/// application rather than by a box. A group covers several types, so the
	/// caller names the one it is silencing from; `null` is a machine's check
	/// or a curated source's.
	#[serde(default)]
	#[schema(value_type = Option<String>)]
	pub application_type: Option<ApplicationType>,
}

/// List server-scoped silences for a server.
///
/// Returns every (source, ref) pair currently silenced specifically for
/// this server, most recently created first. Doesn't include silences
/// applied at the server's group level.
#[utoipa::path(
	post,
	path = "/list_for_server",
	tag = "silenced_refs",
	security(("tailscale-user" = [])),
	request_body = ServerScopeArgs,
	responses(
		(status = 200, body = Vec<ServerSilencedRef>),
	),
)]
pub async fn list_for_server(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ServerScopeArgs>,
) -> Result<Json<Vec<ServerSilencedRef>>> {
	let mut conn = state.db.get().await?;
	let rows = ServerSilencedRef::list_for_server(&mut conn, args.server_id).await?;
	Ok(Json(rows))
}

/// Identifies several applications whose silences to list.
#[derive(Deserialize, ToSchema)]
pub struct ApplicationsScopeArgs {
	/// The applications to read. An empty list reads nothing.
	pub application_ids: Vec<Uuid>,
}

/// List application-scoped silences across several applications.
///
/// The plural of `list_for_server`, for a surface holding a section per
/// application on one machine. Doesn't include silences applied at the group
/// or machine level.
#[utoipa::path(
	post,
	path = "/list_for_servers",
	tag = "silenced_refs",
	security(("tailscale-user" = [])),
	request_body = ApplicationsScopeArgs,
	responses(
		(status = 200, body = Vec<ServerSilencedRef>),
	),
)]
pub async fn list_for_servers(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ApplicationsScopeArgs>,
) -> Result<Json<Vec<ServerSilencedRef>>> {
	let mut conn = state.db.get().await?;
	let rows = ServerSilencedRef::list_for_servers(&mut conn, &args.application_ids).await?;
	Ok(Json(rows))
}

/// List group-scoped silences for a server group.
///
/// Returns every (source, ref) pair currently silenced for this server
/// group — applying to every server in the group — most recently created
/// first.
#[utoipa::path(
	post,
	path = "/list_for_group",
	tag = "silenced_refs",
	security(("tailscale-user" = [])),
	request_body = GroupScopeArgs,
	responses(
		(status = 200, body = Vec<ServerGroupSilencedRef>),
	),
)]
pub async fn list_for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<GroupScopeArgs>,
) -> Result<Json<Vec<ServerGroupSilencedRef>>> {
	let mut conn = state.db.get().await?;
	let rows = ServerGroupSilencedRef::list_for_group(&mut conn, args.server_group_id).await?;
	Ok(Json(rows))
}

/// Silence an issue on a server.
///
/// Suppresses alerting for the given (source, ref) pair on this server:
/// matching issues keep being recorded, but stop counting toward opening
/// or extending an incident. Idempotent — silencing a pair that's already
/// silenced leaves the original entry, including who created it and when,
/// unchanged. Requires admin access. Returns 400 if the request is
/// invalid, for example if it references a server that doesn't exist.
#[utoipa::path(
	post,
	path = "/silence_server",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceServerArgs,
	responses(
		(status = 200, body = ServerSilencedRef),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn silence_server(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<SilenceServerArgs>,
) -> Result<Json<ServerSilencedRef>> {
	let mut conn = state.db.get().await?;
	let row = ServerSilencedRef::add(
		&mut conn,
		args.server_id,
		&args.source,
		&args.r#ref,
		Some(&admin.0.login),
	)
	.await?;
	Ok(Json(row))
}

/// Unsilence an issue on a server.
///
/// Removes a server-scoped silence for the given (source, ref) pair, if
/// one exists. Removing a silence that isn't there is not an error.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/unsilence_server",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceServerArgs,
	responses(
		(status = 200),
	),
)]
pub async fn unsilence_server(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SilenceServerArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	ServerSilencedRef::remove(&mut conn, args.server_id, &args.source, &args.r#ref).await?;
	Ok(Json(()))
}

/// Silence an issue on a server group.
///
/// Suppresses alerting for the given (source, ref) pair across every
/// server in this group: matching issues keep being recorded, but stop
/// counting toward opening or extending an incident. Idempotent —
/// silencing a pair that's already silenced leaves the original entry,
/// including who created it and when, unchanged. Requires admin access.
/// Returns 400 if the request is invalid, for example if it references a
/// server group that doesn't exist.
#[utoipa::path(
	post,
	path = "/silence_group",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceGroupArgs,
	responses(
		(status = 200, body = ServerGroupSilencedRef),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn silence_group(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<SilenceGroupArgs>,
) -> Result<Json<ServerGroupSilencedRef>> {
	let mut conn = state.db.get().await?;
	let row = ServerGroupSilencedRef::add(
		&mut conn,
		args.server_group_id,
		&args.source,
		&args.r#ref,
		args.application_type.as_ref(),
		Some(&admin.0.login),
	)
	.await?;
	Ok(Json(row))
}

/// Unsilence an issue on a server group.
///
/// Removes a group-scoped silence for the given (source, ref) pair, if
/// one exists. Removing a silence that isn't there is not an error.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/unsilence_group",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceGroupArgs,
	responses(
		(status = 200),
	),
)]
pub async fn unsilence_group(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SilenceGroupArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	ServerGroupSilencedRef::remove(
		&mut conn,
		args.server_group_id,
		&args.source,
		&args.r#ref,
		args.application_type.as_ref(),
	)
	.await?;
	Ok(Json(()))
}

/// List machine-scoped silences for a machine.
///
/// Returns every (source, ref) pair currently silenced specifically for this
/// box, most recently created first. Doesn't include silences applied at the
/// machine's group level, nor those on the applications running on it: an
/// application's checks are silenced against the application.
#[utoipa::path(
	post,
	path = "/list_for_machine",
	tag = "silenced_refs",
	security(("tailscale-user" = [])),
	request_body = MachineScopeArgs,
	responses(
		(status = 200, body = Vec<MachineSilencedRef>),
	),
)]
pub async fn list_for_machine(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<MachineScopeArgs>,
) -> Result<Json<Vec<MachineSilencedRef>>> {
	let mut conn = state.db.get().await?;
	let rows = MachineSilencedRef::list_for_machine(&mut conn, args.machine_id).await?;
	Ok(Json(rows))
}

/// Silence an issue on a machine.
///
/// Suppresses alerting for the given (source, ref) pair on this box: matching
/// issues keep being recorded, but stop counting toward opening or extending
/// an incident. Idempotent. Requires admin access.
#[utoipa::path(
	post,
	path = "/silence_machine",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceMachineArgs,
	responses(
		(status = 200, body = MachineSilencedRef),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn silence_machine(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<SilenceMachineArgs>,
) -> Result<Json<MachineSilencedRef>> {
	let mut conn = state.db.get().await?;
	let row = MachineSilencedRef::add(
		&mut conn,
		args.machine_id,
		&args.source,
		&args.r#ref,
		Some(&admin.0.login),
	)
	.await?;
	Ok(Json(row))
}

/// Unsilence an issue on a machine.
///
/// Removes a machine-scoped silence for the given (source, ref) pair, if one
/// exists. Removing a silence that isn't there is not an error. Requires admin
/// access.
#[utoipa::path(
	post,
	path = "/unsilence_machine",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceMachineArgs,
	responses(
		(status = 204, description = "Silence removed, or there was none."),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn unsilence_machine(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SilenceMachineArgs>,
) -> Result<axum::http::StatusCode> {
	let mut conn = state.db.get().await?;
	MachineSilencedRef::remove(&mut conn, args.machine_id, &args.source, &args.r#ref).await?;
	Ok(axum::http::StatusCode::NO_CONTENT)
}

/// List cluster-scoped silences for a cluster.
///
/// Returns every (source, ref) pair currently silenced for this cluster, most
/// recently created first. A cluster belongs to no group, so these are the
/// only silences bearing on its checks.
#[utoipa::path(
	post,
	path = "/list_for_cluster",
	tag = "silenced_refs",
	security(("tailscale-user" = [])),
	request_body = ClusterScopeArgs,
	responses(
		(status = 200, body = Vec<ClusterSilencedRef>),
	),
)]
pub async fn list_for_cluster(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ClusterScopeArgs>,
) -> Result<Json<Vec<ClusterSilencedRef>>> {
	let mut conn = state.db.get().await?;
	let rows = ClusterSilencedRef::list_for_cluster(&mut conn, args.kubernetes_cluster_id).await?;
	Ok(Json(rows))
}

/// Silence an issue on a cluster.
///
/// The matching check keeps being recorded but presents as skipped and leaves
/// the cluster's health. Idempotent. Requires admin access.
#[utoipa::path(
	post,
	path = "/silence_cluster",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceClusterArgs,
	responses(
		(status = 200, body = ClusterSilencedRef),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn silence_cluster(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<SilenceClusterArgs>,
) -> Result<Json<ClusterSilencedRef>> {
	let mut conn = state.db.get().await?;
	let row = ClusterSilencedRef::add(
		&mut conn,
		args.kubernetes_cluster_id,
		&args.source,
		&args.r#ref,
		Some(&admin.0.login),
	)
	.await?;
	Ok(Json(row))
}

/// Unsilence an issue on a cluster.
///
/// Removes a cluster-scoped silence for the given (source, ref) pair, if one
/// exists. Removing a silence that isn't there is not an error. Requires admin
/// access.
#[utoipa::path(
	post,
	path = "/unsilence_cluster",
	tag = "silenced_refs",
	security(("tailscale-admin" = [])),
	request_body = SilenceClusterArgs,
	responses(
		(status = 204, description = "Silence removed, or there was none."),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn unsilence_cluster(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SilenceClusterArgs>,
) -> Result<axum::http::StatusCode> {
	let mut conn = state.db.get().await?;
	ClusterSilencedRef::remove(
		&mut conn,
		args.kubernetes_cluster_id,
		&args.source,
		&args.r#ref,
	)
	.await?;
	Ok(axum::http::StatusCode::NO_CONTENT)
}
