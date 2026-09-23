use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list))
		.routes(routes!(danger: add))
		.routes(routes!(danger: delete))
		.routes(routes!(danger: set_danger))
}

/// One entry on the allow-list and the permissions it carries.
#[derive(Serialize, ToSchema)]
pub struct AdminEntry {
	/// The login the entry admits.
	pub email: String,
	/// Whether the entry carries the danger permission. An entry exists because
	/// the login is an administrator; this says whether it also reaches danger
	/// (see the ADM spec). The tailnet policy can confer either permission on a
	/// login with no entry here at all, which this does not show.
	pub danger: bool,
}

/// List the admin allow-list.
///
/// Returns every account granted admin access to this API and whether its entry
/// also carries the danger permission, in no particular order.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "admin_list",
	tag = "admins",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "Admin entries.", body = Vec<AdminEntry>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<AdminEntry>>> {
	let mut conn = state.db.get().await?;
	let admins = database::admins::Admin::list(&mut conn)
		.await?
		.into_iter()
		.map(|a| AdminEntry {
			email: a.email,
			danger: a.danger,
		})
		.collect();
	Ok(Json(admins))
}

/// Request body for granting or withdrawing the danger permission.
#[derive(Deserialize, ToSchema)]
pub struct SetDangerArgs {
	/// The allow-list entry to amend.
	pub email: String,
	/// Whether the entry should carry the danger permission.
	pub danger: bool,
}

/// Grant or withdraw the danger permission on an allow-list entry.
///
/// Amends the operator's existing entry rather than adding them to a second
/// list. Takes effect at once: the permission is resolved afresh for each
/// request, so withdrawing it reaches an operator who is already raised.
#[utoipa::path(
	post,
	path = "/set_danger",
	operation_id = "admin_set_danger",
	tag = "admins",
	security(("tailscale-admin" = [])),
	request_body = SetDangerArgs,
	responses(
		(status = 200, description = "Permission amended."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, description = "No such allow-list entry.", body = ProblemDetailsSchema),
	),
)]
pub async fn set_danger(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SetDangerArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	database::admins::Admin::set_danger(&mut conn, &args.email, args.danger).await?;
	Ok(Json(()))
}

/// Request body for granting admin access to an email address.
#[derive(Deserialize, ToSchema)]
pub struct AddArgs {
	/// The email address to add to the admin allow-list.
	pub email: String,
}

/// Add an email address to the admin allow-list.
///
/// Grants admin access to the given email address. Has no effect if the
/// email is already an admin.
#[utoipa::path(
	post,
	path = "/add",
	operation_id = "admin_add",
	tag = "admins",
	security(("tailscale-admin" = [])),
	request_body = AddArgs,
	responses(
		(status = 200, description = "Admin added (idempotent)."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn add(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<AddArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	database::admins::Admin::add(&mut conn, &args.email).await?;
	Ok(Json(()))
}

/// Request body for revoking admin access from an email address.
#[derive(Deserialize, ToSchema)]
pub struct DeleteArgs {
	/// The email address to remove from the admin allow-list.
	pub email: String,
}

/// Remove an email address from the admin allow-list.
///
/// Revokes admin access for the given email address. Has no effect if the
/// email was not an admin.
#[utoipa::path(
	post,
	path = "/delete",
	operation_id = "admin_delete",
	tag = "admins",
	security(("tailscale-admin" = [])),
	request_body = DeleteArgs,
	responses(
		(status = 200, description = "Admin removed (idempotent)."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn delete(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeleteArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	database::admins::Admin::delete(&mut conn, &args.email).await?;
	Ok(Json(()))
}
