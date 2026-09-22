use std::time::{Duration, Instant};

use axum::Json;
use axum::extract::State;
use bestool_postgres::error::format_db_error;
use bestool_postgres::stringify::postgres_to_json_value;
use bestool_postgres::text_cast::{CellRef, TextCaster};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleUser;
use database::sql_playground_history::SqlPlaygroundHistory;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::fns::Page;
use crate::state::AppState;

/// A single SQL statement to run, as submitted to (or replayed from) the
/// read-only SQL playground.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SqlQuery {
	/// The SQL text to execute.
	pub query: String,
}

/// The result of running a query in the read-only SQL playground.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SqlResult {
	/// Column names, in the order they appear in each row. Empty if the
	/// query returned no rows.
	pub columns: Vec<String>,
	/// The result rows. Each row is a list of values positionally
	/// aligned with `columns`. Most Postgres types are converted to
	/// their natural JSON equivalent (numbers, booleans, strings,
	/// timestamps as strings, nested JSON, arrays). Types without a
	/// natural JSON equivalent — money, ranges, and geometric types, for
	/// example — are rendered as their text representation instead, and
	/// so is a genuine SQL NULL, which appears here as the literal
	/// string `"NULL"` rather than a JSON null. A JSON `null` in this
	/// data means the value couldn't be converted at all.
	pub rows: Vec<Vec<Value>>,
	/// Number of rows returned.
	pub row_count: usize,
	/// How long the query took to run, in milliseconds. Timing starts
	/// immediately before the query is sent and stops once every value
	/// has been read back, including any text conversions.
	///
	/// Endpoint overhead is deliberately outside the window: both pool
	/// checkouts, the write to the shared history, and the transaction
	/// setup. Those are not what the operator is measuring when they time
	/// a query, and the read-only pool's checkout in particular can
	/// dominate under contention.
	pub execution_time_ms: u64,
}

/// One entry in the shared history of queries run in the SQL playground.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct SqlHistoryEntry {
	/// Unique identifier for this history entry.
	pub id: Uuid,
	/// The SQL text that was executed.
	pub query: String,
	/// Tailscale login of the user who ran the query.
	pub tailscale_user: String,
	/// When the query was executed.
	pub created_at: Timestamp,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: is_sql_available))
		.routes(routes!(read_only: execute_query))
		.routes(routes!(read_only: get_last_user_query))
		.routes(routes!(read_only: get_query_history))
}

/// Check whether the read-only SQL playground is enabled.
///
/// Returns `true` if this server is configured with a read-only database
/// connection to run playground queries against, `false` otherwise. Does
/// not require authentication.
#[utoipa::path(
	post,
	path = "/is_sql_available",
	tag = "sql",
	responses(
		(status = 200, description = "Whether the read-only SQL playground is configured.", body = bool, content_type = "application/json"),
	),
)]
pub async fn is_sql_available(State(state): State<AppState>) -> Json<bool> {
	Json(state.ro_pool.is_some())
}

/// Request body for running a query in the SQL playground.
#[derive(Deserialize, ToSchema)]
pub struct ExecuteArgs {
	/// The query to execute.
	pub query: SqlQuery,
}

