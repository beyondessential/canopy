//! The operator's own safety-mode session: reading it, raising it, lowering it.
//!
//! Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//!
//! Every handler here is graded read-only, because these are how an operator
//! reaches a higher mode in the first place and a read-only session must be able
//! to call them. What they change is the operator's own session, not the fleet
//! or Canopy's records.

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::safety::SafetyMode;
use database::operator_sessions::OperatorSession;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::safety::SESSION_HEADER;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: session))
		.routes(routes!(read_only: raise))
		.routes(routes!(read_only: lower))
}

/// The state of an operator's session, as the client presents it.
#[derive(Debug, Serialize, ToSchema)]
pub struct SessionState {
	/// The session identifier, to be sent back on every subsequent request.
	pub id: Uuid,
	/// The mode the session is in right now. A raise that has lapsed reads as
	/// read-only here, so the client and the server agree.
	pub mode: SafetyMode,
	/// When the current raise lapses, as an RFC 3339 timestamp. Absent while the
	/// session is read-only. The client counts down to this.
	pub raise_expires_at: Option<String>,
}

impl SessionState {
	fn of(session: &OperatorSession) -> Self {
		let mode = session.effective_mode(Timestamp::now());
		Self {
			id: session.id,
			mode,
			// A lapsed raise has no time left to show, whatever is stored.
			raise_expires_at: match mode {
				SafetyMode::ReadOnly => None,
				_ => session.raise_expires_at.map(|at| at.to_string()),
			},
		}
	}
}

/// The session named by the request header, if it exists and belongs to the
/// caller. A session identifier from another login is not this caller's to use.
async fn presented_session(
	state: &AppState,
	headers: &axum::http::HeaderMap,
	login: &str,
) -> Result<Option<OperatorSession>> {
	let Some(id) = headers
		.get(SESSION_HEADER)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| Uuid::parse_str(value.trim()).ok())
	else {
		return Ok(None);
	};
	let mut conn = state.db.get().await?;
	Ok(OperatorSession::get(&mut conn, id)
		.await?
		.filter(|session| session.login == login))
}

/// Read the caller's session, minting one if they have none.
///
/// A client calls this when it connects. Presenting a session identifier that is
/// unknown, expired, or another login's mints a fresh read-only session rather
/// than failing, so a client always ends up with a usable session.
#[utoipa::path(
	post,
	path = "/session",
	operation_id = "safety_session",
	tag = "safety",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "The caller's session.", body = SessionState),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn session(
	State(state): State<AppState>,
	TailscaleAdmin(user): TailscaleAdmin,
	headers: axum::http::HeaderMap,
) -> Result<Json<SessionState>> {
	if let Some(session) = presented_session(&state, &headers, &user.login).await? {
		return Ok(Json(SessionState::of(&session)));
	}
	let mut conn = state.db.get().await?;
	let session = OperatorSession::create(&mut conn, &user.login).await?;
	Ok(Json(SessionState::of(&session)))
}

/// Request body for raising a session to a higher mode.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RaiseArgs {
	/// The mode to raise to. Raising to read-only is a lowering; use `lower`.
	pub mode: SafetyMode,
}

/// Raise the caller's session to a higher mode for ten minutes.
///
/// Raising to danger requires the danger permission; an operator without it is
/// told they lack the permission rather than that something went wrong. The
/// client offers danger to every operator, because nothing tells it in advance
/// whether its operator holds the permission.
#[utoipa::path(
	post,
	path = "/raise",
	operation_id = "safety_raise",
	tag = "safety",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "The raised session.", body = SessionState),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn raise(
	State(state): State<AppState>,
	TailscaleAdmin(user): TailscaleAdmin,
	headers: axum::http::HeaderMap,
	Json(args): Json<RaiseArgs>,
) -> Result<Json<SessionState>> {
	if args.mode == SafetyMode::ReadOnly {
		return Err(AppError::BadRequest(
			"raising to read-only is a lowering; use lower".into(),
		));
	}

	let mut conn = state.db.get().await?;
	if args.mode == SafetyMode::Danger
		&& !user
			.has_danger(&mut conn, state.tailnet_directory.as_ref())
			.await?
	{
		return Err(AppError::DangerNotPermitted);
	}

	let session = match presented_session(&state, &headers, &user.login).await? {
		Some(session) => session,
		None => OperatorSession::create(&mut conn, &user.login).await?,
	};
	let raised = OperatorSession::raise(&mut conn, session.id, &user.login, args.mode)
		.await?
		.ok_or_else(|| AppError::NotFound("session".into()))?;
	Ok(Json(SessionState::of(&raised)))
}

/// Lower the caller's session back to read-only at once.
///
/// An operator lowers their mode without waiting for the remaining time to run
/// out. Lowering a session that is already read-only is no change.
#[utoipa::path(
	post,
	path = "/lower",
	operation_id = "safety_lower",
	tag = "safety",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "The lowered session.", body = SessionState),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn lower(
	State(state): State<AppState>,
	TailscaleAdmin(user): TailscaleAdmin,
	headers: axum::http::HeaderMap,
) -> Result<Json<SessionState>> {
	let mut conn = state.db.get().await?;
	let session = match presented_session(&state, &headers, &user.login).await? {
		Some(session) => session,
		None => OperatorSession::create(&mut conn, &user.login).await?,
	};
	let lowered = OperatorSession::lower(&mut conn, session.id, &user.login)
		.await?
		.ok_or_else(|| AppError::NotFound("session".into()))?;
	Ok(Json(SessionState::of(&lowered)))
}
