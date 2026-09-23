//! Enforcement of the safety-mode grades declared on the administrative
//! surface's handlers.
//!
//! Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//!
//! Each handler declares the mode it requires as a prefix on its `routes!`
//! entry, which records the grade as an OpenAPI operation extension. That
//! document is the single place the grade is written: [`GradeMap`] reads it back
//! at startup, and [`enforce`] decides every graded request against the caller's
//! session and identity. Nothing else in the server carries a second copy.

use std::{
	collections::{HashMap, hash_map::Entry},
	sync::{Arc, Mutex},
	time::{Duration, Instant},
};

use axum::http::Method;
use axum::{
	extract::{FromRequestParts, MatchedPath, OptionalFromRequestParts, Request, State},
	middleware::Next,
	response::Response,
};
use canopy_utoipa_axum::SAFETY_MODE_EXTENSION;
use commons_errors::{AppError, Result};
use commons_servers::tailscale_auth::{TailscaleUser, use_dev_identity};
use commons_types::safety::SafetyMode;
use database::operator_sessions::OperatorSession;
use jiff::Timestamp;
use uuid::Uuid;

use crate::state::AppState;

/// The request header carrying the caller's session identifier. Added centrally
/// by the client's `callApi`, so every request from a client that has a session
/// presents it.
pub const SESSION_HEADER: &str = "x-canopy-session";

/// How long a session's last-seen may go unwritten.
///
/// A day's grace before the sweep retires a session, so a quarter of an hour is
/// all the precision it needs. Well above the burst of parallel reads a page
/// makes, so opening one writes the row once rather than a dozen times over,
/// and the rest of the time the boundary takes no connection at all.
const TOUCH_INTERVAL: Duration = Duration::from_secs(15 * 60);

/// When each session's last-seen was last written by this process.
///
/// The row is what the sweep reads; this only decides when to write it. A
/// second process keeps its own and writes on its own cadence, which costs one
/// extra write per interval and nothing in correctness.
///
/// Keyed by the login as well as the session, because the write is scoped to
/// the login too: a caller presenting someone else's identifier writes nothing,
/// and must not take the slot that would have carried the owner's own write.
#[derive(Clone, Default)]
pub struct TouchLog(Arc<Mutex<HashMap<(Uuid, String), Instant>>>);

impl TouchLog {
	/// Whether this session's last-seen is due to be written, recording that it
	/// is about to be so the requests beside this one do not write it too.
	fn due(&self, id: Uuid, login: &str) -> bool {
		let now = Instant::now();
		let mut written = self
			.0
			.lock()
			.expect("touch log is never held across an await");
		// Sessions come and go, so entries no longer standing in the way of a
		// write are dropped rather than kept for a session that may be gone.
		written.retain(|_, at| now.duration_since(*at) < TOUCH_INTERVAL);
		match written.entry((id, login.to_owned())) {
			Entry::Occupied(_) => false,
			Entry::Vacant(slot) => {
				slot.insert(now);
				true
			}
		}
	}
}

/// The grade each handler on the administrative surface requires, keyed by the
/// method and routed path the request matched.
#[derive(Debug, Default)]
pub struct GradeMap(HashMap<(Method, String), SafetyMode>);

impl GradeMap {
	/// Read every operation's grade out of the built OpenAPI document.
	///
	/// An operation carrying no grade is absent from the map rather than
	/// defaulting to one, so [`Self::required`] can tell "graded read-only"
	/// apart from "not graded at all".
	pub fn from_openapi(api: &utoipa::openapi::OpenApi) -> Self {
		let mut map = HashMap::new();
		for (path, item) in &api.paths.paths {
			let operations = [
				(Method::GET, &item.get),
				(Method::PUT, &item.put),
				(Method::POST, &item.post),
				(Method::DELETE, &item.delete),
				(Method::OPTIONS, &item.options),
				(Method::HEAD, &item.head),
				(Method::PATCH, &item.patch),
				(Method::TRACE, &item.trace),
			];
			for (method, operation) in operations {
				let Some(operation) = operation else { continue };
				let Some(mode) = operation
					.extensions
					.as_ref()
					.and_then(|ext| ext.get(SAFETY_MODE_EXTENSION))
					.and_then(|value| value.as_str())
				else {
					continue;
				};
				match mode.parse::<SafetyMode>() {
					Ok(mode) => {
						map.insert((method, path.clone()), mode);
					}
					Err(_) => {
						// Doubt resolves upwards on an enforcement boundary: a
						// grade nothing can read is the most restrictive one,
						// not none. The build rejects this anyway.
						tracing::error!(
							%path, %method, %mode,
							"handler declares a safety mode that is not one of the three; \
							 requiring danger until it does"
						);
						map.insert((method, path.clone()), SafetyMode::Danger);
					}
				}
			}
		}
		Self(map)
	}

	/// The grade a request matching this method and path requires, or `None`
	/// where the handler declares none.
	pub fn required(&self, method: &Method, path: &str) -> Option<SafetyMode> {
		self.0.get(&(method.clone(), path.to_owned())).copied()
	}

	/// How many handlers carry a grade. Used by the startup report and by tests
	/// asserting the surface is fully graded.
	pub fn len(&self) -> usize {
		self.0.len()
	}

	pub fn is_empty(&self) -> bool {
		self.0.is_empty()
	}
}

