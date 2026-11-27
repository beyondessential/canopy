//! The operator's own homepage: the page the interface's root opens.
//!
//! Spec: `.workhorse/specs/private-server/homepage.md` (id `HOME`).
//!
//! Every handler here is graded read-only, because what they change is the
//! operator's own preference rather than the fleet or Canopy's records, and a
//! read-only session has to be able to reach them (see SAFE, "Grading the
//! operator surface"). They ask for a tailnet identity, since every operator has
//! a homepage.

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleUser;
use database::operator_homepages::OperatorHomepage;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: get))
		.routes(routes!(read_only, silent: set))
		.routes(routes!(read_only, silent: clear))
}

/// The caller's homepage: the page they chose, and their default.
#[derive(Debug, Serialize, ToSchema)]
pub struct Homepage {
	/// The path the caller has chosen, if any.
	pub chosen: Option<String>,
	/// The default the tailnet policy gives the caller, if any. Where it gives
	/// none, the default is the status page.
	pub default: Option<String>,
}

/// The caller's homepage.
///
/// Returns the path the caller has chosen, if any, and the default the tailnet
/// policy gives them, if any. The root opens the chosen path, else the
/// default, else the status page.
#[utoipa::path(
	post,
	path = "/get",
	operation_id = "homepage_get",
	tag = "homepage",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Homepage),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
// spec: HOME#what-the-root-opens
pub async fn get(State(state): State<AppState>, user: TailscaleUser) -> Result<Json<Homepage>> {
	let mut conn = state.db.get().await?;
	let chosen = OperatorHomepage::get(&mut conn, &user.login).await?;
	let default = match &state.tailnet_directory {
		Some(directory) => directory.default_home_by_policy(&user.login).await,
		None => None,
	};
	Ok(Json(Homepage { chosen, default }))
}

/// A page to choose as homepage.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetHomepageArgs {
	/// The page's path within the interface, beginning with a single `/`,
	/// without a query or fragment.
	pub path: String,
}

/// Choose the caller's homepage, replacing any earlier choice.
///
/// Refuses a path that does not begin with a single `/`, is `/` alone, or
/// carries a query or fragment.
#[utoipa::path(
	post,
	path = "/set",
	operation_id = "homepage_set",
	tag = "homepage",
	security(("tailscale-user" = [])),
	request_body = SetHomepageArgs,
	responses(
		(status = 200, description = "Chosen."),
		(status = 400, description = "The path is not a page's path.", body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
// spec: HOME#choosing-a-homepage
pub async fn set(
	State(state): State<AppState>,
	user: TailscaleUser,
	Json(args): Json<SetHomepageArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	OperatorHomepage::set(&mut conn, &user.login, &args.path).await?;
	Ok(Json(()))
}

/// Clear the caller's choice, so the root opens their default again.
///
/// Has no effect where the caller has chosen nothing.
#[utoipa::path(
	post,
	path = "/clear",
	operation_id = "homepage_clear",
	tag = "homepage",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, description = "Cleared, or there was no choice."),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
// spec: HOME#choosing-a-homepage
pub async fn clear(State(state): State<AppState>, user: TailscaleUser) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	OperatorHomepage::clear(&mut conn, &user.login).await?;
	Ok(Json(()))
}
