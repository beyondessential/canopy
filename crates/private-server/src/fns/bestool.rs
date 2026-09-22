use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleUser;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::fns::Page;
use crate::state::AppState;

/// Summary of a saved SQL snippet, as shown when listing the snippet
/// library.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BestoolSnippetInfo {
	/// Unique identifier for this version of the snippet.
	pub id: Uuid,
	/// The snippet's display name.
	pub name: String,
	/// Optional description of what the snippet does.
	pub description: Option<String>,
}

/// Full content of a saved SQL snippet, including its query text.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BestoolSnippetDetail {
	/// Unique identifier for this version of the snippet.
	pub id: Uuid,
	/// The snippet's display name.
	pub name: String,
	/// Optional description of what the snippet does.
	pub description: Option<String>,
	/// The stored SQL query text.
	pub sql: String,
	/// Tailscale login of the user who created this version of the
	/// snippet.
	pub editor: String,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list_snippets))
		.routes(routes!(write: save_snippet))
		.routes(routes!(read_only: get_snippet))
		.routes(routes!(read_only: get_latest_snippet_id))
		.routes(routes!(write: delete_snippet))
}

/// Pagination parameters for listing the snippet library.
#[derive(Deserialize, ToSchema)]
pub struct BestoolListArgs {
	/// Number of snippets to skip before the returned page starts.
	pub offset: u64,
	/// Maximum number of snippets to return; defaults to 50 if omitted.
	pub limit: Option<u64>,
}

/// List saved SQL snippets.
///
/// Returns a page of the current snippets in the library — one entry per
/// snippet, showing only its latest, non-deleted version — ordered by
/// name, together with the total count.
#[utoipa::path(
	post,
	path = "/list_snippets",
	tag = "bestool",
	request_body = BestoolListArgs,
	responses(
		(status = 200, body = Page<BestoolSnippetInfo>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn list_snippets(
	State(state): State<AppState>,
	Json(args): Json<BestoolListArgs>,
) -> Result<Json<Page<BestoolSnippetInfo>>> {
	let mut conn = state.db.get().await?;
	let total = database::BestoolSnippet::count_current(&mut conn).await? as u64;
	let snippets = database::BestoolSnippet::list_current(
		&mut conn,
		args.offset as i64,
		args.limit.unwrap_or(50) as i64,
	)
	.await?;
	let items = snippets
		.into_iter()
		.map(|s| BestoolSnippetInfo {
			id: s.id,
			name: s.name,
			description: s.description,
		})
		.collect();
	Ok(Json(Page { items, total }))
}

/// Request body for creating a new snippet, or saving a new version of an
/// existing one.
#[derive(Deserialize, ToSchema)]
pub struct SaveArgs {
	/// When set, this save creates a new version that supersedes the
	/// snippet with this id (an edit to an existing snippet). When
	/// omitted, a brand new snippet is created.
	pub supersedes: Option<Uuid>,
	/// The snippet's display name.
	pub name: String,
	/// Optional description of what the snippet does.
	pub description: Option<String>,
	/// The SQL query text to store.
	pub sql: String,
}

/// Save a SQL snippet.
///
/// Creates a brand new snippet, or — when `supersedes` is set — a new
/// version that supersedes an existing one. The new version is recorded
/// under the caller's Tailscale identity. Returns the full content of the
/// newly created version.
#[utoipa::path(
	post,
	path = "/save_snippet",
	tag = "bestool",
	security(("tailscale-user" = [])),
	request_body = SaveArgs,
	responses(
		(status = 200, body = BestoolSnippetDetail),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn save_snippet(
	State(state): State<AppState>,
	user: TailscaleUser,
	Json(args): Json<SaveArgs>,
) -> Result<Json<BestoolSnippetDetail>> {
	let mut conn = state.db.get().await?;
	let snippet = database::BestoolSnippet::create(
		&mut conn,
		user.login,
		args.name,
		args.description,
		args.sql,
		args.supersedes,
	)
	.await?;
	Ok(Json(BestoolSnippetDetail {
		id: snippet.id,
		name: snippet.name,
		description: snippet.description,
		sql: snippet.sql,
		editor: snippet.editor,
	}))
}

/// Request body identifying a single snippet version.
#[derive(Deserialize, ToSchema)]
pub struct GetArgs {
	/// The snippet version's unique identifier. Doesn't need to be the
	/// latest version in its history.
	pub id: Uuid,
}

/// Get a saved SQL snippet by id.
///
/// Returns the full content of the given snippet version, including its
/// SQL text. The id doesn't have to be the current version — superseded
/// and deleted versions can still be fetched directly.
#[utoipa::path(
	post,
	path = "/get_snippet",
	tag = "bestool",
	request_body = GetArgs,
	responses(
		(status = 200, body = BestoolSnippetDetail),
		(status = 404, body = ProblemDetailsSchema),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get_snippet(
	State(state): State<AppState>,
	Json(args): Json<GetArgs>,
) -> Result<Json<BestoolSnippetDetail>> {
	let mut conn = state.db.get().await?;
	let snippet = database::BestoolSnippet::get_by_id(&mut conn, args.id)
		.await?
		.ok_or_else(|| AppError::NotFound(format!("no snippet with id {}", args.id)))?;
	Ok(Json(BestoolSnippetDetail {
		id: snippet.id,
		name: snippet.name,
		description: snippet.description,
		sql: snippet.sql,
		editor: snippet.editor,
	}))
}

/// Resolve a snippet id to its latest version.
///
/// Follows the version chain forward from the given id and returns the
/// newest id in that chain (the same id, if it's already the latest).
/// Useful for turning a stored or bookmarked snippet id into the id of
/// its current version.
#[utoipa::path(
	post,
	path = "/get_latest_snippet_id",
	tag = "bestool",
	request_body = GetArgs,
	responses(
		(status = 200, description = "The newest id in the supersedes chain rooted at the given id.", body = Uuid, content_type = "application/json"),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get_latest_snippet_id(
	State(state): State<AppState>,
	Json(args): Json<GetArgs>,
) -> Result<Json<Uuid>> {
	let mut conn = state.db.get().await?;
	let id = database::BestoolSnippet::get_latest_id(&mut conn, args.id).await?;
	Ok(Json(id))
}

/// Soft-delete a saved snippet.
///
/// Marks the given snippet version as deleted; it stops appearing in the
/// snippet list, but the version itself (and the rest of its history)
/// isn't removed. Deleting one version doesn't affect any other version
/// in the same snippet's history.
#[utoipa::path(
	post,
	path = "/delete_snippet",
	tag = "bestool",
	request_body = GetArgs,
	responses(
		(status = 200, description = "Snippet soft-deleted."),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn delete_snippet(
	State(state): State<AppState>,
	Json(args): Json<GetArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	let _ = database::BestoolSnippet::delete(&mut conn, args.id).await?;
	Ok(Json(()))
}
