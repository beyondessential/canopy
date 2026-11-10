//! Helpers shared across more than one tool module: JSON result shaping,
//! id/enum parsing, and lookups used by several domains. Single-domain
//! helpers stay local to the module that uses them.

use std::collections::HashMap;

use commons_types::Uuid;
use database::{diesel_async::AsyncPgConnection, server_groups::ServerGroup};
use jiff::{SignedDuration, Timestamp};
use rmcp::model::{CallToolResult, ContentBlock, ErrorData as McpError};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, JsonSchema)]
pub(crate) struct EmptyArgs {}

/// Serialize a payload into a tool result, providing both the structured
/// content (for clients that read it) and a pretty-printed text fallback.
pub(crate) fn ok_json<T: Serialize>(value: &T) -> Result<CallToolResult, McpError> {
	let json = serde_json::to_value(value).map_err(mcp_err)?;
	let text = serde_json::to_string_pretty(&json).map_err(mcp_err)?;
	let mut result = CallToolResult::success(vec![ContentBlock::text(text)]);
	result.structured_content = Some(json);
	Ok(result)
}

/// A "ran successfully but found nothing" result the caller's client renders.
pub(crate) fn not_found(message: String) -> CallToolResult {
	CallToolResult::error(vec![ContentBlock::text(message)])
}

/// Whether an error is a record that was not there, however it arrived.
pub(crate) fn is_not_found(error: &commons_errors::AppError) -> bool {
	error.is_not_found()
}

/// Map any internal/db error into an MCP protocol error.
pub(crate) fn mcp_err(e: impl std::fmt::Display) -> McpError {
	McpError::internal_error(e.to_string(), None)
}

pub(crate) fn parse_opt<T: std::str::FromStr>(
	v: &Option<String>,
	field: &str,
) -> Result<Option<T>, McpError> {
	match v.as_deref() {
		Some(s) => s
			.parse::<T>()
			.map(Some)
			.map_err(|_| McpError::invalid_params(format!("invalid {field}: {s}"), None)),
		None => Ok(None),
	}
}

pub(crate) fn parse_uuid(s: &str, field: &str) -> Result<Uuid, McpError> {
	Uuid::parse_str(s).map_err(|_| McpError::invalid_params(format!("invalid {field}: {s}"), None))
}

pub(crate) fn parse_opt_uuid(v: &Option<String>, field: &str) -> Result<Option<Uuid>, McpError> {
	v.as_deref().map(|s| parse_uuid(s, field)).transpose()
}

/// Deduplicate a stream of ids, preserving first-seen order.
pub(crate) fn unique(it: impl IntoIterator<Item = Uuid>) -> Vec<Uuid> {
	let mut seen = std::collections::HashSet::new();
	it.into_iter().filter(|x| seen.insert(*x)).collect()
}

pub(crate) async fn group_names(
	conn: &mut AsyncPgConnection,
	ids: &[Uuid],
) -> Result<HashMap<Uuid, String>, McpError> {
	let groups = ServerGroup::list_by_ids(conn, ids).await.map_err(mcp_err)?;
	Ok(groups.into_iter().map(|g| (g.id, g.name)).collect())
}

/// A timestamp `days` ago (clamped to a decade), for recency windows.
pub(crate) fn since_from_days(days: u32) -> Timestamp {
	let days = days.min(3650) as i64;
	Timestamp::now() - SignedDuration::from_hours(24 * days)
}
