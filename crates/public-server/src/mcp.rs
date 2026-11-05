//! The internet-facing mount of the MCP query interface, gated by
//! bearer tokens ([`database::mcp_tokens::McpToken`]).
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`).
//!
//! This is deliberately NOT part of [`crate::routes`]: it must not appear on
//! the private server's `/public` nest (the operator surface has its own
//! tailnet-gated mount at `/api/mcp`) nor in the device OpenAPI spec. The
//! binary's `main` and the test harness compose it in alongside the device
//! routes.

use axum::{
	Router,
	extract::{Request, State},
	http::header,
	middleware::{self, Next},
	response::{IntoResponse, Response},
};
use axum_client_ip::ClientIp;
use commons_errors::AppError;
use database::mcp_tokens::McpToken;
use std::time::Duration;

use crate::state::AppState;

/// Failed-auth budget within a 1-minute window, per source IP. Only failures
/// spend it, so an agent presenting a working token never approaches it;
/// a token-guesser is blunted. Guessing is hopeless anyway (tokens are 256-bit
/// CSPRNG), this just bounds the DB lookups it can burn.
///
/// Once an IP has spent the budget it is turned away for the rest of the
/// window whatever it presents, valid token included. That is the cost of
/// bounding the lookups: whether a token is good is exactly what the lookup
/// answers. It only bites an IP already producing thirty auth failures a
/// minute.
const RL_WINDOW: Duration = Duration::from_secs(60);
const RL_PER_IP: u32 = 30;

/// The `/mcp` mount: the shared MCP tower service behind the bearer gate.
pub fn routes(state: AppState) -> Router<()> {
	Router::new().nest(
		"/mcp",
		Router::new()
			.fallback_service(canopy_mcp::service(state.db_read.clone(), state.db.clone()))
			.layer(middleware::from_fn_with_state(state, require_bearer_token)),
	)
}

/// Gate on a usable bearer token. Missing, malformed, unknown, revoked, and
/// expired tokens all yield the same opaque 401 (with a `WWW-Authenticate`
/// challenge); the distinction is logged. The token's name is logged on
/// success so each query is attributable, and its `last_used_at` is bumped.
async fn require_bearer_token(
	State(state): State<AppState>,
	ClientIp(ip): ClientIp,
	mut req: Request,
	next: Next,
) -> Result<Response, AppError> {
	// Before anything that costs: an IP that has already spent its budget is
	// turned away without a connection checkout or a query. `refuse` consults
	// the same budget, but by the time it runs `find_active` has been and
	// gone — so the budget bounded the *responses* a guesser got, not the
	// database work it burned, which is what it is documented to bound.
	let key = format!("mcp:{ip}");
	if state.rate_limiter.exceeded(&key, RL_PER_IP, RL_WINDOW) {
		tracing::warn!(target: "mcp_auth", %ip, "mcp auth rate limit exceeded");
		return Err(AppError::RateLimited);
	}

	let presented = req
		.headers()
		.get(header::AUTHORIZATION)
		.and_then(|v| v.to_str().ok())
		.and_then(|v| {
			let (scheme, token) = v.split_once(' ')?;
			scheme
				.eq_ignore_ascii_case("bearer")
				.then_some(token.trim())
		});

	let token = match presented {
		None => {
			tracing::warn!(target: "mcp_auth", %ip, "mcp request without bearer token");
			return refuse(&state, ip);
		}
		Some(plaintext) => {
			let mut conn = state.db.get().await?;
			match McpToken::find_active(&mut conn, plaintext).await? {
				None => {
					tracing::warn!(target: "mcp_auth", %ip, "mcp request with unusable bearer token");
					return refuse(&state, ip);
				}
				Some(token) => token,
			}
		}
	};

	tracing::info!(token = %token.name, %ip, "mcp request");
	{
		let mut conn = state.db.get().await?;
		McpToken::touch_last_used(&mut conn, token.id).await?;
	}
	req.extensions_mut().insert(canopy_mcp::Caller::Token {
		id: token.id,
		name: token.name,
		grants: token.grants,
	});

	Ok(next.run(req).await)
}

/// The uniform refusal: 429 once an IP exhausts its failed-attempt budget,
/// otherwise the opaque 401 with its bearer challenge.
fn refuse(state: &AppState, ip: std::net::IpAddr) -> Result<Response, AppError> {
	if !state
		.rate_limiter
		.check(&format!("mcp:{ip}"), RL_PER_IP, RL_WINDOW)
	{
		tracing::warn!(target: "mcp_auth", %ip, "mcp auth rate limit exceeded");
		return Err(AppError::RateLimited);
	}
	let mut res = AppError::AuthTokenNotValid.into_response();
	res.headers_mut().insert(
		header::WWW_AUTHENTICATE,
		header::HeaderValue::from_static("Bearer"),
	);
	Ok(res)
}
