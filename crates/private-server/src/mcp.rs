//! The operator-surface gate for the MCP mount; the interface itself lives in
//! the `canopy-mcp` crate.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`).

use crate::state::AppState;

/// Gate the MCP mount on an authenticated tailnet user (any user, not only
/// admins). Reuses the operator surface's `TailscaleUser` extractor, so the
/// debug-build dev bypass applies in local dev and tests. The caller's login is
/// logged so each query is attributable, and the caller, with whether they are
/// an administrator, is put in the request's extensions for the tool that
/// writes to read.
pub async fn require_tailnet_user(
	axum::extract::State(state): axum::extract::State<AppState>,
	req: axum::extract::Request,
	next: axum::middleware::Next,
) -> Result<axum::response::Response, commons_errors::AppError> {
	use axum::extract::FromRequestParts as _;
	use commons_errors::AppError;
	use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};

	let (mut parts, body) = req.into_parts();
	let user = TailscaleUser::from_request_parts(&mut parts, &state).await?;
	let admin = match TailscaleAdmin::from_request_parts(&mut parts, &state).await {
		Ok(_) => true,
		Err(AppError::AuthInsufficientPermissions { .. }) => false,
		Err(e) => return Err(e),
	};
	tracing::info!(login = %user.login, admin, "mcp request");
	parts.extensions.insert(canopy_mcp::Caller::Operator {
		login: user.login,
		name: user.name,
		admin,
	});
	let req = axum::extract::Request::from_parts(parts, body);
	Ok(next.run(req).await)
}
