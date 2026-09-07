//! `find_incidents` / `get_incident` / `find_issues` / `get_issue` tools.

use commons_types::{Uuid, server::rank::ServerRank, status::CheckResult};
use database::{
	applications::Application,
	diesel_async::AsyncPgConnection,
	issues::{Incident, IncidentStatusFilter, Issue, IssueListFilters},
	server_groups::ServerGroup,
	slack_outbox::SlackOutbox,
};
use jiff::Timestamp;
use rmcp::{
	handler::server::wrapper::Parameters,
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
	CanopyMcp,
	util::{
		group_names, mcp_err, not_found, ok_json, parse_opt_uuid, parse_uuid, since_from_days,
		unique,
	},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindIncidentsArgs {
	/// Look back this many days; returns incidents that were open at any point in
	/// the window (still open, or closed within it). Default 7.
	pub since_days: Option<u32>,
	/// Restrict to one group's id.
	pub group_id: Option<String>,
	/// Filter by status: `open` (not yet closed), `resolved` (operator-resolved),
	/// or `all` (default).
	pub status: Option<String>,
	/// Max incidents to return (default 100).
	pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IncidentIdArgs {
	/// The incident's id.
	pub incident_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindIssuesArgs {
	/// Only currently-active, unresolved issues. Default true.
	pub active_only: Option<bool>,
	/// Filter to issues whose latest effective result is one of these:
	/// `failed`, `warning`, `broken`, `passed`, `skipped`.
	pub results: Option<Vec<String>>,
	/// Restrict to issues whose target is in this group's id: its
	/// applications', its machines', and the group's own.
	pub group_id: Option<String>,
	/// Restrict to one application's id. Returns the issues of the machine it
	/// runs on among its own, matching what that application presents: a box's
	/// disk filling degrades the software on it.
	pub application_id: Option<String>,
	/// Restrict to one machine's id, its own issues only. Ask this to find out
	/// what is wrong with a box rather than with any workload on it.
	pub machine_id: Option<String>,
	/// Only issues last seen within this many days.
	pub since_days: Option<u32>,
	/// Max issues to return (default 100).
	pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckDocArgs {
	/// The source that reports the check (e.g. `alertd`, `canopy`).
	pub source: String,
	/// The check's name.
	pub check_name: String,
}

#[derive(Serialize)]
struct CheckDocOut {
	source: String,
	check_name: String,
	/// How the check presents: `<type>.<check>` where it is one application
	/// type's, the bare name otherwise.
	qualified_name: String,
	/// The application type this entry is for, or `null` where the check is the
	/// machine's or a curated source's.
	application_type: Option<String>,
	ceiling: CheckResult,
	escalates: bool,
	/// Operator-authored markdown, or `null` if nobody has documented
	/// this check yet.
	documentation: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct IssueIdArgs {
	/// The issue's id.
	pub issue_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckRefArg {
	/// The source that reports the check (e.g. `alertd`, `canopy`).
	pub source: String,
	/// The check's name.
	pub check_name: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct CheckStabilityArgs {
	/// The (source, check) pairs to fetch stability for. Up to 32.
	pub checks: Vec<CheckRefArg>,
	/// Restrict to one server's id.
	pub application_id: Option<String>,
	/// Restrict to one group's id (its applications plus its group-scoped
	/// checks).
	pub group_id: Option<String>,
}

#[derive(Serialize)]
struct CheckStabilityRow {
	issue_id: Uuid,
	/// The server the state belongs to; `null` for group- or canopy-wide
	/// states.
	application_id: Option<Uuid>,
	server_name: Option<String>,
	/// The group for group-scoped states; `null` otherwise.
	group_id: Option<Uuid>,
	source: String,
	check_name: Option<String>,
	observed_result: Option<CheckResult>,
	effective_result: Option<CheckResult>,
	active: bool,
	/// The full stability record: observation counters, the
	/// healthy↔degraded transition ring (oldest first), the hour-of-week
	/// duty profile (168 buckets, UTC, Monday 00:00 first), and derived
	/// flap statistics. `null` for states that predate stability
	/// recording.
	stability: Option<database::stability::StabilityData>,
}

#[derive(Serialize)]
struct CheckStabilityOut {
	/// One row per matching check state across all scopes.
	rows: Vec<CheckStabilityRow>,
}

#[derive(Serialize)]
struct IncidentSummary {
	id: Uuid,
	/// The group the incident targets, or `null` for a canopy-wide
	/// incident (aggregating canopy's self-alerts).
	group_id: Option<Uuid>,
	group_name: Option<String>,
	/// Which of the group's environments the incident targets. `null` with a
	/// group is the group itself: its own checks, and the members of a group
	/// with no ranked application.
	rank: Option<ServerRank>,
	/// `open` (not closed), `resolved` (operator-resolved), or `closed`.
	status: &'static str,
	opened_at: Timestamp,
	closed_at: Option<Timestamp>,
	resolved_at: Option<Timestamp>,
	resolved_by: Option<String>,
	resolved_reason: Option<String>,
	/// Whether the incident ever escalated (a critical issue joined).
	escalated: bool,
	/// Whether the incident actually surfaced to operators (its Slack open
	/// notice was delivered): it outlived the group's grace window, or it
	/// escalated. Incidents that flapped shut within the grace never published.
	/// Prefer counting `published` incidents over raw rows.
	published: bool,
	/// How long the incident was (or has been) open, in seconds.
	open_duration_secs: i64,
	issue_count: i64,
}

#[derive(Serialize)]
struct IncidentList {
	count: usize,
	/// How many of `count` actually surfaced to operators (see `published`).
	published_count: usize,
	since: Timestamp,
	incidents: Vec<IncidentSummary>,
}

/// The display names every grain an issue can be filed at, for a batch of them.
///
/// One resolver so the incident view and the issue list cannot disagree about
/// what a scope is called, and so adding a grain is one change rather than two.
async fn scope_labels(
	conn: &mut AsyncPgConnection,
	issues: &[&Issue],
) -> Result<
	(
		std::collections::HashMap<Uuid, (Option<String>, Option<String>)>,
		std::collections::HashMap<Uuid, Option<String>>,
		std::collections::HashMap<Uuid, String>,
	),
	McpError,
> {
	let applications = Application::names_by_ids(
		conn,
		&unique(issues.iter().filter_map(|i| i.application_id)),
	)
	.await
	.map_err(mcp_err)?;
	let machines = database::machines::Machine::names_by_ids(
		conn,
		&unique(issues.iter().filter_map(|i| i.machine_id)),
	)
	.await
	.map_err(mcp_err)?;
	let groups = group_names(
		conn,
		&unique(issues.iter().filter_map(|i| i.server_group_id)),
	)
	.await?;
	Ok((applications, machines, groups))
}

/// What an issue is filed against, and what that thing is called.
///
/// One tagged value rather than a row of nullable ids: a client asking "whose
/// failure is this" gets the grain and the name together, and cannot read a
/// machine's issue as an unattributed one because `application_id` was null.
// spec: MCP#incidents-and-issues
#[derive(Serialize)]
#[serde(tag = "grain", rename_all = "snake_case")]
enum IssueScopeOut {
	/// The software: a version, a database connection, a service of its own.
	Application { id: Uuid, name: Option<String> },
	/// The box: its disks, its memory, its clock, its reachability.
	Machine { id: Uuid, name: Option<String> },
	/// The group as a whole, rather than any one of its parts.
	Group { id: Uuid, name: Option<String> },
	/// Canopy watching itself.
	Canopy,
}

impl IssueScopeOut {
	fn of(
		issue: &Issue,
		applications: &std::collections::HashMap<Uuid, (Option<String>, Option<String>)>,
		machines: &std::collections::HashMap<Uuid, Option<String>>,
		groups: &std::collections::HashMap<Uuid, String>,
	) -> Self {
		match database::issues::Scope::from_columns(
			issue.application_id,
			issue.machine_id,
			issue.server_group_id,
		) {
			database::issues::Scope::Application(id) => Self::Application {
				id,
				name: applications.get(&id).and_then(|(n, _)| n.clone()),
			},
			database::issues::Scope::Machine(id) => Self::Machine {
				id,
				name: machines.get(&id).cloned().flatten(),
			},
			database::issues::Scope::Group(id) => Self::Group {
				id,
				name: groups.get(&id).cloned(),
			},
			database::issues::Scope::Global => Self::Canopy,
		}
	}
}

#[derive(Serialize)]
struct IncidentIssueOut {
	issue_id: Uuid,
	/// What the source reported on the latest filing, before policy.
	observed_result: Option<CheckResult>,
	/// What policy made of it — the result canopy acts on.
	effective_result: Option<CheckResult>,
	/// Whether the check's policy escalates (an effective failure
	/// notifies immediately, bypassing incident grace).
	escalates: bool,
	source: String,
	r#ref: String,
	description: Option<String>,
	message: String,
	active: bool,
	/// Whose failure this is: the box, the software on it, the group, or
	/// Canopy itself.
	scope: IssueScopeOut,
	first_seen: Timestamp,
	last_seen: Timestamp,
	joined_at: Timestamp,
	/// None = still attached to the incident.
	left_at: Option<Timestamp>,
}

#[derive(Serialize)]
struct IncidentDetail {
	id: Uuid,
	/// The group the incident targets, or `null` for a canopy-wide
	/// incident (aggregating canopy's self-alerts).
	group_id: Option<Uuid>,
	group_name: Option<String>,
	/// Which of the group's environments the incident targets. `null` with a
	/// group is the group itself: its own checks, and the members of a group
	/// with no ranked application.
	rank: Option<ServerRank>,
	status: &'static str,
	opened_at: Timestamp,
	closed_at: Option<Timestamp>,
	resolved_at: Option<Timestamp>,
	resolved_by: Option<String>,
	resolved_reason: Option<String>,
	escalated_at: Option<Timestamp>,
	/// Whether the incident surfaced to operators (Slack open delivered).
	published: bool,
	open_duration_secs: i64,
	created_at: Timestamp,
	updated_at: Timestamp,
	issues: Vec<IncidentIssueOut>,
}

#[derive(Serialize)]
struct IssueSummary {
	id: Uuid,
	/// Whose failure this is: the box, the software on it, the group, or
	/// Canopy itself.
	scope: IssueScopeOut,
	source: String,
	r#ref: String,
	observed_result: Option<CheckResult>,
	effective_result: Option<CheckResult>,
	escalates: bool,
	description: Option<String>,
	message: String,
	active: bool,
	first_seen: Timestamp,
	last_seen: Timestamp,
	resolved_at: Option<Timestamp>,
	snoozed_until: Option<Timestamp>,
}

#[derive(Serialize)]
struct IssueList {
	count: usize,
	issues: Vec<IssueSummary>,
}

#[derive(Serialize)]
struct IncidentRefOut {
	incident_id: Uuid,
	opened_at: Timestamp,
	closed_at: Option<Timestamp>,
}

#[derive(Serialize)]
struct IssueDetail {
	id: Uuid,
	application_id: Option<Uuid>,
	server_name: Option<String>,
	group_id: Option<Uuid>,
	source: String,
	r#ref: String,
	observed_result: Option<CheckResult>,
	effective_result: Option<CheckResult>,
	escalates: bool,
	description: Option<String>,
	message: String,
	active: bool,
	first_seen: Timestamp,
	last_seen: Timestamp,
	resolved_at: Option<Timestamp>,
	resolved_by: Option<String>,
	resolved_reason: Option<String>,
	snoozed_until: Option<Timestamp>,
	incidents: Vec<IncidentRefOut>,
}

#[tool_router(router = incidents_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		description = "List incidents that were open at any point in a recent window (default last \
		               7 days), optionally for one group. Use this for 'incidents open in the past \
		               week'.\n\n\
		               IMPORTANT for summaries/ranking: count `published` incidents, not raw rows. \
		               The window includes a large volume of sub-grace flapping (health checks that \
		               recover/refire, alerts that self-clear in under a minute) that was recorded \
		               but never surfaced to anyone. An incident is `published` only if its Slack \
		               open notice was delivered: it stayed open past the group's grace window \
		               (slack_open_delay, ~3 min by default) OR it escalated (a critical issue \
		               joined, which bypasses the grace). A count dominated by unpublished \
		               short-lived incidents usually means a twitchy alert/health-check \
		               threshold, not a real outage. `published_count` gives the surfaced \
		               subset directly."
	)]
	async fn find_incidents(
		&self,
		Parameters(args): Parameters<FindIncidentsArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let since = since_from_days(args.since_days.unwrap_or(7));
		let group = parse_opt_uuid(&args.group_id, "group_id")?;
		let limit = args.limit.unwrap_or(100);
		let status = match args.status.as_deref().unwrap_or("all") {
			"open" => IncidentStatusFilter::Open,
			"resolved" => IncidentStatusFilter::Resolved,
			_ => IncidentStatusFilter::All,
		};

		let incidents: Vec<Incident> =
			Incident::list_open_since(&mut conn, since, group, status, limit)
				.await
				.map_err(mcp_err)?;

		let group_names = group_names(
			&mut conn,
			&unique(incidents.iter().filter_map(|i| i.server_group_id)),
		)
		.await?;
		let ids: Vec<Uuid> = incidents.iter().map(|i| i.id).collect();
		let stats = Incident::stats_for(&self.db, &ids).await.map_err(mcp_err)?;
		let published = SlackOutbox::delivered_open_ids(&mut conn, &ids)
			.await
			.map_err(mcp_err)?;

		let summaries: Vec<IncidentSummary> = incidents
			.iter()
			.map(|i| {
				let s = stats.get(&i.id);
				IncidentSummary {
					id: i.id,
					group_id: i.server_group_id,
					rank: i.rank,
					group_name: i
						.server_group_id
						.and_then(|gid| group_names.get(&gid).cloned()),
					status: incident_status(i),
					opened_at: i.opened_at,
					closed_at: i.closed_at,
					resolved_at: i.resolved_at,
					resolved_by: i.resolved_by.clone(),
					resolved_reason: i.resolved_reason.clone(),
					escalated: i.escalated_at.is_some(),
					published: published.contains(&i.id),
					open_duration_secs: open_duration_secs(i),
					issue_count: s.map_or(0, |s| s.issue_count),
				}
			})
			.collect();

		ok_json(&IncidentList {
			count: summaries.len(),
			published_count: summaries.iter().filter(|s| s.published).count(),
			since,
			incidents: summaries,
		})
	}

	#[tool(
		description = "Full detail for one incident: timing, status, and the issues attached to it \
		               (with their severities and messages)."
	)]
	async fn get_incident(
		&self,
		Parameters(args): Parameters<IncidentIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let id = parse_uuid(&args.incident_id, "incident_id")?;
		let Ok((incident, rows)) = Incident::get_with_issues(&mut conn, id).await else {
			return Ok(not_found(format!("no incident with id {id}")));
		};
		let group = match incident.server_group_id {
			Some(gid) => ServerGroup::get_by_id(&mut conn, gid).await.ok(),
			None => None,
		};
		let published = SlackOutbox::delivered_open_ids(&mut conn, &[incident.id])
			.await
			.map_err(mcp_err)?
			.contains(&incident.id);
		let (names, machine_names, group_names_map) =
			scope_labels(&mut conn, &rows.iter().map(|(_, i)| i).collect::<Vec<_>>()).await?;

		let issues = rows
			.iter()
			.map(|(link, iss)| IncidentIssueOut {
				issue_id: iss.id,
				observed_result: iss.observed_result,
				effective_result: iss.effective_result,
				escalates: iss.escalates,
				source: iss.source.clone(),
				r#ref: iss.r#ref.clone(),
				description: iss.description.clone(),
				message: iss.message.clone(),
				active: iss.active,
				scope: IssueScopeOut::of(iss, &names, &machine_names, &group_names_map),
				first_seen: iss.first_seen,
				last_seen: iss.last_seen,
				joined_at: link.joined_at,
				left_at: link.left_at,
			})
			.collect();

		ok_json(&IncidentDetail {
			id: incident.id,
			group_id: incident.server_group_id,
			rank: incident.rank,
			group_name: group.as_ref().map(|g| g.name.clone()),
			status: incident_status(&incident),
			opened_at: incident.opened_at,
			closed_at: incident.closed_at,
			resolved_at: incident.resolved_at,
			resolved_by: incident.resolved_by.clone(),
			resolved_reason: incident.resolved_reason.clone(),
			escalated_at: incident.escalated_at,
			published,
			open_duration_secs: open_duration_secs(&incident),
			created_at: incident.created_at,
			updated_at: incident.updated_at,
			issues,
		})
	}

	#[tool(
		description = "List issues across the fleet, filtered by active state, effective result, \
		               group, server, and recency. Issues are the per-(server,source,check) conditions \
		               that make up incidents."
	)]
	async fn find_issues(
		&self,
		Parameters(args): Parameters<FindIssuesArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let results = parse_results(&args.results)?;
		let group = parse_opt_uuid(&args.group_id, "group_id")?;
		let server = parse_opt_uuid(&args.application_id, "application_id")?;
		let machine = parse_opt_uuid(&args.machine_id, "machine_id")?;
		let since = args.since_days.map(since_from_days);
		let limit = args.limit.unwrap_or(100);

		let issues = Issue::list(
			&mut conn,
			IssueListFilters {
				active_only: args.active_only.unwrap_or(true),
				results,
				server_group_id: group,
				application_id: server,
				machine_id: machine,
				since,
			},
			limit,
		)
		.await
		.map_err(mcp_err)?;

		let (names, machine_names, group_names_map) =
			scope_labels(&mut conn, &issues.iter().collect::<Vec<_>>()).await?;
		let summaries: Vec<IssueSummary> = issues
			.iter()
			.map(|i| issue_summary(i, &names, &machine_names, &group_names_map))
			.collect();
		ok_json(&IssueList {
			count: summaries.len(),
			issues: summaries,
		})
	}

	#[tool(
		description = "Full detail for one issue: its fields and the incidents it is or was part of."
	)]
	async fn get_issue(
		&self,
		Parameters(args): Parameters<IssueIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let id = parse_uuid(&args.issue_id, "issue_id")?;
		let Ok(issue) = Issue::get_by_id(&mut conn, id).await else {
			return Ok(not_found(format!("no issue with id {id}")));
		};
		let inc = Incident::for_issues(&mut conn, &[id])
			.await
			.map_err(mcp_err)?;
		let server_name = match issue.application_id {
			Some(sid) => Application::names_by_ids(&mut conn, &[sid])
				.await
				.map_err(mcp_err)?
				.get(&sid)
				.and_then(|(n, _)| n.clone()),
			None => None,
		};

		let incidents = inc
			.get(&id)
			.into_iter()
			.flatten()
			.map(|r| IncidentRefOut {
				incident_id: r.incident_id,
				opened_at: r.opened_at,
				closed_at: r.closed_at,
			})
			.collect();

		ok_json(&IssueDetail {
			id: issue.id,
			application_id: issue.application_id,
			server_name,
			group_id: issue.server_group_id,
			source: issue.source.clone(),
			r#ref: issue.r#ref.clone(),
			observed_result: issue.observed_result,
			effective_result: issue.effective_result,
			escalates: issue.escalates,
			description: issue.description.clone(),
			message: issue.message.clone(),
			active: issue.active,
			first_seen: issue.first_seen,
			last_seen: issue.last_seen,
			resolved_at: issue.resolved_at,
			resolved_by: issue.resolved_by.clone(),
			resolved_reason: issue.resolved_reason.clone(),
			snoozed_until: issue.snoozed_until,
			incidents,
		})
	}

	#[tool(
		description = "Get the operator-authored documentation for a (source, check): what the \
		               check observes, what each result means, and hints for solving a failure. \
		               Prefer this curated knowledge over inferring what a check does from its \
		               name. Also returns the check's current policy (ceiling, escalates)."
	)]
	async fn get_check_documentation(
		&self,
		Parameters(args): Parameters<CheckDocArgs>,
	) -> Result<CallToolResult, McpError> {
		use database::check_policies::CheckPolicy;
		let mut conn = self.conn().await?;
		// A name can be several entries: an application-subject check is one per
		// type, each with its own ceiling and documentation. Return them all
		// rather than picking one, so the caller sees what it is choosing between.
		let policies =
			CheckPolicy::get_across_namespaces(&mut conn, &args.source, &args.check_name)
				.await
				.map_err(mcp_err)?;
		if policies.is_empty() {
			return Ok(not_found(format!(
				"no catalog entry for ({}, {}) — that source has never reported that check",
				args.source, args.check_name
			)));
		}
		let out: Vec<CheckDocOut> = policies
			.into_iter()
			.map(|policy| {
				let namespace = policy.namespace().ok();
				CheckDocOut {
					qualified_name: namespace.as_ref().map_or_else(
						|| policy.check_name.clone(),
						|ns| ns.qualified_name(&policy.check_name),
					),
					application_type: namespace
						.as_ref()
						.and_then(|ns| ns.application_type())
						.map(|t| t.to_string()),
					source: policy.source,
					check_name: policy.check_name,
					ceiling: policy.ceiling,
					escalates: policy.escalates,
					documentation: policy.documentation,
				}
			})
			.collect();
		ok_json(&out)
	}

	#[tool(
		description = "Full stability records for a set of checks, one row per (target, source, \
		               check) state: observation counts, the recent healthy<->degraded transition \
		               ring, an hour-of-week degradation profile (168 buckets, UTC, Monday 00:00 \
		               first), and derived flap statistics (recent flip counts, typical \
		               degraded-run and healthy-gap durations). Built from observed results, \
		               before policy, so grading never distorts it. The raw material for telling \
		               a flap from a load-dependent pattern from a real change in behaviour. \
		               Optionally narrow to one server or one group."
	)]
	async fn get_check_stability(
		&self,
		Parameters(args): Parameters<CheckStabilityArgs>,
	) -> Result<CallToolResult, McpError> {
		if args.checks.is_empty() {
			return Err(McpError::invalid_params(
				"checks must name at least one (source, check_name) pair".to_string(),
				None,
			));
		}
		if args.checks.len() > 32 {
			return Err(McpError::invalid_params(
				"too many checks: at most 32 (source, check_name) pairs per call".to_string(),
				None,
			));
		}
		let application_id = parse_opt_uuid(&args.application_id, "application_id")?;
		let group_id = parse_opt_uuid(&args.group_id, "group_id")?;
		let pairs: Vec<(String, String)> = args
			.checks
			.into_iter()
			.map(|c| (c.source, c.check_name))
			.collect();

		let mut conn = self.conn().await?;
		let states =
			database::stability::states_for_checks(&mut conn, &pairs, application_id, group_id)
				.await
				.map_err(mcp_err)?;

		let server_ids: Vec<Uuid> = unique(states.iter().filter_map(|(st, _)| st.application_id));
		let names = Application::names_by_ids(&mut conn, &server_ids)
			.await
			.map_err(mcp_err)?;
		let now = Timestamp::now();
		let rows: Vec<CheckStabilityRow> = states
			.into_iter()
			.map(|(st, stability)| CheckStabilityRow {
				issue_id: st.id,
				application_id: st.application_id,
				server_name: st
					.application_id
					.and_then(|sid| names.get(&sid))
					.and_then(|(n, _)| n.clone()),
				group_id: st.server_group_id,
				source: st.source,
				check_name: st.check_name,
				observed_result: st.observed_result,
				effective_result: st.effective_result,
				active: st.active,
				stability: stability
					.map(|row| database::stability::StabilityData::from_row(&row, now)),
			})
			.collect();
		ok_json(&CheckStabilityOut { rows })
	}
}

