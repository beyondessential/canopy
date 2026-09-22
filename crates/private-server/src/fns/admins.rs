use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use serde::Deserialize;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list))
		.routes(routes!(danger: add))
		.routes(routes!(danger: delete))
}

/// List the admin allow-list.
///
/// Returns the email addresses of every account currently granted admin
/// access to this API, in no particular order.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "admin_list",
	tag = "admins",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "Admin emails.", body = Vec<String>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<String>>> {
	let mut conn = state.db.get().await?;
	let admins = database::admins::Admin::list(&mut conn)
		.await?
		.into_iter()
		.map(|a| a.email)
		.collect();
	Ok(Json(admins))
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
