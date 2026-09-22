use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::server::app_type::{ApplicationType, Caps};
use serde::Serialize;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: public_url))
		.routes(routes!(read_only: server_versions_url))
		.routes(routes!(read_only: calendar_url))
		.routes(routes!(read_only: is_current_user_admin))
		.routes(routes!(read_only: products))
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

/// Check whether the caller is an admin.
///
/// Reports `true` if the caller is authenticated and their identity is on
/// the admin allow-list, `false` if the caller definitely isn't an admin —
/// including when the caller is not authenticated at all. This endpoint
/// intentionally requires no authentication of its own, since it exists so a
/// client can check whether to show admin-only controls before doing anything
/// else.
///
/// A failure that isn't an authorization outcome (a database blip while
/// reading the allow-list, say) is reported as an error, not as `false`.
// No `security` block: the handler intentionally accepts unauthenticated
// callers and reports `false`. Marking it admin-gated (or even user-gated)
// would make Swagger UI demand auth before letting you call it, defeating
// the point of the probe.
#[utoipa::path(
	post,
	path = "/is_current_user_admin",
	tag = "commons",
	responses(
		(status = 200, description = "`true` if the caller's Tailscale identity is an administrator (on the admin allow-list, or granted admin by the tailnet policy); `false` otherwise (including when no Tailscale identity is present).", body = bool, content_type = "application/json"),
		(status = 500, description = "The caller's admin status could not be determined. Distinct from a `false` answer: retry rather than treating the caller as a non-admin.", body = ProblemDetailsSchema),
	),
)]
pub async fn is_current_user_admin(
	admin: std::result::Result<TailscaleAdmin, AppError>,
) -> Result<Json<bool>> {
	admin_probe_answer(admin).map(Json)
}

/// Classify an admin-extractor outcome into the probe's answer.
///
/// `TailscaleAdmin` rejects for two very different kinds of reason, and
/// collapsing them both to `false` is a bug: the UI caches the answer for the
/// session, so one database hiccup while checking the allow-list used to leave
/// a real admin looking at a read-only app with no admin controls and no
/// explanation. Only a definite authorization outcome is an answer; everything
/// else propagates so the caller knows to retry.
fn admin_probe_answer(admin: std::result::Result<TailscaleAdmin, AppError>) -> Result<bool> {
	match admin {
		Ok(_) => Ok(true),
		Err(
			AppError::AuthMissingHeader(_)
			| AppError::AuthTailnetIdentityMissing
			| AppError::AuthInsufficientPermissions { .. },
		) => Ok(false),
		Err(err) => Err(err),
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use commons_servers::tailscale_auth::TailscaleUser;

	#[test]
	fn admin_is_true() {
		let admin = TailscaleAdmin(TailscaleUser::default());
		assert!(admin_probe_answer(Ok(admin)).unwrap());
	}

	#[test]
	fn authorization_outcomes_are_a_definite_no() {
		for err in [
			AppError::AuthMissingHeader("Tailscale-User-Login"),
			AppError::AuthTailnetIdentityMissing,
			AppError::AuthInsufficientPermissions {
				required: "admin".into(),
			},
		] {
			assert!(!admin_probe_answer(Err(err)).unwrap());
		}
	}

	#[test]
	fn infrastructure_failures_are_not_an_answer() {
		for err in [
			AppError::custom("connection pool timed out"),
			AppError::Upstream("tailnet control plane".into()),
		] {
			assert!(admin_probe_answer(Err(err)).is_err());
		}
	}
}
