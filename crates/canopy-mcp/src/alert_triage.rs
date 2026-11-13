//! Tools carrying alert triage: what an agent reads to triage an incident
//! notification, the guidance it decides by, and the verdicts it records.
//!
//! Spec: `.workhorse/specs/private-server/mcp.md` (id `MCP`), "Alert triage",
//! and `.workhorse/specs/monitoring/alert-triage.md` (id `TRI`).
//!
//! Canopy makes no triage decision: it gathers the incident's context, serves
//! the guidance in `alert_triage_guidance.md`, and keeps the record.

use commons_errors::AppError;
use commons_types::{Uuid, server::rank::ServerRank, status::CheckResult};
use database::{
	alert_triage::{self, Changes, NewVerdict, Recorded, Severity, Verdict, VerdictRow},
	audit::{self, Action, Record},
	diesel_async::AsyncConnection,
	memory::{self, MemoryEntryData},
	notes::IncidentNote,
	stability::{CheckStability, StabilityData},
};
use jiff::{Timestamp, civil};
use rmcp::{
	handler::server::{tool::Extension, wrapper::Parameters},
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

use crate::{
	CanopyMcp,
	incidents::{IssueScopeOut, scope_labels},
	notes::{NOTES_LIMIT, NoteOut, refusal, require_notes},
	util::{is_not_found, mcp_err, not_found, ok_json, parse_uuid},
};

/// What the agent decides and reports by, served with every triage. Kept as
/// markdown beside this module so it can be revised without touching the code.
// spec: TRI#how-the-agent-decides
pub const GUIDANCE: &str = include_str!("alert_triage_guidance.md");

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TriageAlertArgs {
	/// The incident's id: the UUID at the end of the notification's link to
	/// it. Read it from the link; never fetch the link.
	pub incident_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum VerdictArg {
	Escalated,
	Logged,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RecordAlertTriageArgs {
	/// The incident's id.
	pub incident_id: String,
	/// The `revision` triage_alert returned.
	pub revision: String,
	/// `escalated` or `logged`.
	pub verdict: VerdictArg,
	/// Why, in a few lines (at most 4,000 characters). Shown on the incident as
	/// your note.
	pub reasoning: String,
	/// The memory entries the verdict relies on, such as one naming a pattern
	/// as harmless.
	pub memory_entry_ids: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct AttachAlertTriageReportArgs {
	/// The verdict's id, as record_alert_triage returned it.
	pub verdict_id: String,
	/// A link to where the verdict was reported: the escalation post, or the
	/// write-up reply.
	pub link: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindAlertTriageVerdictsArgs {
	/// The write-up day, as `YYYY-MM-DD`, naming the day it begins on at 8am
	/// New Zealand time. Defaults to the current write-up day.
	pub date: Option<String>,
}

#[derive(Serialize)]
struct IncidentOut {
	id: Uuid,
	target: String,
	group_id: Option<Uuid>,
	rank: Option<ServerRank>,
	status: &'static str,
	opened_at: Timestamp,
	closed_at: Option<Timestamp>,
	resolved_at: Option<Timestamp>,
	resolved_by: Option<String>,
}

#[derive(Serialize)]
struct LiveIssueOut {
	issue_id: Uuid,
	source: String,
	check_name: String,
	effective_result: Option<CheckResult>,
	escalates: bool,
	headline: Option<String>,
	message: String,
	scope: IssueScopeOut,
	degraded_since: Option<Timestamp>,
}

#[derive(Serialize)]
struct CheckOut {
	source: String,
	check_name: String,
	escalates: bool,
	failing_since: Option<Timestamp>,
	documentation: Option<String>,
	stability: Vec<IssueStabilityOut>,
}

#[derive(Serialize)]
struct IssueStabilityOut {
	issue_id: Uuid,
	stability: StabilityData,
}

#[derive(Serialize)]
struct AlreadyTriagedOut {
	verdict_id: Uuid,
	verdict: Verdict,
	recorded_at: Timestamp,
	report_link: Option<String>,
	reasoning: String,
}

#[derive(Serialize)]
struct TriageOut {
	incident: IncidentOut,
	revision: String,
	worst_result: Option<Severity>,
	already_triaged: Option<AlreadyTriagedOut>,
	/// What has changed since the latest verdict; null before the first.
	changes: Option<Changes>,
	live_issues: Vec<LiveIssueOut>,
	checks: Vec<CheckOut>,
	memory: Vec<MemoryEntryData>,
	verdicts: Vec<VerdictRow>,
	notes: Vec<NoteOut>,
	guidance: &'static str,
}

#[derive(Serialize)]
struct RecordedOut {
	verdict: VerdictRow,
	note_id: Uuid,
}

fn status_of(incident: &database::issues::Incident) -> &'static str {
	if incident.resolved_at.is_some() {
		"resolved"
	} else if incident.closed_at.is_some() {
		"closed"
	} else {
		"open"
	}
}

fn refused(error: AppError) -> Result<CallToolResult, McpError> {
	match error {
		AppError::BadRequest(message) => Err(McpError::invalid_params(message, None)),
		error if is_not_found(&error) => Ok(not_found("not found".into())),
		error => Err(mcp_err(error)),
	}
}

#[tool_router(router = alert_triage_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		annotations(read_only_hint = true),
		description = "Triage a Canopy incident notification. Call this for every incident \
		               notification before deciding anything, with the incident id read from \
		               the notification's link (never fetch the link). Returns, in one result: \
		               the incident and its live issues; `revision`, to give back when recording; \
		               `already_triaged` when this state has been triaged before; `changes` since \
		               its last triage; each failing check's documentation and stability; current \
		               memory entries for its checks, group and incident; its earlier verdicts \
		               and notes; and `guidance`, which you decide and report by. You make the \
		               decision. Reads only."
	)]
	async fn triage_alert(
		&self,
		Parameters(args): Parameters<TriageAlertArgs>,
	) -> Result<CallToolResult, McpError> {
		let incident_id = parse_uuid(&args.incident_id, "incident_id")?;
		let mut conn = self.conn().await?;
		let assessment = match alert_triage::assess(&mut conn, incident_id).await {
			Ok(assessment) => assessment,
			Err(error) if is_not_found(&error) => {
				return Ok(not_found(format!("no incident with id {incident_id}")));
			}
			Err(error) => return Err(mcp_err(error)),
		};

		let live: Vec<&database::issues::Issue> = assessment
			.rows
			.iter()
			.filter(|(link, _)| link.left_at.is_none())
			.map(|(_, issue)| issue)
			.collect();
		let (applications, machines, groups, clusters) = scope_labels(&mut conn, &live).await?;
		let live_issues = live
			.iter()
			.map(|issue| LiveIssueOut {
				issue_id: issue.id,
				source: issue.source.clone(),
				check_name: issue
					.check_name
					.clone()
					.unwrap_or_else(|| issue.r#ref.clone()),
				effective_result: issue.effective_result,
				escalates: issue.escalates,
				headline: issue.description.clone().or_else(|| issue.title.clone()),
				message: issue.message.clone(),
				scope: IssueScopeOut::of(issue, &applications, &machines, &groups, &clusters),
				degraded_since: issue.degraded_since,
			})
			.collect();

		let issue_ids: Vec<Uuid> = assessment
			.checks
			.iter()
			.flat_map(|c| c.issue_ids.iter().copied())
			.collect();
		let stability = CheckStability::for_issue_ids(&mut conn, &issue_ids)
			.await
			.map_err(mcp_err)?;
		let now = Timestamp::now();
		let checks = assessment
			.checks
			.iter()
			.map(|check| CheckOut {
				source: check.check.source.clone(),
				check_name: check.check.name.clone(),
				escalates: check.escalates,
				failing_since: check.began,
				documentation: check.documentation.clone(),
				stability: check
					.issue_ids
					.iter()
					.filter_map(|id| {
						stability.get(id).map(|row| IssueStabilityOut {
							issue_id: *id,
							stability: StabilityData::from_row(row, now),
						})
					})
					.collect(),
			})
			.collect();

		let memory = memory::present(&mut conn, assessment.memory.clone())
			.await
			.map_err(mcp_err)?;
		let notes = IncidentNote::list_for_incident(&mut conn, incident_id, NOTES_LIMIT, false)
			.await
			.map_err(mcp_err)?
			.into_iter()
			.map(NoteOut::from)
			.collect();
		let incident = &assessment.incident;

		ok_json(&TriageOut {
			incident: IncidentOut {
				id: incident.id,
				target: assessment.target.clone(),
				group_id: incident.server_group_id,
				rank: incident.rank,
				status: status_of(incident),
				opened_at: incident.opened_at,
				closed_at: incident.closed_at,
				resolved_at: incident.resolved_at,
				resolved_by: incident.resolved_by.clone(),
			},
			revision: assessment.state.revision.clone(),
			worst_result: assessment.state.worst,
			already_triaged: assessment
				.already_triaged
				.as_ref()
				.map(|row| AlreadyTriagedOut {
					verdict_id: row.id,
					verdict: row.verdict,
					recorded_at: row.created_at,
					report_link: row.report_link.clone(),
					reasoning: row.reasoning.clone(),
				}),
			changes: assessment.changes.clone(),
			live_issues,
			checks,
			memory,
			verdicts: assessment.verdicts.clone(),
			notes,
			guidance: GUIDANCE,
		})
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Record your alert triage verdict on an incident, before reporting it. Give \
		               the `revision` triage_alert returned, `escalated` or `logged`, the \
		               `reasoning` (shown on the incident as your note), and any \
		               `memory_entry_ids` it relies on. Refused, with no effect, when the \
		               incident changed since the revision: call triage_alert again. Returns the \
		               verdict; then attach where you reported it with \
		               attach_alert_triage_report. Needs an administrator, or a token with the \
		               notes grant."
	)]
	async fn record_alert_triage(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<RecordAlertTriageArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let incident_id = parse_uuid(&args.incident_id, "incident_id")?;
		let memory_entry_ids = args
			.memory_entry_ids
			.unwrap_or_default()
			.iter()
			.map(|id| parse_uuid(id, "memory_entry_ids"))
			.collect::<Result<Vec<_>, _>>()?;
		let new = NewVerdict {
			incident_id,
			revision: args.revision,
			verdict: match args.verdict {
				VerdictArg::Escalated => Verdict::Escalated,
				VerdictArg::Logged => Verdict::Logged,
			},
			reasoning: args.reasoning,
			memory_entry_ids,
		};
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let recorded =
					alert_triage::record(conn, &new, caller.name(), caller.note_author_kind())
						.await?;
				if let Recorded::Done { verdict, note } = &recorded {
					let incident = Record::incident(conn, incident_id).await?;
					let note_record = Record::note(note.id, &note.author);
					audit::record(
						conn,
						&actor,
						&Action::IncidentNoteAdded {
							incident: incident.clone(),
							note: note_record.clone(),
							author_kind: note.author_kind.as_str().to_owned(),
						},
					)
					.await?;
					audit::record(
						conn,
						&actor,
						&Action::AlertTriageRecorded {
							incident,
							note: note_record,
							verdict_id: verdict.id,
							verdict: verdict.verdict.as_str().to_owned(),
						},
					)
					.await?;
				}
				Ok(recorded)
			})
			.await;
		match result {
			Ok(Recorded::Done { verdict, note }) => ok_json(&RecordedOut {
				verdict: *verdict,
				note_id: note.id,
			}),
			// No fresh revision is handed back: the agent reads the incident
			// again before deciding, rather than recording against a state it
			// has not seen.
			Ok(Recorded::Stale) => Ok(refusal(
				"the incident changed since triage_alert read it; call triage_alert again",
				json!(null),
			)),
			Err(error) if is_not_found(&error) => {
				Ok(not_found(format!("no incident with id {incident_id}")))
			}
			Err(error) => refused(error),
		}
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = true
		),
		description = "Attach where an alert triage verdict was reported (the escalation post or \
		               the write-up reply) to the verdict record_alert_triage returned, so a \
		               later triage of the same incident can link to it. Needs an administrator, \
		               or a token with the notes grant."
	)]
	async fn attach_alert_triage_report(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<AttachAlertTriageReportArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_notes(parts).await?;
		let verdict_id = parse_uuid(&args.verdict_id, "verdict_id")?;
		let actor = caller.actor();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let result = conn
			.transaction::<_, AppError, _>(async |conn| {
				let verdict = alert_triage::attach_report(conn, verdict_id, &args.link).await?;
				let incident = Record::incident(conn, verdict.incident_id).await?;
				audit::record(
					conn,
					&actor,
					&Action::AlertTriageReported {
						incident,
						verdict_id,
						link: verdict.report_link.clone().unwrap_or_default(),
					},
				)
				.await?;
				Ok(verdict)
			})
			.await;
		match result {
			Ok(verdict) => ok_json(&verdict),
			Err(error) if is_not_found(&error) => {
				Ok(not_found(format!("no verdict with id {verdict_id}")))
			}
			Err(error) => refused(error),
		}
	}

	#[tool(
		annotations(read_only_hint = true),
		description = "The alert triage verdicts recorded in a write-up day, oldest first, each \
		               with its incident's target, verdict and where it was reported, and the \
		               incidents published that day with no verdict. A write-up day runs from \
		               8am New Zealand time to 8am the next day; `date` (YYYY-MM-DD) names the \
		               day it begins on, defaulting to the current one. Draw the morning summary \
		               from this: how many were escalated and how many logged, which incidents \
		               are still open, and which published incidents were never triaged."
	)]
	async fn find_alert_triage_verdicts(
		&self,
		Parameters(args): Parameters<FindAlertTriageVerdictsArgs>,
	) -> Result<CallToolResult, McpError> {
		let date = match args.date.as_deref() {
			Some(text) => text.parse::<civil::Date>().map_err(|_| {
				McpError::invalid_params(format!("invalid date: {text} (YYYY-MM-DD)"), None)
			})?,
			None => alert_triage::write_up_date(Timestamp::now()).map_err(mcp_err)?,
		};
		let mut conn = self.conn().await?;
		let day = alert_triage::find_day(&mut conn, date)
			.await
			.map_err(mcp_err)?;
		let escalated = day
			.verdicts
			.iter()
			.filter(|v| v.verdict.verdict == Verdict::Escalated)
			.count();
		let mut out = serde_json::to_value(&day).map_err(mcp_err)?;
		out["escalated_count"] = json!(escalated);
		out["logged_count"] = json!(day.verdicts.len() - escalated);
		ok_json(&out)
	}
}
