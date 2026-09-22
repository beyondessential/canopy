//! Admin management of the bearer tokens gating the public MCP mount.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Access tokens".
//!
//! `mint` is the only place the token plaintext ever leaves the system; the
//! row views carry metadata only.

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::Uuid;
use database::mcp_tokens::McpToken;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list))
		.routes(routes!(danger: mint))
		.routes(routes!(danger: revoke))
}

/// Metadata about an MCP access token. Never includes the secret value
/// itself — that's only ever returned once, at minting time.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct McpTokenView {
	/// Unique identifier of the token.
	pub id: Uuid,
	/// Operator-chosen label for what the token is used for.
	pub name: String,
	/// The login of the admin who minted this token.
	pub created_by: String,
	/// When the token was minted.
	pub created_at: Timestamp,
	/// When the token stops being accepted. Tokens are valid for one year
	/// from minting; there is no way to request a different lifetime.
	pub expires_at: Timestamp,
	/// When the token was revoked, or `null` if it has not been revoked.
	pub revoked_at: Option<Timestamp>,
	/// When the token was last used to authenticate, or `null` if it has
	/// never been used. May lag the true last use by up to a minute.
	pub last_used_at: Option<Timestamp>,
}

impl From<McpToken> for McpTokenView {
	fn from(t: McpToken) -> Self {
		Self {
			id: t.id,
			name: t.name,
			created_by: t.created_by,
			created_at: t.created_at,
			expires_at: t.expires_at,
			revoked_at: t.revoked_at,
			last_used_at: t.last_used_at,
		}
	}
}

/// List MCP access tokens.
///
/// Returns every access token that has ever been minted, newest first,
/// including ones that have since been revoked — this is the full history
/// view. Token secrets are never included, only metadata about each token.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "mcp_tokens_list",
	tag = "mcp_tokens",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "All MCP access tokens, newest first, revoked included.", body = Vec<McpTokenView>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<McpTokenView>>> {
	let mut conn = state.db.get().await?;
	let tokens = McpToken::list(&mut conn).await?;
	Ok(Json(tokens.into_iter().map(Into::into).collect()))
}

/// Request body for minting a new MCP access token.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MintArgs {
	/// Operator-chosen label, e.g. which agent will hold this token. Cannot
	/// be empty or only whitespace.
	pub name: String,
}

/// The result of minting a new MCP access token: its metadata plus the
/// one-time secret value. This is the only response that will ever include
/// the secret.
#[derive(Debug, Serialize, ToSchema)]
pub struct MintedToken {
	/// Metadata about the newly minted token.
	pub token: McpTokenView,
	/// The bearer token itself. Shown once; never retrievable again.
	pub secret: String,
}

/// Mint a new MCP access token.
///
/// Creates a new bearer token that can be used to authenticate against the
/// public MCP endpoint, and returns its metadata together with the
/// plaintext secret. The secret appears only in this response and cannot be
/// retrieved again afterwards, so it must be copied out immediately. Tokens
/// are valid for one year from minting. Returns 400 if the supplied name is
/// empty or only whitespace.
#[utoipa::path(
	post,
	path = "/mint",
	operation_id = "mcp_tokens_mint",
	tag = "mcp_tokens",
	security(("tailscale-admin" = [])),
	request_body = MintArgs,
	responses(
		(status = 200, description = "Freshly minted token; the secret in this response is shown once.", body = MintedToken),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn mint(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<MintArgs>,
) -> Result<Json<MintedToken>> {
	let name = args.name.trim();
	if name.is_empty() {
		return Err(commons_errors::AppError::BadRequest(
			"token name cannot be empty".into(),
		));
	}
	let mut conn = state.db.get().await?;
	let (token, secret) = McpToken::mint(&mut conn, name, &admin.login).await?;
	Ok(Json(MintedToken {
		token: token.into(),
		secret,
	}))
}

/// Request body for revoking an MCP access token.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RevokeArgs {
	/// The id of the token to revoke.
	pub id: Uuid,
}

/// Revoke an MCP access token.
///
/// Immediately invalidates the token with the given id so it can no longer
/// authenticate. Revoking an already-revoked token succeeds without doing
/// anything further. Returns 404 if no token with that id exists.
#[utoipa::path(
	post,
	path = "/revoke",
	operation_id = "mcp_tokens_revoke",
	tag = "mcp_tokens",
	security(("tailscale-admin" = [])),
	request_body = RevokeArgs,
	responses(
		(status = 200, description = "Token revoked (idempotent)."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn revoke(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<RevokeArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	McpToken::revoke(&mut conn, args.id).await?;
	Ok(Json(()))
}
