use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::{Uuid, server::rank::ServerRank};
use database::issues::Scope;
use database::maintenance_windows::MaintenanceWindow;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

/// How many of a target's ended windows its history returns.
const HISTORY_LIMIT: i64 = 20;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(list_open))
		.routes(routes!(for_target))
		.routes(routes!(declare))
		.routes(routes!(lift))
}

/// The target a window covers: exactly one of the ids is set.
#[derive(Deserialize, ToSchema)]
pub struct TargetArgs {
	/// The application, for a window over one workload. Covers that
	/// application and nothing else on the box it runs on.
	pub application_id: Option<Uuid>,
	/// The machine, for a window over one box. Covers every application on it.
	pub machine_id: Option<Uuid>,
	/// The group, for a window over a whole group or one of its environments.
	pub server_group_id: Option<Uuid>,
}

impl TargetArgs {
	fn scope(&self) -> Result<Scope> {
		match (self.application_id, self.machine_id, self.server_group_id) {
			(Some(id), None, None) => Ok(Scope::Application(id)),
			(None, Some(id), None) => Ok(Scope::Machine(id)),
			(None, None, Some(id)) => Ok(Scope::Group(id)),
			_ => Err(AppError::BadRequest(
				"a maintenance window covers one application, one machine or one group".into(),
			)),
		}
	}
}

/// Declare a window over a target, or amend the one it already has.
#[derive(Deserialize, ToSchema)]
pub struct DeclareArgs {
	/// The application, for a window over one workload. Covers that
	/// application and nothing else on the box it runs on.
	pub application_id: Option<Uuid>,
	/// The machine, for a window over one box. Covers every application on it.
	pub machine_id: Option<Uuid>,
	/// The group, for a window over a whole group or one of its environments.
	pub server_group_id: Option<Uuid>,
	/// With the group, the rank of the environment the window covers: the
	/// machines serving the group's applications at that rank, and nothing else
	/// of the group. Absent for a window over the whole group.
	pub rank: Option<ServerRank>,
	/// When the work is expected to finish. The window ends itself then.
	#[schema(value_type = String, format = DateTime)]
	pub expected_end: Timestamp,
	/// What is being done.
	pub note: Option<String>,
}

/// The window to lift.
#[derive(Deserialize, ToSchema)]
pub struct LiftArgs {
	/// The window, as returned when it was declared or listed.
	pub id: Uuid,
}

/// An open window with the target it covers, named so a fleet-wide view
/// reads without a lookup per row.
#[derive(Serialize, ToSchema)]
pub struct OpenWindow {
	/// The window itself.
	pub window: MaintenanceWindow,
	/// The target as it reads to an operator.
	pub target: String,
}

/// Every maintenance window currently holding, across the fleet.
///
/// Most recently declared first. A window that has ended is history and
/// belongs to its target, so it is not listed here even while its settle
/// period runs.
#[utoipa::path(
	post,
	path = "/list_open",
	tag = "maintenance",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Vec<OpenWindow>),
	),
)]
pub async fn list_open(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(_args): Json<serde_json::Value>,
) -> Result<Json<Vec<OpenWindow>>> {
	let mut conn = state.db.get().await?;
	let windows = MaintenanceWindow::list_open(&mut conn).await?;
	let mut out = Vec::with_capacity(windows.len());
	for window in windows {
		let target =
			database::maintenance_windows::target_label(&mut conn, window.scope(), window.rank)
				.await?;
		out.push(OpenWindow { window, target });
	}
	Ok(Json(out))
}

/// A target's maintenance windows.
///
/// Open and ended, most recently declared first, so what was being done the
/// last time the target went quiet is readable against it.
#[utoipa::path(
	post,
	path = "/for_target",
	tag = "maintenance",
	security(("tailscale-user" = [])),
	request_body = TargetArgs,
	responses(
		(status = 200, body = Vec<MaintenanceWindow>),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn for_target(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<TargetArgs>,
) -> Result<Json<Vec<MaintenanceWindow>>> {
	let mut conn = state.db.get().await?;
	let rows = MaintenanceWindow::list_for_scope(&mut conn, args.scope()?, HISTORY_LIMIT).await?;
	Ok(Json(rows))
}

/// Declare that an application, a machine, a group, or one of a group's
/// environments is being worked on.
///
/// Every check on the target grades to skipped while the window holds and
/// for a settle period after it ends, so nothing on it opens or joins an
/// incident. A window over one application leaves the rest of the box watched;
/// one over the machine covers everything on it. Issues already in an open
/// incident leave it, closing the incident where nothing else holds it open. A
/// target that already has an open window has that window amended rather than
/// a second opened.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/declare",
	tag = "maintenance",
	security(("tailscale-admin" = [])),
	request_body = DeclareArgs,
	responses(
		(status = 200, body = MaintenanceWindow),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn declare(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<DeclareArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = state.db.get().await?;
	let scope = TargetArgs {
		application_id: args.application_id,
		machine_id: args.machine_id,
		server_group_id: args.server_group_id,
	}
	.scope()?;
	let window = MaintenanceWindow::declare(
		&mut conn,
		scope,
		args.rank,
		args.expected_end,
		args.note.as_deref(),
		Some(&admin.0.login),
	)
	.await?;
	Ok(Json(window))
}

/// Lift a window before its expected end.
///
/// Suspension runs on for the settle period, after which the target is
/// watched again. Lifting a window that has already ended changes nothing.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/lift",
	tag = "maintenance",
	security(("tailscale-admin" = [])),
	request_body = LiftArgs,
	responses(
		(status = 200, body = MaintenanceWindow),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn lift(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<LiftArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = state.db.get().await?;
	let window = MaintenanceWindow::lift(&mut conn, args.id, Some(&admin.0.login)).await?;
	Ok(Json(window))
}
