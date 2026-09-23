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
	collections::HashMap,
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

/// How many sessions this process tracks last-seen writes for. Far above the
/// sessions a Canopy instance's operators hold between them, and a ceiling
/// rather than a target: it exists so the map cannot grow without bound.
const MAX_TRACKED_SESSIONS: usize = 10_000;

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
	/// Whether this session's last-seen is due to be written.
	fn due(&self, id: Uuid, login: &str) -> bool {
		let now = Instant::now();
		// A poisoned lock is no reason to refuse every request on the
		// administrative surface: the worst a torn entry costs is one extra
		// write of a timestamp.
		let mut written = self.0.lock().unwrap_or_else(|held| held.into_inner());
		// Sessions come and go, so entries no longer standing in the way of a
		// write are dropped rather than kept for a session that may be gone.
		written.retain(|_, at| now.duration_since(*at) < TOUCH_INTERVAL);
		!written.contains_key(&(id, login.to_owned()))
	}

	/// Record a write that landed, so the requests beside it do not write again.
	///
	/// Only a write that matched a row is recorded, so an identifier that is
	/// unknown or another login's leaves nothing behind: a caller offering a
	/// fresh one each request cannot grow this without bound.
	fn wrote(&self, id: Uuid, login: &str) {
		let mut written = self.0.lock().unwrap_or_else(|held| held.into_inner());
		// However many sessions are genuinely live, this is a cache of when they
		// were last written, and starting it over costs one extra write each.
		if written.len() >= MAX_TRACKED_SESSIONS {
			written.clear();
		}
		written.insert((id, login.to_owned()), Instant::now());
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

	/// Whether any method on this path is graded.
	///
	/// A request naming a path the surface serves but a method it does not is
	/// the router's to refuse, with the method-not-allowed that says so, rather
	/// than the boundary's to treat as an unrecognised route.
	pub fn knows_path(&self, path: &str) -> bool {
		self.0.keys().any(|(_, known)| known == path)
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
	if OperatorSession::touch(&mut conn, id, login).await? > 0 {
		safety.touched.wrote(id, login);
	}
	Ok(())
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

	let method = request.method().clone();
	let (mut parts, body) = request.into_parts();

	let Some(required) = required else {
		// Every handler on this router declares a grade, so a miss is the server
		// failing to recognise its own route rather than a request that needs no
		// mode. It is refused, because permitting it is the one outcome that
		// would be a silent hole — and reported as the fault it is, rather than
		// as a lapsed raise, which would send the client off to raise again.
		if safety.grades.knows_path(&path) {
			return Ok(next
				.run(axum::extract::Request::from_parts(parts, body))
				.await);
		}
		tracing::error!(%path, %method, "no safety mode for this route; refusing");
		return Err(AppError::custom(format!(
			"no safety mode is declared for {path}"
		)));
	};

	let session_id = parts
		.headers
		.get(SESSION_HEADER)
		.and_then(|value| value.to_str().ok())
		.and_then(|value| Uuid::parse_str(value.trim()).ok());

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
			&& OperatorSession::touch(&mut conn, session.id, &user.login).await? > 0
		{
			safety.touched.wrote(session.id, &user.login);
		}
	}

	Ok(next
		.run(axum::extract::Request::from_parts(parts, body))
		.await)
}
