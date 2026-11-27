use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleLevel;
use commons_types::server::app_type::{ApplicationType, Caps};
use database::access_lists::AccessLevel;
use serde::Serialize;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: public_url))
		.routes(routes!(read_only, silent: server_versions_url))
		.routes(routes!(read_only, silent: calendar_url))
		.routes(routes!(read_only, silent: current_user_level))
		.routes(routes!(read_only, silent: products))
}

/// One product canopy monitors, with what canopy does for its applications and the
/// roles it defines.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApplicationTypeInfo {
	/// The type itself.
	pub r#type: ApplicationType,
	/// What Canopy does for applications of this type.
	pub caps: Caps,
	/// How the type reads when nothing has named the application.
	pub label: String,
}

/// Describe every application type Canopy monitors.
///
/// The operator UI reads this to decide what to present for an application —
/// whether a version applies and whether it can be graded, whether the
/// public-name field is meaningful — rather than restating the mapping
/// client-side, where it would drift as types are added. It offers no roles to
/// choose from: a type is reported, never entered.
// spec: APP#capabilities
#[utoipa::path(
	post,
	path = "/products",
	tag = "commons",
	responses(
		(status = 200, description = "Every application type and its capabilities.", body = Vec<ApplicationTypeInfo>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn products(State(state): State<AppState>) -> Result<Json<Vec<ApplicationTypeInfo>>> {
	let mut conn = state.db_read.get().await?;
	// The types Canopy has handling for, plus the ones the fleet is actually
	// running. The set is open, so a constant list would leave a reported type
	// without a label or capabilities anywhere the SPA presents it.
	// spec: APP#where-a-type-comes-from
	let mut types: Vec<ApplicationType> = ApplicationType::KNOWN.to_vec();
	for in_use in database::applications::Application::distinct_types(&mut conn).await? {
		if !types.contains(&in_use) {
			types.push(in_use);
		}
	}
	// Alphabetical, everywhere types are listed. An invented precedence is
	// surprising to read and is one more thing to maintain as types appear.
	types.sort_by_key(ToString::to_string);

	Ok(Json(
		types
			.into_iter()
			.map(|r#type| ApplicationTypeInfo {
				caps: r#type.caps(),
				label: r#type.label(),
				r#type,
			})
			.collect(),
	))
}

/// Get the configured public API base URL.
///
/// Returns the base URL of the device-facing public API for this Canopy
/// instance, or `null` if none is configured. Used by the operator UI to
/// build links out to device-facing resources.
#[utoipa::path(
	post,
	path = "/public_url",
	tag = "commons",
	responses(
		(status = 200, description = "Public-server URL, if configured.", body = Option<String>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn public_url() -> Result<Json<Option<String>>> {
	Ok(Json(std::env::var("PUBLIC_URL").ok()))
}

/// Get a ready-to-share link to the public server-versions page.
///
/// Returns a full URL to the public server-versions status page, with its
/// access secret already embedded in the query string, so it can be shared
/// and opened directly without further configuration. Returns `null` if the
/// public API base URL or the server-versions secret is not configured.
#[utoipa::path(
	post,
	path = "/server_versions_url",
	tag = "commons",
	responses(
		(status = 200, description = "Application-versions URL with embedded auth secret, if configured.", body = Option<String>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn server_versions_url() -> Result<Json<Option<String>>> {
	let url = (|| {
		let public_url = std::env::var("PUBLIC_URL").ok()?;
		let secret = std::env::var("SERVER_VERSIONS_SECRET").ok()?;
		Some(format!("{public_url}/server-versions?s={secret}"))
	})();
	Ok(Json(url))
}

/// Get the subscription URL of the planned-upgrades calendar feed.
///
/// Returns the feed's full URL, secret included, so it can be handed to a
/// calendar application or shared as-is. Returns `null` if the public API base
/// URL or the secret gating the public reads is not configured.
// spec: UPG#the-calendar-feed
#[utoipa::path(
	post,
	path = "/calendar_url",
	tag = "commons",
	responses(
		(status = 200, description = "Calendar feed URL with its embedded secret, if configured.", body = Option<String>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn calendar_url() -> Result<Json<Option<String>>> {
	let url = (|| {
		let public_url = std::env::var("PUBLIC_URL").ok()?;
		let secret = std::env::var("SERVER_VERSIONS_SECRET").ok()?;
		Some(format!("{public_url}/calendar/{secret}/upgrades.ics"))
	})();
	Ok(Json(url))
}

/// The caller's access level.
///
/// Reports the level the caller holds on the operator surface: `admin`, `pm`,
/// or `all` for a caller holding nothing higher, and `null` for a caller
/// bearing no tailnet identity at all. This endpoint intentionally requires no
/// level of its own, since it exists so a client can decide which controls to
/// offer before doing anything else.
///
/// A failure that isn't an authorisation outcome (a database blip while
/// reading the access lists, say) is reported as an error, not as a lower
/// level, because a caller cannot tell a wrongly-low answer from a real one.
// No `security` block: the handler intentionally accepts unauthenticated
// callers and reports `null`. Marking it user-gated would make Swagger UI
// demand auth before letting you call it, defeating the point of the probe.
// spec: ADM#reporting-a-level-to-a-caller
#[utoipa::path(
	post,
	path = "/current_user_level",
	tag = "commons",
	responses(
		(status = 200, description = "The caller's level, or `null` when no tailnet identity is present.", body = Option<AccessLevel>, content_type = "application/json"),
		(status = 500, description = "The caller's level could not be determined. Distinct from any answer: retry rather than treating the caller as holding a lower level.", body = ProblemDetailsSchema),
	),
)]
pub async fn current_user_level(caller: Option<TailscaleLevel>) -> Json<Option<AccessLevel>> {
	Json(caller.map(|TailscaleLevel(_, level)| level))
}
