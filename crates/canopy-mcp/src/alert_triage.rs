//! The guidance an agent triages a Canopy incident notification by.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Alert triage",
//! and `.workhorse/specs/monitoring/alert-triage.md` (id `TRI`).
//!
//! Canopy makes no triage decision and keeps no triage record of its own: the
//! agent reads the incident through the other tools and records its verdict
//! as an incident note. This module only serves the guidance, kept as markdown
//! beside it so the triage policy is revised in this repository.

use rmcp::{
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use serde::Serialize;

use crate::{CanopyMcp, util::ok_json};

// spec: TRI#how-the-agent-decides
pub const GUIDANCE: &str = include_str!("alert_triage_guidance.md");

#[derive(Serialize)]
struct GuidanceOut {
	guidance: &'static str,
}

#[tool_router(router = alert_triage_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "The guidance for triaging a Canopy incident notification: what bears on \
		               whether it is escalated or logged, what to read, how to record the verdict \
		               as an incident note, and how to report it. Read it before triaging an \
		               alert, and follow it."
	)]
	async fn get_alert_triage_guidance(&self) -> Result<CallToolResult, McpError> {
		ok_json(&GuidanceOut { guidance: GUIDANCE })
	}
}