/// What [`enforce`] needs to decide a request: the application state it resolves
/// identity and permissions against, and the grades it decides them against.
#[derive(Clone)]
pub struct SafetyState {
	pub app: AppState,
	pub grades: Arc<GradeMap>,
	pub touched: TouchLog,
}

/// Keep a session alive, at most once per [`TOUCH_INTERVAL`] per process.
///
/// A read costs no connection at all on the overwhelming majority of requests,
/// which matters because reads are most of them and the boundary would
/// otherwise take a primary-pool connection alongside the handler's own.
async fn touch(safety: &SafetyState, id: Uuid, login: &str) -> Result<()> {
	if !safety.touched.due(id, login) {
		return Ok(());
	}
	let mut conn = safety.app.db.get().await?;
	OperatorSession::touch(&mut conn, id, login).await
}

/// Whether a caller holds the danger permission (see the ADM spec).
///
/// The one place that question is answered, so the boundary and the raise
/// control cannot disagree about it. The development identity holds it for the
/// same reason it is an administrator: so the suite reaches danger-graded work
/// without seeding a permission. Compiled out of release builds.
pub async fn holds_danger(app: &AppState, user: &TailscaleUser) -> Result<bool> {
	if use_dev_identity() {
		return Ok(true);
	}
	let mut conn = app.db.get().await?;
	user.has_danger(&mut conn, app.tailnet_directory.as_ref())
		.await
}

/// Decide one request against the grade its handler declares.
///
/// Read-only-graded requests are answered without a session, so a client can
/// read before it has one. Anything higher is decided against the session named
/// by [`SESSION_HEADER`]: an unknown, expired, or someone else's session all
/// read as read-only rather than being refused outright, which keeps the two
/// refusals meaning exactly what they say.
pub async fn enforce(
	State(safety): State<SafetyState>,
	request: Request,
	next: Next,
) -> Result<Response> {
	let path = request
		.extensions()
		.get::<MatchedPath>()
		.map(|matched| matched.as_str().to_owned())
		.unwrap_or_else(|| request.uri().path().to_owned());
	let required = safety.grades.required(request.method(), &path);

	// The dev identity is an administrator by construction, and is treated as
	// holding a danger-mode session and the danger permission here for the same
	// reason: so the existing suite reaches graded handlers without driving a
	// session. Compiled out of release builds.
	if use_dev_identity() {
		return Ok(next.run(request).await);
	}

	let Some(required) = required else {
		// Nothing on this router reaches a handler without a grade, so a miss is
		// the boundary failing to recognise the request rather than a request
		// that needs no mode. It is refused: an unrecognised path is exactly the
		// case where permitting it would be a silent hole.
		if crate::fns::MOVED_PATHS.contains(&path.as_str()) {
			return Ok(next.run(request).await);
		}
		tracing::error!(%path, method = %request.method(), "no safety mode for this route; refusing");
		return Err(AppError::SafetyModeTooLow {
			required: SafetyMode::Danger.to_string(),
		});
	};

	let session_id = request
		.headers()
		.get(SESSION_HEADER)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| Uuid::parse_str(value.trim()).ok());

	let (mut parts, body) = request.into_parts();

	// A read-only-graded request needs no session, and no identity either: a
	// client may read before it has either. It keeps its own session alive when
	// it presents both, and only its own — an identifier is not something a
	// caller can refresh by holding it.
	if required == SafetyMode::ReadOnly {
		if let Some(id) = session_id {
			let user = <TailscaleUser as OptionalFromRequestParts<AppState>>::from_request_parts(
				&mut parts,
				&safety.app,
			)
			.await?;
			if let Some(user) = user {
				touch(&safety, id, &user.login).await?;
			}
		}
		return Ok(next
			.run(axum::extract::Request::from_parts(parts, body))
			.await);
	}

	let user =
		<TailscaleUser as FromRequestParts<AppState>>::from_request_parts(&mut parts, &safety.app)
			.await?;

	// Resolved afresh per request and checked before the mode, so an operator
	// who can never make this request is told that rather than being told to
	// raise. Withdrawing the permission takes effect during an existing raise.
	if required == SafetyMode::Danger && !holds_danger(&safety.app, &user).await? {
		return Err(AppError::DangerNotPermitted);
	}

	// Scoped so the connection goes back to the pool before the handler runs:
	// held across it, every graded request would take two, and a busy server
	// could exhaust the pool on connections waiting for their own handlers.
	{
		let mut conn = safety.app.db.get().await?;
		let session = match session_id {
			Some(id) => OperatorSession::get(&mut conn, id).await?,
			None => None,
		};
		// A session belonging to another login is not this caller's to use, so
		// it reads as no session at all rather than as its owner's mode.
		let session = session.filter(|session| session.login == user.login);
		let mode = session
			.as_ref()
			.map(|session| session.effective_mode(Timestamp::now()))
			.unwrap_or(SafetyMode::ReadOnly);

		if !mode.permits(required) {
			return Err(AppError::SafetyModeTooLow {
				required: required.to_string(),
			});
		}

		// On the connection already in hand, and on the same cadence as a read's.
		if let Some(session) = session
			&& safety.touched.due(session.id, &user.login)
		{
			OperatorSession::touch(&mut conn, session.id, &user.login).await?;
		}
	}

	Ok(next
		.run(axum::extract::Request::from_parts(parts, body))
		.await)
}
