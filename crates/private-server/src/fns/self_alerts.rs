//! Self-alerts for the operator UI: canopy's problems with its own
//! operation, presented apart from fleet issues.
//!
//! Spec: `.workhorse/specs/private-server/self-alerts.md` (id `SELF`).

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::Uuid;
use commons_types::issue::ResolvedReason;
use commons_types::status::CheckResult;
use database::issues::Issue;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: active))
		.routes(routes!(read_only: list))
		.routes(routes!(write: resolve))
}

/// A self-alert: a problem with canopy's own operation, such as an
/// expiring credential or a failed notification delivery — distinct from
/// issues raised against monitored applications.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SelfAlertView {
	/// Unique identifier of this self-alert.
	pub id: Uuid,
	/// The stable identifier of the underlying condition, e.g.
	/// `mcp-token-expiry`. Stable across repeated raises of the same
	/// condition.
	pub r#ref: String,
	/// What canopy observed on the latest raise, before policy.
	#[schema(value_type = Option<String>)]
	pub observed_result: Option<CheckResult>,
	/// What policy made of it — the result canopy acts on.
	#[schema(value_type = Option<String>)]
	pub effective_result: Option<CheckResult>,
	/// Whether this condition's policy escalates: an effective failure
	/// notifies immediately, bypassing incident grace.
	pub escalates: bool,
	/// Single-line headline.
	pub title: Option<String>,
	/// Full detail message describing the condition.
	pub message: String,
	/// Whatever structured detail the condition attached, or `null` where it
	/// attached none. A message is for reading and this is for acting on: the
	/// stale-healthcheck alert, for one, lists the checks it names here so the
	/// surface can link each to its own policy page.
	pub detail: Option<serde_json::Value>,
	/// Whether the underlying condition is still ongoing. Becomes `false`
	/// once the condition has cleared on its own, independently of whether
	/// an operator has resolved the alert.
	pub active: bool,
	/// When this condition was first raised.
	pub first_seen: Timestamp,
	/// When this condition was most recently reaffirmed as still ongoing.
	pub last_seen: Timestamp,
	/// When an operator marked this alert resolved, or `null` if it has not
	/// been resolved.
	pub resolved_at: Option<Timestamp>,
	/// The login of the operator who resolved this alert, or `null` if it
	/// has not been resolved.
	pub resolved_by: Option<String>,
}

impl From<Issue> for SelfAlertView {
	fn from(i: Issue) -> Self {
		Self {
			id: i.id,
			r#ref: i.r#ref,
			observed_result: i.observed_result,
			effective_result: i.effective_result,
			escalates: i.escalates,
			title: i.description,
			message: i.message,
			detail: i.detail,
			active: i.active,
			first_seen: i.first_seen,
			last_seen: i.last_seen,
			resolved_at: i.resolved_at,
			resolved_by: i.resolved_by,
		}
	}
}

/// List currently-alerting self-alerts.
///
/// Returns self-alerts whose underlying condition is still ongoing and
/// that have not been marked resolved by an operator. This is the feed
/// meant for a live alert banner; see the full listing endpoint for
/// complete history including recovered and resolved alerts.
#[utoipa::path(
	post,
	path = "/active",
	operation_id = "self_alerts_active",
	tag = "self_alerts",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, description = "Currently-alerting self-alerts (active and not operator-resolved), for the banner.", body = Vec<SelfAlertView>),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn active(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<Vec<SelfAlertView>>> {
	let mut conn = state.db.get().await?;
	let alerts = database::self_alerts::list(&mut conn, 50)
		.await?
		.into_iter()
		.filter(|i| i.active && i.resolved_at.is_none())
		.map(Into::into)
		.collect();
	Ok(Json(alerts))
}

/// List self-alerts.
///
/// Returns self-alerts ordered by most recent activity first, including
/// ones that have since recovered on their own or been resolved by an
/// operator. Use the "active" endpoint instead for just the ones currently
/// alerting.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "self_alerts_list",
	tag = "self_alerts",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, description = "Self-alerts, newest activity first, recovered/resolved included.", body = Vec<SelfAlertView>),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<Vec<SelfAlertView>>> {
	let mut conn = state.db.get().await?;
	let alerts = database::self_alerts::list(&mut conn, 50)
		.await?
		.into_iter()
		.map(Into::into)
		.collect();
	Ok(Json(alerts))
}

/// Request body for resolving a self-alert.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SelfAlertsResolveArgs {
	/// The id of the self-alert to resolve.
	pub id: Uuid,
}

/// Mark a self-alert as resolved.
///
/// Records that an operator has resolved the given self-alert. Returns 404
/// if no self-alert with that id exists.
#[utoipa::path(
	post,
	path = "/resolve",
	operation_id = "self_alerts_resolve",
	tag = "self_alerts",
	security(("tailscale-admin" = [])),
	request_body = SelfAlertsResolveArgs,
	responses(
		(status = 200, description = "Alert marked operator-resolved."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn resolve(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<SelfAlertsResolveArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Issue::resolve(&mut conn, args.id, &admin.login, ResolvedReason::Fixed).await?;
	Ok(Json(()))
}
