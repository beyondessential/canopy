use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use database::Db;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::state::AppState;

/// A single named SQL snippet from the bestool snippet library.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SnippetResponse {
	/// Human-readable explanation of what the snippet does, if the author
	/// provided one.
	pub description: Option<String>,
	/// The snippet's SQL text.
	pub sql: String,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(public: list_snippets))
}

/// List all current bestool SQL snippets.
///
/// Returns the library of named SQL snippets that devices running bestool
/// fetch and run, keyed by snippet name. Only the current version of each
/// snippet is included: if a snippet has been superseded by a newer one
/// under the same name, only the newer version is returned, and
/// soft-deleted snippets are omitted entirely. This endpoint does not
/// require device authentication.
#[utoipa::path(
	get,
	path = "/snippets",
	tag = "bestool",
	responses(
		(status = 200, description = "Current bestool snippets, keyed by name.", body = BTreeMap<String, SnippetResponse>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
#[axum::debug_handler]
async fn list_snippets(State(db): State<Db>) -> Result<Json<BTreeMap<String, SnippetResponse>>> {
	let mut conn = db.get().await?;

	let snippets = database::BestoolSnippet::list_current(&mut conn, 0, i64::MAX).await?;

	let response = snippets
		.into_iter()
		.map(|s| {
			(
				s.name,
				SnippetResponse {
					description: s.description,
					sql: s.sql,
				},
			)
		})
		.collect();

	Ok(Json(response))
}

#[cfg(test)]
mod tests {
	use super::*;
	use axum::http::StatusCode;
	use commons_tests::server;
	use database::BestoolSnippet;

	#[tokio::test(flavor = "multi_thread")]
	async fn test_list_snippets_empty() {
		server::run(|_conn, public_server, _private_server| async move {
			let response = public_server.get("/bestool/snippets").await;

			assert_eq!(response.status_code(), StatusCode::OK);
			let body: BTreeMap<String, SnippetResponse> = response.json();
			assert!(body.is_empty());
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_list_snippets_with_items() {
		server::run(|mut conn, public_server, _private_server| async move {
			// Create a snippet
			BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"test_snippet".to_string(),
				Some("A test snippet".to_string()),
				"SELECT 1".to_string(),
				None,
			)
			.await
			.unwrap();

			let response = public_server.get("/bestool/snippets").await;

			assert_eq!(response.status_code(), StatusCode::OK);
			let body: BTreeMap<String, SnippetResponse> = response.json();
			assert_eq!(body.len(), 1);
			assert!(body.contains_key("test_snippet"));

			let snippet = &body["test_snippet"];
			assert_eq!(snippet.description, Some("A test snippet".to_string()));
			assert_eq!(snippet.sql, "SELECT 1");
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_list_snippets_excludes_superseded() {
		server::run(|mut conn, public_server, _private_server| async move {
			// Create original snippet
			let original = BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"test_snippet".to_string(),
				Some("Original".to_string()),
				"SELECT 1".to_string(),
				None,
			)
			.await
			.unwrap();

			// Create superseding snippet
			BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"test_snippet".to_string(),
				Some("Updated".to_string()),
				"SELECT 2".to_string(),
				Some(original.id),
			)
			.await
			.unwrap();

			let response = public_server.get("/bestool/snippets").await;

			assert_eq!(response.status_code(), StatusCode::OK);
			let body: BTreeMap<String, SnippetResponse> = response.json();
			assert_eq!(body.len(), 1);

			let snippet = &body["test_snippet"];
			assert_eq!(snippet.description, Some("Updated".to_string()));
			assert_eq!(snippet.sql, "SELECT 2");
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_list_snippets_excludes_deleted() {
		server::run(|mut conn, public_server, _private_server| async move {
			// Create snippet
			let snippet = BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"test_snippet".to_string(),
				None,
				"SELECT 1".to_string(),
				None,
			)
			.await
			.unwrap();

			// Soft delete it
			BestoolSnippet::delete(&mut conn, snippet.id).await.unwrap();

			let response = public_server.get("/bestool/snippets").await;

			assert_eq!(response.status_code(), StatusCode::OK);
			let body: BTreeMap<String, SnippetResponse> = response.json();
			assert!(body.is_empty());
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn test_snippet_cannot_supersede_multiple() {
		server::run(|mut conn, _public_server, _private_server| async move {
			// Create first original snippet
			let original1 = BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"snippet1".to_string(),
				None,
				"SELECT 1".to_string(),
				None,
			)
			.await
			.unwrap();

			// Create second original snippet
			let _original2 = BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"snippet2".to_string(),
				None,
				"SELECT 2".to_string(),
				None,
			)
			.await
			.unwrap();

			// Create first superseding snippet - should work
			let _supersede1 = BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"snippet1_v2".to_string(),
				None,
				"SELECT 1 UPDATED".to_string(),
				Some(original1.id),
			)
			.await
			.unwrap();

			// Try to create another snippet that supersedes the same original - should fail
			let result = BestoolSnippet::create(
				&mut conn,
				"test@example.com".to_string(),
				"snippet1_v3".to_string(),
				None,
				"SELECT 1 UPDATED AGAIN".to_string(),
				Some(original1.id),
			)
			.await;

			// Should get a database constraint error
			assert!(
				result.is_err(),
				"Should not allow a snippet to be superseded by multiple snippets"
			);
		})
		.await
	}
}
