//! `find_vulnerabilities`, `get_vulnerability` and `record_vulnerability_decision`.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Vulnerabilities".

use database::{
	audit,
	diesel_async::AsyncConnection,
	osv::Severity,
	vulnerabilities::{
		Decision, ItemDetail, ItemSummary, ListFilter, NewDecision, Outcome, Source, Status, Via,
		today,
	},
};
use rmcp::{
	handler::server::{tool::Extension, wrapper::Parameters},
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::Deserialize;

use crate::{
	CanopyMcp, caller,
	util::{mcp_err, not_found, ok_json},
};

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 200;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindVulnerabilitiesArgs {
	/// Only items of this source: `tamanu` (dependencies of Tamanu versions) or
	/// `linux` (packages installed on machines).
	pub source: Option<String>,
	/// Only items with this triage status: `open`, `not_affected`,
	/// `accepted_risk` or `will_fix`. Omitted, every status.
	pub status: Option<String>,
	/// Only items at or above this severity: `low`, `medium`, `high` or
	/// `critical`.
	pub min_severity: Option<String>,
	/// Include items nothing counts for any more. Defaults to false.
	pub include_uncounted: Option<bool>,
	/// The most items to return, at most 200. Defaults to 50.
	pub limit: Option<u32>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GetVulnerabilityArgs {
	/// The advisory's OSV identifier, such as `GHSA-xxxx-xxxx-xxxx`.
	pub id: String,
	/// `tamanu` or `linux`.
	pub source: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordVulnerabilityDecisionArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// `tamanu` or `linux`.
	pub source: String,
	/// `not_affected`, `accepted_risk` or `will_fix`.
	pub outcome: String,
	/// Why. Required for `not_affected` and `accepted_risk`.
	pub reason: Option<String>,
	/// When to look at an accepted risk again, as `YYYY-MM-DD`, in the future.
	/// Required for `accepted_risk`.
	pub review_by: Option<String>,
	/// A link to the issue tracking the fix, for `will_fix`.
	pub issue_url: Option<String>,
}

fn invalid(what: &str, value: &str) -> McpError {
	McpError::invalid_params(format!("invalid {what}: {value}"), None)
}

fn source(value: &str) -> Result<Source, McpError> {
	Source::parse(value).ok_or_else(|| invalid("source", value))
}

#[tool_router(router = vulnerabilities_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		annotations(read_only_hint = true),
		description = "Vulnerabilities in what the production fleet runs, as the operator \
		               interface lists them: one item per OSV advisory within a source, sorted \
		               by severity and then by how much each exposes. Source `tamanu` is the \
		               dependencies of Tamanu versions, `linux` the packages installed on \
		               machines. Each item has its identifier and aliases (such as a CVE), \
		               summary, severity, affected packages, how many applications (tamanu) or \
		               machines (linux) it exposes, its triage status with the current decision, \
		               why it reopened if it did, and the model's suggestion if there is one. \
		               Only production counts as exposure. Items nothing counts for any more are \
		               left out unless include_uncounted is set. When more match than limit, \
		               `truncated` is true."
	)]
	async fn find_vulnerabilities(
		&self,
		Parameters(args): Parameters<FindVulnerabilitiesArgs>,
	) -> Result<CallToolResult, McpError> {
		let filter = ListFilter {
			source: args.source.as_deref().map(source).transpose()?,
			status: args
				.status
				.as_deref()
				.map(|s| Status::parse(s).ok_or_else(|| invalid("status", s)))
				.transpose()?,
			min_severity: args
				.min_severity
				.as_deref()
				.map(|s| {
					s.parse::<Severity>()
						.ok()
						.filter(|s| *s != Severity::Unrated)
						.ok_or_else(|| invalid("min_severity", s))
				})
				.transpose()?,
			include_uncounted: args.include_uncounted.unwrap_or(false),
			limit: Some(args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT) as usize),
		};
		let mut conn = self.conn().await?;
		let listing = ItemSummary::list(&mut conn, &filter, today())
			.await
			.map_err(mcp_err)?;
		ok_json(&listing)
	}

	#[tool(
		annotations(read_only_hint = true),
		description = "One vulnerability in full: the advisory's details and affected ranges with \
		               the versions that fix it; for a tamanu item, each exposed Tamanu version \
		               with the package version it carries, the workspaces that reach the \
		               package, whether any depends on it directly, and how many applications run \
		               it; for a linux item, each exposed machine with its group and the \
		               package version installed; the decision history, with reopenings; and the \
		               model's suggestion with its reasoning."
	)]
	async fn get_vulnerability(
		&self,
		Parameters(args): Parameters<GetVulnerabilityArgs>,
	) -> Result<CallToolResult, McpError> {
		let source = source(&args.source)?;
		let mut conn = self.conn().await?;
		match ItemDetail::load(&mut conn, &args.id, source, today()).await {
			Ok(detail) => ok_json(&detail),
			Err(commons_errors::AppError::NotFound(message)) => Ok(not_found(message)),
			Err(e) => Err(mcp_err(e)),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Record a triage decision on a vulnerability, as its current decision across \
		               its whole source: `not_affected` with a reason, `accepted_risk` with a \
		               reason and a future review date (`review_by`, YYYY-MM-DD), or `will_fix` \
		               with an optional reason and issue link. It may replace any decision, an \
		               operator's included, and the item's history keeps the one it replaced. The \
		               decision is recorded as made through this interface by the caller. \
		               Refused, with no effect, unless the caller is an administrator (on the \
		               operator surface) or holds a token with the triage grant."
	)]
	async fn record_vulnerability_decision(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<RecordVulnerabilityDecisionArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = caller::of(parts)?;
		if !caller.may_triage().await.map_err(mcp_err)? {
			return Err(McpError::invalid_request(
				"recording a triage decision needs an administrator, or a token with the triage grant",
				None,
			));
		}

		let source = source(&args.source)?;
		let outcome =
			Outcome::parse(&args.outcome).ok_or_else(|| invalid("outcome", &args.outcome))?;
		let review_by = args
			.review_by
			.as_deref()
			.filter(|date| !date.trim().is_empty())
			.map(|date| {
				date.trim()
					.parse()
					.map_err(|_| invalid("review_by (expected YYYY-MM-DD)", date))
			})
			.transpose()?;

		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let decision = conn
			.transaction::<_, commons_errors::AppError, _>(async |conn| {
				let decision = Decision::record(
					conn,
					NewDecision {
						vulnerability_id: args.id.clone(),
						source,
						outcome,
						reason: args.reason,
						review_by,
						issue_url: args.issue_url,
						ground: None,
						actor: actor.clone(),
						via: Via::Mcp,
					},
					today(),
				)
				.await?;
				audit::record(conn, &actor, &decision.audited()).await?;
				Ok(decision)
			})
			.await;
		match decision {
			Ok(decision) => ok_json(&decision),
			Err(commons_errors::AppError::BadRequest(message)) => {
				Err(McpError::invalid_params(message, None))
			}
			Err(commons_errors::AppError::NotFound(message)) => Ok(not_found(message)),
			Err(e) => Err(mcp_err(e)),
		}
	}
}
