use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use database::reporting_schemas::{Pair, ReportingSchemaRequest};
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(for_group))
		.routes(routes!(build))
}

/// Request body for reading a group's pairs.
#[derive(Deserialize, ToSchema)]
pub struct PairsForGroupArgs {
	/// The group to report on.
	pub group_id: Uuid,
}

/// Where each of a group's pairs of group and Tamanu version stands.
///
/// One entry per published version the group's Tamanu applications report
/// running, plus the version its open plan moves it to, so whether a group's
/// applications can be offered the schema for the version they run or are
/// moving to is answered in one place.
// spec: RPT#alerting
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "reporting_schemas_for_group",
	tag = "reporting_schemas",
	security(("tailscale-admin" = [])),
	request_body = PairsForGroupArgs,
	responses(
		(status = 200, description = "Pairs, one per version the group runs or is moving to.", body = Vec<Pair>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<PairsForGroupArgs>,
) -> Result<Json<Vec<Pair>>> {
	let mut conn = state.db_read.get().await?;
	let pairs = database::reporting_schemas::pairs_for_group(&mut conn, args.group_id).await?;
	Ok(Json(pairs))
}

/// Which pair to build.
#[derive(Deserialize, ToSchema)]
pub struct BuildPairArgs {
	/// The group whose schema to build.
	pub group_id: Uuid,
	/// The Tamanu version to build it for.
	pub version_id: Uuid,
}

/// Ask for a pair's schema to be built.
///
/// This is how a schema is refreshed after the group's configuration changes,
/// and how a settled pair is put back on the worklist: a build against a fixed
/// version and configuration fails the same way every time, so a failed pair
/// waits for this rather than retrying on its own.
// spec: RPT#pairs
#[utoipa::path(
	post,
	path = "/build",
	operation_id = "reporting_schemas_build",
	tag = "reporting_schemas",
	security(("tailscale-admin" = [])),
	request_body = BuildPairArgs,
	responses(
		(status = 200),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn build(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<BuildPairArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	let TailscaleAdmin(TailscaleUser { login, .. }) = admin;
	ReportingSchemaRequest::enqueue(&mut conn, args.group_id, args.version_id, Some(&login))
		.await?;
	Ok(Json(()))
}