/// How long the incident was (or has been) open, in seconds.
fn open_duration_secs(i: &Incident) -> i64 {
	let end = i.closed_at.unwrap_or_else(Timestamp::now);
	end.duration_since(i.opened_at).as_secs().max(0)
}

fn incident_status(i: &Incident) -> &'static str {
	if i.resolved_at.is_some() {
		"resolved"
	} else if i.closed_at.is_some() {
		"closed"
	} else {
		"open"
	}
}

fn parse_results(v: &Option<Vec<String>>) -> Result<Option<Vec<CheckResult>>, McpError> {
	match v {
		Some(list) if !list.is_empty() => {
			let mut out = Vec::with_capacity(list.len());
			for s in list {
				out.push(
					s.parse::<CheckResult>().map_err(|_| {
						McpError::invalid_params(format!("invalid result: {s}"), None)
					})?,
				);
			}
			Ok(Some(out))
		}
		_ => Ok(None),
	}
}

fn issue_summary(
	i: &Issue,
	names: &std::collections::HashMap<Uuid, (Option<String>, Option<String>)>,
	machines: &std::collections::HashMap<Uuid, Option<String>>,
	groups: &std::collections::HashMap<Uuid, String>,
) -> IssueSummary {
	IssueSummary {
		id: i.id,
		scope: IssueScopeOut::of(i, names, machines, groups),
		source: i.source.clone(),
		r#ref: i.r#ref.clone(),
		observed_result: i.observed_result,
		effective_result: i.effective_result,
		escalates: i.escalates,
		description: i.description.clone(),
		message: i.message.clone(),
		active: i.active,
		first_seen: i.first_seen,
		last_seen: i.last_seen,
		resolved_at: i.resolved_at,
		snoozed_until: i.snoozed_until,
	}
}
