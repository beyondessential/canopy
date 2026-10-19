//! `find_audit_entries` tool.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Audit log".

use database::audit::{self, Entry, Filter, READ_WINDOW};
use jiff::{SignedDuration, Timestamp};
use rmcp::{
	handler::server::wrapper::Parameters,
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
	CanopyMcp,
	util::{mcp_err, ok_json},
};

/// How far back the tool reaches, in days. The same ninety days the operator
/// interface reads from the database, whatever an action is kept for.
const MAX_WINDOW_DAYS: u32 = 90;
const DEFAULT_WINDOW_DAYS: u32 = 7;
const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindAuditEntriesArgs {
	/// How many days back to look, at most 90. Defaults to 7. A longer window
	/// is refused.
	pub window_days: Option<u32>,
	/// Only entries by this actor: an operator's login, or a device's, token's
	/// or job's identifier.
	pub actor: Option<String>,
	/// Only entries of this action, by its stable name, such as
	/// `maintenance_declared`. Naming an action returns it whether or not it
	/// is routine.
	pub action: Option<String>,
	/// Only entries that acted on a record with this identifier, or whose
	/// record's name contained this.
	pub record: Option<String>,
	/// Include routine actions, which are taken at high volume and are left out
	/// unless asked for or named in `action`. Defaults to false.
	pub include_routine: Option<bool>,
	/// The most entries to return, at most 200. Defaults to 50.
	pub limit: Option<u32>,
}

#[derive(Serialize)]
struct AuditEntries {
	/// Entries newest first.
	entries: Vec<Entry>,
	/// Whether more entries match than were returned.
	truncated: bool,
}

#[tool_router(router = audit_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Reads the log; recording nothing itself (see the AUD spec).
		annotations(read_only_hint = true),
		description = "Entries of the audit log, newest first: who did what to which records. \
		               Each has when it happened, its actor (an operator, a device, an access \
		               token or Canopy's own job), its action, the records it acted on with \
		               their names at the time, and the action's detail. An entry written ahead \
		               of its action carries a status: done, failed, or unconfirmed, meaning it \
		               was never settled and may or may not have taken effect. Reaches back at \
		               most 90 days (window_days) and no further, even for entries Canopy keeps \
		               longer; older entries are read from the operator interface. Actions taken \
		               at high volume, such as issuing backup credentials or reading an \
		               inventory, are left out unless include_routine is set or the action is \
		               named."
	)]
	async fn find_audit_entries(
		&self,
		Parameters(args): Parameters<FindAuditEntriesArgs>,
	) -> Result<CallToolResult, McpError> {
		let window_days = args.window_days.unwrap_or(DEFAULT_WINDOW_DAYS);
		if window_days > MAX_WINDOW_DAYS {
			return Err(McpError::invalid_params(
				format!(
					"window_days is at most {MAX_WINDOW_DAYS}: this tool reaches back \
					 {MAX_WINDOW_DAYS} days and no further"
				),
				None,
			));
		}
		let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);

		let mut conn = self.conn().await?;
		let oldest = Timestamp::now() - READ_WINDOW;
		let since = (Timestamp::now() - SignedDuration::from_hours(24 * window_days as i64))
			.max(oldest);
		let record = args.record.filter(|record| !record.trim().is_empty());
		let mut entries = audit::find(
			&mut conn,
			&Filter {
				actor_id: args.actor,
				action: args.action,
				include_routine: args.include_routine.unwrap_or(false),
				record: record.clone(),
				record_name: record,
				since: Some(since),
				// One more than asked for, to say whether there is more.
				limit: limit as i64 + 1,
				..Default::default()
			},
		)
		.await
		.map_err(mcp_err)?;
		let truncated = entries.len() > limit as usize;
		entries.truncate(limit as usize);
		ok_json(&AuditEntries { entries, truncated })
	}
}