/// Run a query in the read-only SQL playground.
///
/// The query runs inside an explicit read-only transaction — Postgres
/// rejects write statements within it — and the transaction is always
/// rolled back afterwards regardless of the outcome, so nothing it does
/// can be committed. Execution is capped at 60 seconds; a query that runs
/// longer is aborted and reported as an error. The query text is recorded
/// to the shared query history under the caller's identity before it
/// runs, even if execution subsequently fails.
///
/// Returns an error if the read-only SQL playground isn't configured on
/// this server, or if the query fails, times out, or can't be recorded to
/// history.
#[utoipa::path(
	post,
	path = "/execute_query",
	tag = "sql",
	security(("tailscale-user" = [])),
	request_body = ExecuteArgs,
	responses(
		(status = 200, body = SqlResult),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn execute_query(
	State(state): State<AppState>,
	user: TailscaleUser,
	Json(args): Json<ExecuteArgs>,
) -> Result<Json<SqlResult>> {
	let Some(ro_pool) = state.ro_pool.clone() else {
		return Err(AppError::custom(
			"SQL functionality is disabled (RO_DATABASE_URL not set)",
		));
	};

	let query = args.query;

	let mut conn = state.db.get().await?;
	SqlPlaygroundHistory::create(&mut conn, query.query.clone(), user.login.clone())
		.await
		.map_err(|e| AppError::custom(format!("Failed to record query history: {}", e)))?;

	let mut client = ro_pool
		.get()
		.await
		.map_err(|e| AppError::custom(format!("Failed to get connection: {}", e)))?;

	let transaction = client
		.build_transaction()
		.read_only(true)
		.start()
		.await
		.map_err(|e| AppError::custom(format_db_error(&e, None)))?;

	transaction
		.execute("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY", &[])
		.await
		.map_err(|e| {
			AppError::custom(format_db_error(
				&e,
				Some("SET SESSION CHARACTERISTICS AS TRANSACTION READ ONLY"),
			))
		})?;

	// The clock starts here, not before the pool checkouts, the history
	// write, and the transaction setup. Those are endpoint overhead, and
	// under contention for the read-only pool they dominated what was
	// reported as the query's own time.
	let start_time = Instant::now();

	let rows = tokio::time::timeout(
		Duration::from_secs(60),
		transaction.query(&query.query, &[]),
	)
	.await
	.map_err(|_| AppError::custom("Query execution timed out after 60 seconds"))?
	.map_err(|e| AppError::custom(format_db_error(&e, Some(&query.query))))?;

	transaction
		.rollback()
		.await
		.map_err(|e| AppError::custom(format_db_error(&e, None)))?;

	if rows.is_empty() {
		return Ok(Json(SqlResult {
			columns: Vec::new(),
			rows: Vec::new(),
			row_count: 0,
			execution_time_ms: start_time.elapsed().as_millis() as u64,
		}));
	}

	let first_row = &rows[0];
	let columns: Vec<String> = first_row
		.columns()
		.iter()
		.map(|col| col.name().to_string())
		.collect();

	let mut null_cells = Vec::new();
	let mut all_values = Vec::new();
	for (row_idx, row) in rows.iter().enumerate() {
		let mut row_values = Vec::with_capacity(columns.len());
		for col_idx in 0..columns.len() {
			let value = postgres_to_json_value(row, col_idx);
			row_values.push(value);
			if let Value::Null = &row_values[col_idx] {
				null_cells.push(CellRef { row_idx, col_idx });
			}
		}
		all_values.push(row_values);
	}

	if !null_cells.is_empty() {
		let text_caster = TextCaster::new(ro_pool.clone());
		let text_results = text_caster.cast_batch(&rows, &null_cells).await;
		for (cell_ref, text_result) in null_cells.iter().zip(text_results) {
			match text_result {
				Ok(text) => {
					all_values[cell_ref.row_idx][cell_ref.col_idx] = Value::String(text);
				}
				Err(_) => {
					all_values[cell_ref.row_idx][cell_ref.col_idx] = Value::Null;
				}
			}
		}
	}

	// After the text conversions, which the field documents as included —
	// they are a second round-trip and the operator is paying for them.
	Ok(Json(SqlResult {
		columns,
		rows: all_values,
		row_count: rows.len(),
		execution_time_ms: start_time.elapsed().as_millis() as u64,
	}))
}

/// Get the caller's most recently run playground query.
///
/// Returns the SQL text of the last query the caller executed in the SQL
/// playground, or `null` if they haven't run one yet.
#[utoipa::path(
	post,
	path = "/get_last_user_query",
	tag = "sql",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, description = "The caller's most recent SQL playground query, if any.", body = Option<String>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get_last_user_query(
	State(state): State<AppState>,
	user: TailscaleUser,
) -> Result<Json<Option<String>>> {
	let mut conn = state.db.get().await?;
	let last = SqlPlaygroundHistory::get_last_by_user(&mut conn, &user.login)
		.await?
		.map(|entry| entry.query);
	Ok(Json(last))
}

/// Pagination parameters for browsing the shared query history.
#[derive(Deserialize, ToSchema)]
pub struct HistoryArgs {
	/// Number of history entries to skip before the returned page
	/// starts.
	pub offset: u64,
	/// Maximum number of entries to return; defaults to 10 if omitted.
	pub limit: Option<u64>,
}

/// List the shared history of SQL playground queries.
///
/// Returns a page of every query run by any user in the SQL playground,
/// most recent first, together with the total number of entries.
#[utoipa::path(
	post,
	path = "/get_query_history",
	tag = "sql",
	request_body = HistoryArgs,
	responses(
		(status = 200, body = Page<SqlHistoryEntry>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get_query_history(
	State(state): State<AppState>,
	Json(args): Json<HistoryArgs>,
) -> Result<Json<Page<SqlHistoryEntry>>> {
	let limit = args.limit.unwrap_or(10) as i64;
	let offset = args.offset as i64;
	let mut conn = state.db.get().await?;
	let total = SqlPlaygroundHistory::count_all(&mut conn)
		.await?
		.try_into()
		.unwrap_or(0);
	let items = SqlPlaygroundHistory::get_paginated(&mut conn, offset, limit)
		.await?
		.into_iter()
		.map(|entry| SqlHistoryEntry {
			id: entry.id,
			query: entry.query,
			tailscale_user: entry.tailscale_user,
			created_at: entry.created_at,
		})
		.collect();
	Ok(Json(Page { items, total }))
}
