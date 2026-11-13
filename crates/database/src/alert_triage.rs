//! Alert triage: what Canopy decides about an incident notification before an
//! agent judges the rest, and the verdicts agents record on incidents.
//!
//! Spec: `.workhorse/specs/monitoring/alert-triage.md` (id `TRI`).
//!
//! An incident's state is read from its live members: which checks are
//! failing, whether each escalates, and the worst live result. That state is
//! digested into a revision, so a verdict can be refused when the incident moved
//! between the read and the record, and so a repeated alert at an unchanged
//! state finds the verdict already given. The latest verdict's state is the
//! baseline the next triage compares against.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};

use commons_errors::{AppError, Result};
use commons_types::{server::rank::ServerRank, status::CheckResult};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp, civil, tz::TimeZone};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::{
	issues::{Incident, IncidentIssue, IncidentTarget, Issue},
	memory::MemoryEntry,
	notes::{AuthorKind, IncidentNote},
	schema::{
		alert_triage_verdicts, check_policies, incident_issues, incidents, issues, memory_entries,
		slack_outbox,
	},
	server_groups::{ServerGroup, environment_name},
	slack_outbox::{KIND_INCIDENT_OPEN, SlackOutbox},
};

/// Two failures are close in time when they began within this of each other.
// spec: TRI#what-is-triaged
pub const CLOSE_IN_TIME: SignedDuration = SignedDuration::from_mins(30);

/// How long a published incident may stay open past its notification with no
/// verdict before the self-alert names it.
// spec: TRI#untriaged-incidents
pub const UNTRIAGED_AFTER: SignedDuration = SignedDuration::from_hours(1);

/// Alert triage counts as in use while a verdict has been recorded this
/// recently.
// spec: TRI#untriaged-incidents
pub const IN_USE_WITHIN: SignedDuration = SignedDuration::from_hours(24 * 7);

/// The longest reasoning a verdict holds, in characters.
pub const REASONING_LIMIT: usize = 4000;

/// The longest link to a report, in characters.
pub const REPORT_LINK_LIMIT: usize = 2000;

/// The zone a write-up day is counted in, and the hour it begins.
// spec: TRI#the-daily-write-up
pub const WRITE_UP_ZONE: &str = "Pacific/Auckland";
pub const WRITE_UP_HOUR: i8 = 8;

/// The most incidents a find of verdicts reads for a write-up day.
const WRITE_UP_LIMIT: i64 = 1000;

/// Whether an incident was escalated or logged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Verdict {
	Escalated,
	Logged,
}

impl Verdict {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Escalated => "escalated",
			Self::Logged => "logged",
		}
	}

	fn parse(text: &str) -> Self {
		match text {
			"escalated" => Self::Escalated,
			_ => Self::Logged,
		}
	}
}

/// A live result's severity, least severe first, so the derived ordering ranks
/// them: a broken check counts as a warning.
// spec: TRI#what-is-triaged
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Severity {
	Warning,
	Failure,
	EscalatingFailure,
}

impl Severity {
	/// The severity of a member's effective result, none for one that is not
	/// degraded.
	pub fn of(result: Option<CheckResult>, escalates: bool) -> Option<Self> {
		match result? {
			CheckResult::Failed if escalates => Some(Self::EscalatingFailure),
			CheckResult::Failed => Some(Self::Failure),
			CheckResult::Warning | CheckResult::Broken => Some(Self::Warning),
			CheckResult::Passed | CheckResult::Skipped => None,
		}
	}

	fn as_str(self) -> &'static str {
		match self {
			Self::Warning => "warning",
			Self::Failure => "failure",
			Self::EscalatingFailure => "escalating_failure",
		}
	}

	fn parse(text: &str) -> Option<Self> {
		match text {
			"warning" => Some(Self::Warning),
			"failure" => Some(Self::Failure),
			"escalating_failure" => Some(Self::EscalatingFailure),
			_ => None,
		}
	}
}

/// A check, by its source and name, as an incident memory entry links one.
// spec: TRI#what-is-triaged
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct CheckKey {
	pub source: String,
	pub name: String,
}

impl CheckKey {
	fn of(issue: &Issue) -> Self {
		Self {
			source: issue.source.clone(),
			name: issue
				.check_name
				.clone()
				.unwrap_or_else(|| issue.r#ref.clone()),
		}
	}
}

/// A check failing in an incident, and whether it escalates.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FailingCheck {
	#[serde(flatten)]
	pub check: CheckKey,
	pub escalates: bool,
}

/// An incident's state as triage reads it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct State {
	/// Whether the incident is open: neither closed nor resolved.
	pub open: bool,
	pub worst: Option<Severity>,
	pub failing: Vec<FailingCheck>,
	/// A digest of the live members' results, and of whether the incident is
	/// open. Equal digests are an unchanged state.
	pub revision: String,
	/// When each failing check's failure began, the earliest across the
	/// members filing it.
	pub began: BTreeMap<CheckKey, Timestamp>,
}

/// The live members of an incident: those whose latest link has not left.
fn live(rows: &[(IncidentIssue, Issue)]) -> impl Iterator<Item = &Issue> {
	rows.iter()
		.filter(|(link, _)| link.left_at.is_none())
		.map(|(_, issue)| issue)
}

fn is_failing(issue: &Issue) -> bool {
	issue.active && issue.effective_result == Some(CheckResult::Failed)
}

/// Read an incident's state from its members.
pub fn state_of(incident: &Incident, rows: &[(IncidentIssue, Issue)]) -> State {
	let open = incident.closed_at.is_none() && incident.resolved_at.is_none();
	let mut digest_lines: Vec<String> = Vec::new();
	let mut worst = None;
	let mut failing: BTreeMap<CheckKey, bool> = BTreeMap::new();
	let mut began: BTreeMap<CheckKey, Timestamp> = BTreeMap::new();
	for issue in live(rows) {
		let severity = Severity::of(issue.effective_result, issue.escalates);
		worst = worst.max(severity);
		digest_lines.push(format!(
			"{}|{}",
			issue.id,
			severity.map_or("none", Severity::as_str)
		));
		if is_failing(issue) {
			let key = CheckKey::of(issue);
			*failing.entry(key.clone()).or_default() |= issue.escalates;
			let since = issue.degraded_since.unwrap_or(issue.first_seen);
			began
				.entry(key)
				.and_modify(|at| *at = (*at).min(since))
				.or_insert(since);
		}
	}
	digest_lines.sort();
	let mut hasher = Sha256::new();
	hasher.update(if open { "open\n" } else { "closed\n" });
	for line in &digest_lines {
		hasher.update(line.as_bytes());
		hasher.update(b"\n");
	}
	let revision = hasher
		.finalize()
		.iter()
		.take(12)
		.map(|b| format!("{b:02x}"))
		.collect();
	State {
		open,
		worst,
		failing: failing
			.into_iter()
			.map(|(check, escalates)| FailingCheck { check, escalates })
			.collect(),
		revision,
		began,
	}
}

/// Why Canopy requires an incident to be escalated.
// spec: TRI#what-canopy-decides-must-be-escalated
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "reason", rename_all = "snake_case")]
pub enum Reason {
	/// A first triage of an incident on a production environment.
	ProductionOpening,
	/// A first triage of an incident on Canopy as a whole.
	CanopyOpening,
	/// An escalating check is failing that was not failing as one at the
	/// baseline.
	EscalatingFailure { check: CheckKey },
	/// The worst live result is more severe than at the baseline.
	Worse {
		from: Option<Severity>,
		to: Severity,
	},
	/// A check is failing that was not failing at the baseline.
	NewFailingCheck { check: CheckKey },
	/// The same check is failing in an open incident on another target, the
	/// two failures close in time.
	SharedCheck {
		check: CheckKey,
		incident_id: Uuid,
		target: String,
		began_here: Timestamp,
		began_there: Timestamp,
	},
	/// A failing check with no documentation, no current memory entry linked
	/// to it, and no other published incident it has been part of.
	Unfamiliar { check: CheckKey },
}

/// What is known of a failing check, for judging it.
#[derive(Clone, Debug, Default)]
pub struct Familiarity {
	pub documented: bool,
	pub linked_entries: usize,
	pub other_published_incidents: usize,
}

/// The reasons Canopy requires the incident be escalated, from its state, its
/// baseline, the incidents on other targets sharing a failing check close in
/// time, and what is known of each failing check. A closed or resolved
/// incident has none.
// spec: TRI#what-canopy-decides-must-be-escalated
pub fn reasons(
	incident: &Incident,
	state: &State,
	baseline: Option<&VerdictRow>,
	shared: &[SharedFailure],
	familiarity: &BTreeMap<CheckKey, Familiarity>,
) -> Vec<Reason> {
	let mut out = Vec::new();
	if !state.open {
		return out;
	}
	match (baseline, IncidentTarget::of_incident(incident)) {
		(None, IncidentTarget::Global) => out.push(Reason::CanopyOpening),
		(None, IncidentTarget::Environment(_, ServerRank::Production)) => {
			out.push(Reason::ProductionOpening)
		}
		_ => {}
	}
	let before: BTreeMap<&CheckKey, bool> = baseline
		.map(|row| {
			row.failing_checks
				.iter()
				.map(|f| (&f.check, f.escalates))
				.collect()
		})
		.unwrap_or_default();
	for failing in &state.failing {
		if failing.escalates && before.get(&failing.check) != Some(&true) {
			out.push(Reason::EscalatingFailure {
				check: failing.check.clone(),
			});
		}
	}
	if let Some(row) = baseline {
		if let Some(to) = state.worst
			&& Some(to) > row.worst_result
		{
			out.push(Reason::Worse {
				from: row.worst_result,
				to,
			});
		}
		for failing in &state.failing {
			if !before.contains_key(&failing.check) {
				out.push(Reason::NewFailingCheck {
					check: failing.check.clone(),
				});
			}
		}
	}
	for share in shared {
		out.push(Reason::SharedCheck {
			check: share.check.clone(),
			incident_id: share.incident_id,
			target: share.target.clone(),
			began_here: share.began_here,
			began_there: share.began_there,
		});
	}
	for failing in &state.failing {
		let known = familiarity.get(&failing.check).cloned().unwrap_or_default();
		if !known.documented && known.linked_entries == 0 && known.other_published_incidents == 0 {
			out.push(Reason::Unfamiliar {
				check: failing.check.clone(),
			});
		}
	}
	out
}

/// Whether the harmless-pattern exception can answer `reasons`: the production
/// opening is the only reason.
// spec: TRI#the-harmless-pattern-exception
pub fn exception_can_answer(reasons: &[Reason]) -> bool {
	matches!(reasons, [Reason::ProductionOpening])
}

/// A verdict as recorded.
#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = alert_triage_verdicts)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct VerdictStored {
	id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
	incident_id: Uuid,
	verdict: String,
	reasoning: String,
	revision: String,
	open: bool,
	worst_result: Option<String>,
	failing_checks: serde_json::Value,
	reasons: serde_json::Value,
	memory_entry_ids: Vec<Uuid>,
	note_id: Option<Uuid>,
	report_link: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	reported_at: Option<Timestamp>,
	recorded_by: String,
}

/// A verdict, read back.
#[derive(Clone, Debug, Serialize)]
pub struct VerdictRow {
	pub id: Uuid,
	pub created_at: Timestamp,
	pub incident_id: Uuid,
	pub verdict: Verdict,
	pub reasoning: String,
	pub revision: String,
	pub open: bool,
	pub worst_result: Option<Severity>,
	pub failing_checks: Vec<FailingCheck>,
	pub reasons: Vec<Reason>,
	pub memory_entry_ids: Vec<Uuid>,
	pub note_id: Option<Uuid>,
	pub report_link: Option<String>,
	pub reported_at: Option<Timestamp>,
	pub recorded_by: String,
}

impl From<VerdictStored> for VerdictRow {
	fn from(row: VerdictStored) -> Self {
		Self {
			id: row.id,
			created_at: row.created_at,
			incident_id: row.incident_id,
			verdict: Verdict::parse(&row.verdict),
			reasoning: row.reasoning,
			revision: row.revision,
			open: row.open,
			worst_result: row.worst_result.as_deref().and_then(Severity::parse),
			failing_checks: serde_json::from_value(row.failing_checks).unwrap_or_default(),
			reasons: serde_json::from_value(row.reasons).unwrap_or_default(),
			memory_entry_ids: row.memory_entry_ids,
			note_id: row.note_id,
			report_link: row.report_link,
			reported_at: row.reported_at,
			recorded_by: row.recorded_by,
		}
	}
}

/// An incident's verdicts, newest first.
pub async fn verdicts_for(
	conn: &mut AsyncPgConnection,
	incident_id: Uuid,
) -> Result<Vec<VerdictRow>> {
	let rows: Vec<VerdictStored> = alert_triage_verdicts::table
		.filter(alert_triage_verdicts::incident_id.eq(incident_id))
		.order((
			alert_triage_verdicts::created_at.desc(),
			alert_triage_verdicts::id,
		))
		.select(VerdictStored::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(rows.into_iter().map(VerdictRow::from).collect())
}

/// The latest verdict of each of `incident_ids`.
pub async fn latest_verdicts(
	conn: &mut AsyncPgConnection,
	incident_ids: &[Uuid],
) -> Result<HashMap<Uuid, VerdictRow>> {
	if incident_ids.is_empty() {
		return Ok(HashMap::new());
	}
	let rows: Vec<VerdictStored> = alert_triage_verdicts::table
		.filter(alert_triage_verdicts::incident_id.eq_any(incident_ids))
		.distinct_on(alert_triage_verdicts::incident_id)
		.order((
			alert_triage_verdicts::incident_id,
			alert_triage_verdicts::created_at.desc(),
		))
		.select(VerdictStored::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.map(|row| (row.incident_id, VerdictRow::from(row)))
		.collect())
}

/// A verdict by its identifier.
pub async fn get_verdict(conn: &mut AsyncPgConnection, id: Uuid) -> Result<VerdictRow> {
	alert_triage_verdicts::table
		.filter(alert_triage_verdicts::id.eq(id))
		.select(VerdictStored::as_select())
		.first(conn)
		.await
		.map(VerdictRow::from)
		.map_err(AppError::from)
}

/// A failure on another open incident's target that shares a check with this
/// incident and began close in time to it.
#[derive(Clone, Debug, Serialize)]
pub struct SharedFailure {
	pub check: CheckKey,
	pub incident_id: Uuid,
	pub target: String,
	pub began_here: Timestamp,
	pub began_there: Timestamp,
}

/// An incident on another target related to this one, for the agent to read.
#[derive(Clone, Debug, Serialize)]
pub struct Related {
	pub incident_id: Uuid,
	pub target: String,
	pub opened_at: Timestamp,
	/// The checks it shares with this incident; empty for a candidate.
	pub shared_checks: Vec<CheckKey>,
	/// Its failing checks, for judging whether a candidate shares a cause.
	pub failing_checks: Vec<CheckKey>,
	pub latest_verdict: Option<RelatedVerdict>,
}

/// A related incident's latest verdict and where it was reported.
#[derive(Clone, Debug, Serialize)]
pub struct RelatedVerdict {
	pub verdict: Verdict,
	pub recorded_at: Timestamp,
	pub report_link: Option<String>,
}

impl From<&VerdictRow> for RelatedVerdict {
	fn from(row: &VerdictRow) -> Self {
		Self {
			verdict: row.verdict,
			recorded_at: row.created_at,
			report_link: row.report_link.clone(),
		}
	}
}

/// What a failing check carries for judging it.
#[derive(Clone, Debug)]
pub struct CheckContext {
	pub check: CheckKey,
	pub escalates: bool,
	pub began: Option<Timestamp>,
	/// The live members filing it, for their stability records.
	pub issue_ids: Vec<Uuid>,
	/// Its documentation, from any catalogue entry under its source and name.
	pub documentation: Option<String>,
	pub familiarity: Familiarity,
}

/// Everything a triage of one incident needs.
#[derive(Clone, Debug)]
pub struct Assessment {
	pub incident: Incident,
	pub target: String,
	pub rows: Vec<(IncidentIssue, Issue)>,
	pub state: State,
	pub verdicts: Vec<VerdictRow>,
	/// The latest verdict, when it was given at this same state.
	pub already_triaged: Option<VerdictRow>,
	pub reasons: Vec<Reason>,
	pub exception_can_answer: bool,
	pub checks: Vec<CheckContext>,
	/// Current memory entries linked to a failing check, the incident's group,
	/// or the incident.
	pub memory: Vec<MemoryEntry>,
	pub shared: Vec<Related>,
	pub candidates: Vec<Related>,
}

/// An open incident's live failing members, for comparing across incidents.
struct OpenFailure {
	incident: Incident,
	check: CheckKey,
	began: Timestamp,
}

async fn open_failures(conn: &mut AsyncPgConnection) -> Result<Vec<OpenFailure>> {
	let rows: Vec<(Incident, Issue)> = incident_issues::table
		.inner_join(incidents::table)
		.inner_join(issues::table)
		.filter(incidents::closed_at.is_null())
		.filter(incidents::resolved_at.is_null())
		.filter(incident_issues::left_at.is_null())
		.filter(issues::active.eq(true))
		.select((Incident::as_select(), Issue::as_select()))
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.filter(|(_, issue)| is_failing(issue))
		.map(|(incident, issue)| OpenFailure {
			began: issue.degraded_since.unwrap_or(issue.first_seen),
			check: CheckKey::of(&issue),
			incident,
		})
		.collect())
}

async fn target_labels(
	conn: &mut AsyncPgConnection,
	incidents: impl IntoIterator<Item = &Incident>,
) -> Result<HashMap<Uuid, String>> {
	let incidents: Vec<&Incident> = incidents.into_iter().collect();
	let group_ids: Vec<Uuid> = incidents.iter().filter_map(|i| i.server_group_id).collect();
	let names = ServerGroup::names_by_ids(conn, &group_ids).await?;
	Ok(incidents
		.into_iter()
		.map(|incident| {
			let label = match IncidentTarget::of_incident(incident) {
				IncidentTarget::Environment(group, rank) => environment_name(
					names.get(&group).map_or("unknown group", String::as_str),
					rank,
				),
				IncidentTarget::Global => "Canopy".to_owned(),
			};
			(incident.id, label)
		})
		.collect())
}

/// Read what a triage of `incident_id` needs, and decide what Canopy requires.
// spec: TRI
pub async fn assess(conn: &mut AsyncPgConnection, incident_id: Uuid) -> Result<Assessment> {
	let (incident, rows) = Incident::get_with_issues(conn, incident_id).await?;
	let state = state_of(&incident, &rows);
	let verdicts = verdicts_for(conn, incident_id).await?;
	let baseline = <[VerdictRow]>::first(&verdicts);
	let already_triaged = baseline
		.filter(|row| row.revision == state.revision)
		.cloned();
	let here = IncidentTarget::of_incident(&incident);

	// Open failures on other targets, for shared checks and candidates.
	let others: Vec<OpenFailure> = if state.open {
		open_failures(conn)
			.await?
			.into_iter()
			.filter(|f| IncidentTarget::of_incident(&f.incident) != here)
			.collect()
	} else {
		Vec::new()
	};
	let mut labels = target_labels(conn, others.iter().map(|f| &f.incident)).await?;
	labels.extend(target_labels(conn, [&incident]).await?);
	let target = labels.get(&incident.id).cloned().unwrap_or_default();

	let close = |a: Timestamp, b: Timestamp| a.duration_since(b).abs() <= CLOSE_IN_TIME;
	let mut shared_failures: Vec<SharedFailure> = Vec::new();
	let mut shared_ids: BTreeMap<Uuid, BTreeSet<CheckKey>> = BTreeMap::new();
	let mut candidate_ids: BTreeSet<Uuid> = BTreeSet::new();
	for other in &others {
		if let Some(&began_here) = state.began.get(&other.check) {
			if close(began_here, other.began) {
				shared_failures.push(SharedFailure {
					check: other.check.clone(),
					incident_id: other.incident.id,
					target: labels.get(&other.incident.id).cloned().unwrap_or_default(),
					began_here,
					began_there: other.began,
				});
			}
			shared_ids
				.entry(other.incident.id)
				.or_default()
				.insert(other.check.clone());
		} else if state.began.values().any(|&at| close(at, other.began)) {
			candidate_ids.insert(other.incident.id);
		}
	}
	candidate_ids.retain(|id| !shared_ids.contains_key(id));

	// What is known of each failing check.
	let keys: Vec<CheckKey> = state.failing.iter().map(|f| f.check.clone()).collect();
	let documentation = documentation_for(conn, &keys).await?;
	let memory = current_memory(conn, &incident, &keys).await?;
	let others_published = other_published_incidents(conn, incident_id, &keys).await?;
	let mut familiarity: BTreeMap<CheckKey, Familiarity> = BTreeMap::new();
	for key in &keys {
		familiarity.insert(
			key.clone(),
			Familiarity {
				documented: documentation.contains_key(key),
				linked_entries: memory
					.iter()
					.filter(|entry| entry_links_check(entry, key))
					.count(),
				other_published_incidents: others_published.get(key).copied().unwrap_or(0),
			},
		);
	}

	let reasons = reasons(&incident, &state, baseline, &shared_failures, &familiarity);
	let exception = exception_can_answer(&reasons);

	let checks = state
		.failing
		.iter()
		.map(|failing| CheckContext {
			check: failing.check.clone(),
			escalates: failing.escalates,
			began: state.began.get(&failing.check).copied(),
			issue_ids: live(&rows)
				.filter(|issue| is_failing(issue) && CheckKey::of(issue) == failing.check)
				.map(|issue| issue.id)
				.collect(),
			documentation: documentation.get(&failing.check).cloned(),
			familiarity: familiarity.get(&failing.check).cloned().unwrap_or_default(),
		})
		.collect();

	let related_ids: Vec<Uuid> = shared_ids
		.keys()
		.chain(candidate_ids.iter())
		.copied()
		.collect();
	let latest = latest_verdicts(conn, &related_ids).await?;
	let related = |id: Uuid, shared_checks: Vec<CheckKey>| {
		let mut failing: Vec<CheckKey> = others
			.iter()
			.filter(|f| f.incident.id == id)
			.map(|f| f.check.clone())
			.collect();
		failing.sort();
		failing.dedup();
		let opened_at = others
			.iter()
			.find(|f| f.incident.id == id)
			.map(|f| f.incident.opened_at)
			.unwrap_or_else(Timestamp::now);
		Related {
			incident_id: id,
			target: labels.get(&id).cloned().unwrap_or_default(),
			opened_at,
			shared_checks,
			failing_checks: failing,
			latest_verdict: latest.get(&id).map(RelatedVerdict::from),
		}
	};
	let shared = shared_ids
		.iter()
		.map(|(id, checks)| related(*id, checks.iter().cloned().collect()))
		.collect();
	let candidates = candidate_ids
		.iter()
		.map(|id| related(*id, Vec::new()))
		.collect();

	Ok(Assessment {
		incident,
		target,
		rows,
		state,
		verdicts,
		already_triaged,
		reasons,
		exception_can_answer: exception,
		checks,
		memory,
		shared,
		candidates,
	})
}

fn entry_links_check(entry: &MemoryEntry, key: &CheckKey) -> bool {
	entry.check_source.as_deref() == Some(key.source.as_str())
		&& entry.check_name.as_deref() == Some(key.name.as_str())
}

/// Documentation for each check that has any, from any catalogue entry under
/// its source and name.
async fn documentation_for(
	conn: &mut AsyncPgConnection,
	keys: &[CheckKey],
) -> Result<HashMap<CheckKey, String>> {
	if keys.is_empty() {
		return Ok(HashMap::new());
	}
	let names: Vec<&str> = keys.iter().map(|k| k.name.as_str()).collect();
	let rows: Vec<(String, String, Option<String>)> = check_policies::table
		.filter(check_policies::check_name.eq_any(&names))
		.filter(check_policies::documentation.is_not_null())
		.select((
			check_policies::source,
			check_policies::check_name,
			check_policies::documentation,
		))
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let wanted: HashSet<&CheckKey> = keys.iter().collect();
	let mut out = HashMap::new();
	for (source, name, documentation) in rows {
		let Some(documentation) = documentation.filter(|d| !d.trim().is_empty()) else {
			continue;
		};
		let key = CheckKey { source, name };
		if wanted.contains(&key) {
			out.entry(key).or_insert(documentation);
		}
	}
	Ok(out)
}

/// Current memory entries linked to one of `keys`, to the incident's group, or
/// to the incident.
async fn current_memory(
	conn: &mut AsyncPgConnection,
	incident: &Incident,
	keys: &[CheckKey],
) -> Result<Vec<MemoryEntry>> {
	let names: Vec<&str> = keys.iter().map(|k| k.name.as_str()).collect();
	let mut query = memory_entries::table
		.filter(memory_entries::retired_at.is_null())
		.filter(
			memory_entries::incident_id
				.eq(incident.id)
				.or(memory_entries::check_name.eq_any(&names)),
		)
		.into_boxed();
	if let Some(group) = incident.server_group_id {
		query = memory_entries::table
			.filter(memory_entries::retired_at.is_null())
			.filter(
				memory_entries::incident_id
					.eq(incident.id)
					.or(memory_entries::check_name.eq_any(&names))
					.or(memory_entries::server_group_id.eq(group)),
			)
			.into_boxed();
	}
	let entries: Vec<MemoryEntry> = query
		.order((memory_entries::updated_at.desc(), memory_entries::id))
		.select(MemoryEntry::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	// A check name can be shared across sources: keep only the entries whose
	// source matches too.
	Ok(entries
		.into_iter()
		.filter(|entry| match (&entry.check_source, &entry.check_name) {
			(Some(_), Some(_)) => keys.iter().any(|key| entry_links_check(entry, key)),
			_ => true,
		})
		.collect())
}

/// For each check, how many other published incidents across the fleet it has
/// been part of.
async fn other_published_incidents(
	conn: &mut AsyncPgConnection,
	incident_id: Uuid,
	keys: &[CheckKey],
) -> Result<HashMap<CheckKey, usize>> {
	if keys.is_empty() {
		return Ok(HashMap::new());
	}
	let names: Vec<&str> = keys.iter().map(|k| k.name.as_str()).collect();
	let rows: Vec<(Uuid, String, Option<String>, String)> = incident_issues::table
		.inner_join(issues::table)
		.filter(incident_issues::incident_id.ne(incident_id))
		.filter(
			issues::check_name
				.eq_any(&names)
				.or(issues::ref_.eq_any(&names)),
		)
		.select((
			incident_issues::incident_id,
			issues::source,
			issues::check_name,
			issues::ref_,
		))
		.distinct()
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let ids: Vec<Uuid> = rows.iter().map(|(id, ..)| *id).collect();
	let published = SlackOutbox::delivered_open_ids(conn, &ids).await?;
	let wanted: HashSet<&CheckKey> = keys.iter().collect();
	let mut seen: HashMap<CheckKey, HashSet<Uuid>> = HashMap::new();
	for (id, source, check_name, r#ref) in rows {
		let key = CheckKey {
			source,
			name: check_name.unwrap_or(r#ref),
		};
		if wanted.contains(&key) && published.contains(&id) {
			seen.entry(key).or_default().insert(id);
		}
	}
	Ok(seen.into_iter().map(|(k, v)| (k, v.len())).collect())
}

/// What a verdict records.
#[derive(Clone, Debug)]
pub struct NewVerdict {
	pub incident_id: Uuid,
	/// The revision the triage read.
	pub revision: String,
	/// When the triage read it, so related escalations since can be returned.
	pub read_at: Timestamp,
	pub verdict: Verdict,
	pub reasoning: String,
	pub memory_entry_ids: Vec<Uuid>,
}

/// What recording a verdict did.
#[derive(Clone, Debug)]
pub enum Recorded {
	/// The verdict, the note showing it, and the related incidents escalated
	/// since the triage read.
	Done {
		verdict: Box<VerdictRow>,
		note: IncidentNote,
		escalated_since: Vec<Related>,
	},
	/// The incident changed since the revision read; this is its revision now.
	Stale { current: String },
	/// A logged verdict where Canopy requires an escalation.
	Refused { reasons: Vec<Reason>, why: String },
}

/// The note showing a verdict on its incident.
fn note_body(verdict: Verdict, reasons: &[Reason], reasoning: &str) -> String {
	let mut body = format!("**Alert triage: {}**\n\n{reasoning}", verdict.as_str());
	if !reasons.is_empty() {
		body.push_str("\n\nCanopy required escalation: ");
		body.push_str(
			&reasons
				.iter()
				.map(describe_reason)
				.collect::<Vec<_>>()
				.join("; "),
		);
		body.push('.');
	}
	body
}

/// A reason in a few words, for the note and for refusals.
pub fn describe_reason(reason: &Reason) -> String {
	match reason {
		Reason::ProductionOpening => "first triage of a production incident".to_owned(),
		Reason::CanopyOpening => "first triage of a Canopy-wide incident".to_owned(),
		Reason::EscalatingFailure { check } => {
			format!("escalating check {} is failing", check.name)
		}
		Reason::Worse { from, to } => format!(
			"worst result rose from {} to {}",
			from.map_or("none", Severity::as_str),
			to.as_str()
		),
		Reason::NewFailingCheck { check } => format!("{} is newly failing", check.name),
		Reason::SharedCheck { check, target, .. } => {
			format!("{} is also failing on {target}", check.name)
		}
		Reason::Unfamiliar { check } => format!("{} is an unfamiliar check", check.name),
	}
}

/// Lock the incident's row, so two verdicts on one incident are recorded one
/// after the other.
async fn lock_incident(conn: &mut AsyncPgConnection, incident_id: Uuid) -> Result<()> {
	incidents::table
		.filter(incidents::id.eq(incident_id))
		.select(incidents::id)
		.for_update()
		.first::<Uuid>(conn)
		.await
		.map(drop)
		.map_err(AppError::from)
}

/// Record a verdict, refusing it when the incident changed since the revision
/// read or when it logs what Canopy requires be escalated. Runs inside the
/// caller's transaction; the note it adds is the caller's to audit.
// spec: TRI#recording-a-verdict
pub async fn record(
	conn: &mut AsyncPgConnection,
	new: &NewVerdict,
	author: &str,
	author_kind: AuthorKind,
) -> Result<Recorded> {
	let reasoning = new.reasoning.trim();
	if reasoning.is_empty() {
		return Err(AppError::BadRequest("the reasoning is empty".into()));
	}
	if reasoning.chars().count() > REASONING_LIMIT {
		return Err(AppError::BadRequest(format!(
			"the reasoning is over {REASONING_LIMIT} characters"
		)));
	}
	lock_incident(conn, new.incident_id).await?;
	let assessment = assess(conn, new.incident_id).await?;
	if assessment.state.revision != new.revision {
		return Ok(Recorded::Stale {
			current: assessment.state.revision,
		});
	}

	let entries: Vec<MemoryEntry> = if new.memory_entry_ids.is_empty() {
		Vec::new()
	} else {
		memory_entries::table
			.filter(memory_entries::id.eq_any(&new.memory_entry_ids))
			.select(MemoryEntry::as_select())
			.load(conn)
			.await
			.map_err(AppError::from)?
	};
	if let Some(missing) = new
		.memory_entry_ids
		.iter()
		.find(|id| !entries.iter().any(|e| e.id == **id))
	{
		return Err(AppError::BadRequest(format!(
			"no memory entry with id {missing}"
		)));
	}

	if new.verdict == Verdict::Logged
		&& !assessment.reasons.is_empty()
		&& let Some(why) = exception_refusal(&assessment, &entries)
	{
		return Ok(Recorded::Refused {
			reasons: assessment.reasons,
			why,
		});
	}

	let note = IncidentNote::add(
		conn,
		new.incident_id,
		author,
		author_kind,
		&note_body(new.verdict, &assessment.reasons, reasoning),
	)
	.await?;
	let state = &assessment.state;
	let stored: VerdictStored = diesel::insert_into(alert_triage_verdicts::table)
		.values((
			alert_triage_verdicts::incident_id.eq(new.incident_id),
			alert_triage_verdicts::verdict.eq(new.verdict.as_str()),
			alert_triage_verdicts::reasoning.eq(reasoning),
			alert_triage_verdicts::revision.eq(&state.revision),
			alert_triage_verdicts::open.eq(state.open),
			alert_triage_verdicts::worst_result.eq(state.worst.map(Severity::as_str)),
			alert_triage_verdicts::failing_checks.eq(serde_json::to_value(&state.failing)
				.map_err(|e| AppError::Custom(e.to_string()))?),
			alert_triage_verdicts::reasons.eq(serde_json::to_value(&assessment.reasons)
				.map_err(|e| AppError::Custom(e.to_string()))?),
			alert_triage_verdicts::memory_entry_ids.eq(&new.memory_entry_ids),
			alert_triage_verdicts::note_id.eq(note.id),
			alert_triage_verdicts::recorded_by.eq(author),
		))
		.returning(VerdictStored::as_select())
		.get_result(conn)
		.await
		.map_err(AppError::from)?;

	let escalated_since = assessment
		.shared
		.iter()
		.chain(&assessment.candidates)
		.filter(|related| {
			related
				.latest_verdict
				.as_ref()
				.is_some_and(|v| v.verdict == Verdict::Escalated && v.recorded_at > new.read_at)
		})
		.cloned()
		.collect();

	Ok(Recorded::Done {
		verdict: Box::new(stored.into()),
		note,
		escalated_since,
	})
}

/// Why a logged verdict does not meet the harmless-pattern exception, none when
/// it does: the production opening is the only reason, and every failing check
/// has a current entry among those given linked to it.
// spec: TRI#the-harmless-pattern-exception
fn exception_refusal(assessment: &Assessment, entries: &[MemoryEntry]) -> Option<String> {
	let required = assessment
		.reasons
		.iter()
		.map(describe_reason)
		.collect::<Vec<_>>()
		.join("; ");
	if !assessment.exception_can_answer {
		return Some(format!(
			"Canopy requires this incident be escalated: {required}"
		));
	}
	let uncovered: Vec<&str> = assessment
		.state
		.failing
		.iter()
		.filter(|failing| {
			!entries
				.iter()
				.any(|e| !e.is_retired() && entry_links_check(e, &failing.check))
		})
		.map(|failing| failing.check.name.as_str())
		.collect();
	if uncovered.is_empty() {
		None
	} else {
		Some(format!(
			"Canopy requires this incident be escalated ({required}) unless a current memory \
			 entry linked to each failing check names the pattern harmless; none is given for: {}",
			uncovered.join(", ")
		))
	}
}

/// Attach where a verdict was reported.
// spec: TRI#recording-a-verdict
pub async fn attach_report(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	link: &str,
) -> Result<VerdictRow> {
	let link = link.trim();
	if link.is_empty() || link.chars().count() > REPORT_LINK_LIMIT {
		return Err(AppError::BadRequest(format!(
			"a report link is between 1 and {REPORT_LINK_LIMIT} characters"
		)));
	}
	if !(link.starts_with("https://") || link.starts_with("http://")) {
		return Err(AppError::BadRequest(
			"a report link is an http(s) address".into(),
		));
	}
	diesel::update(alert_triage_verdicts::table.filter(alert_triage_verdicts::id.eq(id)))
		.set((
			alert_triage_verdicts::report_link.eq(link),
			alert_triage_verdicts::reported_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
		))
		.returning(VerdictStored::as_select())
		.get_result(conn)
		.await
		.map(VerdictRow::from)
		.map_err(AppError::from)
}

/// The span of a write-up day: from 8am New Zealand time on `date` to 8am the
/// day after.
// spec: TRI#the-daily-write-up
pub fn write_up_day(date: civil::Date) -> Result<(Timestamp, Timestamp)> {
	let zone = TimeZone::get(WRITE_UP_ZONE).map_err(|e| AppError::Custom(e.to_string()))?;
	let at = |date: civil::Date| -> Result<Timestamp> {
		date.at(WRITE_UP_HOUR, 0, 0, 0)
			.to_zoned(zone.clone())
			.map(|z| z.timestamp())
			.map_err(|e| AppError::Custom(e.to_string()))
	};
	let next = date
		.tomorrow()
		.map_err(|e| AppError::Custom(e.to_string()))?;
	Ok((at(date)?, at(next)?))
}

/// The write-up day `at` falls in.
// spec: TRI#the-daily-write-up
pub fn write_up_date(at: Timestamp) -> Result<civil::Date> {
	let zone = TimeZone::get(WRITE_UP_ZONE).map_err(|e| AppError::Custom(e.to_string()))?;
	let local = at.to_zoned(zone);
	let date = local.date();
	if local.hour() < WRITE_UP_HOUR {
		date.yesterday()
			.map_err(|e| AppError::Custom(e.to_string()))
	} else {
		Ok(date)
	}
}

/// A verdict in a write-up day, with its incident's target.
#[derive(Clone, Debug, Serialize)]
pub struct DayVerdict {
	pub verdict: VerdictRow,
	pub target: String,
	pub incident_open: bool,
}

/// A published incident with no verdict.
#[derive(Clone, Debug, Serialize)]
pub struct Untriaged {
	pub incident_id: Uuid,
	pub target: String,
	pub opened_at: Timestamp,
	pub notified_at: Timestamp,
	pub open: bool,
}

/// What a write-up day holds.
#[derive(Clone, Debug, Serialize)]
pub struct WriteUpDay {
	pub date: civil::Date,
	pub starts_at: Timestamp,
	pub ends_at: Timestamp,
	pub verdicts: Vec<DayVerdict>,
	pub untriaged: Vec<Untriaged>,
}

/// The verdicts recorded in a write-up day, oldest first, and the incidents
/// published in it that have no verdict.
// spec: TRI#the-daily-write-up
pub async fn find_day(conn: &mut AsyncPgConnection, date: civil::Date) -> Result<WriteUpDay> {
	let (starts_at, ends_at) = write_up_day(date)?;
	let rows: Vec<VerdictStored> = alert_triage_verdicts::table
		.filter(alert_triage_verdicts::created_at.ge(jiff_diesel::Timestamp::from(starts_at)))
		.filter(alert_triage_verdicts::created_at.lt(jiff_diesel::Timestamp::from(ends_at)))
		.order((alert_triage_verdicts::created_at, alert_triage_verdicts::id))
		.limit(WRITE_UP_LIMIT)
		.select(VerdictStored::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let notified = notified_between(conn, Some(starts_at), ends_at).await?;
	let mut incident_ids: Vec<Uuid> = rows.iter().map(|r| r.incident_id).collect();
	incident_ids.extend(notified.iter().map(|(id, _)| *id));
	incident_ids.sort();
	incident_ids.dedup();
	let incidents = incidents_by_ids(conn, &incident_ids).await?;
	let labels = target_labels(conn, incidents.values()).await?;
	let with_verdict = ever_triaged(conn, &incident_ids).await?;
	let is_open = |id: &Uuid| {
		incidents
			.get(id)
			.is_some_and(|i| i.closed_at.is_none() && i.resolved_at.is_none())
	};
	let verdicts = rows
		.into_iter()
		.map(|row| DayVerdict {
			target: labels.get(&row.incident_id).cloned().unwrap_or_default(),
			incident_open: is_open(&row.incident_id),
			verdict: row.into(),
		})
		.collect();
	let untriaged = untriaged_of(&notified, &incidents, &labels, &with_verdict);
	Ok(WriteUpDay {
		date,
		starts_at,
		ends_at,
		verdicts,
		untriaged,
	})
}

fn untriaged_of(
	notified: &[(Uuid, Timestamp)],
	incidents: &HashMap<Uuid, Incident>,
	labels: &HashMap<Uuid, String>,
	with_verdict: &HashSet<Uuid>,
) -> Vec<Untriaged> {
	notified
		.iter()
		.filter(|(id, _)| !with_verdict.contains(id))
		.filter_map(|(id, notified_at)| {
			let incident = incidents.get(id)?;
			Some(Untriaged {
				incident_id: *id,
				target: labels.get(id).cloned().unwrap_or_default(),
				opened_at: incident.opened_at,
				notified_at: *notified_at,
				open: incident.closed_at.is_none() && incident.resolved_at.is_none(),
			})
		})
		.collect()
}

/// Incidents whose opening notification was delivered in the span, with when.
async fn notified_between(
	conn: &mut AsyncPgConnection,
	from: Option<Timestamp>,
	to: Timestamp,
) -> Result<Vec<(Uuid, Timestamp)>> {
	let mut query = slack_outbox::table
		.filter(slack_outbox::kind.eq(KIND_INCIDENT_OPEN))
		.filter(slack_outbox::incident_id.is_not_null())
		.filter(slack_outbox::delivered_at.lt(jiff_diesel::Timestamp::from(to)))
		.into_boxed();
	if let Some(from) = from {
		query = query.filter(slack_outbox::delivered_at.ge(jiff_diesel::Timestamp::from(from)));
	}
	let rows: Vec<(Uuid, jiff_diesel::Timestamp)> = query
		.select((
			slack_outbox::incident_id.assume_not_null(),
			slack_outbox::delivered_at.assume_not_null(),
		))
		.order(slack_outbox::delivered_at)
		.limit(WRITE_UP_LIMIT)
		.load(conn)
		.await
		.map_err(AppError::from)?;
	let mut seen = HashSet::new();
	Ok(rows
		.into_iter()
		.filter(|(id, _)| seen.insert(*id))
		.map(|(id, at)| (id, at.to_jiff()))
		.collect())
}

async fn incidents_by_ids(
	conn: &mut AsyncPgConnection,
	ids: &[Uuid],
) -> Result<HashMap<Uuid, Incident>> {
	if ids.is_empty() {
		return Ok(HashMap::new());
	}
	let rows: Vec<Incident> = incidents::table
		.filter(incidents::id.eq_any(ids))
		.select(Incident::as_select())
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(rows.into_iter().map(|i| (i.id, i)).collect())
}

async fn ever_triaged(conn: &mut AsyncPgConnection, ids: &[Uuid]) -> Result<HashSet<Uuid>> {
	if ids.is_empty() {
		return Ok(HashSet::new());
	}
	let rows: Vec<Uuid> = alert_triage_verdicts::table
		.filter(alert_triage_verdicts::incident_id.eq_any(ids))
		.select(alert_triage_verdicts::incident_id)
		.distinct()
		.load(conn)
		.await
		.map_err(AppError::from)?;
	Ok(rows.into_iter().collect())
}

/// Whether alert triage is in use: a verdict has been recorded recently.
// spec: TRI#untriaged-incidents
pub async fn in_use(conn: &mut AsyncPgConnection, now: Timestamp) -> Result<bool> {
	let since = jiff_diesel::Timestamp::from(now - IN_USE_WITHIN);
	diesel::select(diesel::dsl::exists(
		alert_triage_verdicts::table.filter(alert_triage_verdicts::created_at.ge(since)),
	))
	.get_result(conn)
	.await
	.map_err(AppError::from)
}

/// Published incidents still open more than [`UNTRIAGED_AFTER`] past their
/// notification, with no verdict.
// spec: TRI#untriaged-incidents
pub async fn untriaged_overdue(
	conn: &mut AsyncPgConnection,
	now: Timestamp,
) -> Result<Vec<Untriaged>> {
	let notified = notified_between(conn, None, now - UNTRIAGED_AFTER).await?;
	let ids: Vec<Uuid> = notified.iter().map(|(id, _)| *id).collect();
	let incidents: HashMap<Uuid, Incident> = incidents_by_ids(conn, &ids)
		.await?
		.into_iter()
		.filter(|(_, i)| i.closed_at.is_none() && i.resolved_at.is_none())
		.collect();
	let notified: Vec<(Uuid, Timestamp)> = notified
		.into_iter()
		.filter(|(id, _)| incidents.contains_key(id))
		.collect();
	let open_ids: Vec<Uuid> = notified.iter().map(|(id, _)| *id).collect();
	let labels = target_labels(conn, incidents.values()).await?;
	let with_verdict = ever_triaged(conn, &open_ids).await?;
	Ok(untriaged_of(&notified, &incidents, &labels, &with_verdict))
}

#[cfg(test)]
mod tests {
	use super::*;

	fn ts(text: &str) -> Timestamp {
		text.parse().unwrap()
	}

	#[test]
	fn a_write_up_day_runs_from_8am_new_zealand_time_across_daylight_saving() {
		// Summer: NZDT, thirteen hours ahead.
		let (start, end) = write_up_day(civil::date(2026, 1, 10)).unwrap();
		assert_eq!(start, ts("2026-01-09T19:00:00Z"));
		assert_eq!(end, ts("2026-01-10T19:00:00Z"));
		// The day daylight saving ends (5 April 2026) is an hour longer.
		let (start, end) = write_up_day(civil::date(2026, 4, 4)).unwrap();
		assert_eq!(start, ts("2026-04-03T19:00:00Z"));
		assert_eq!(end, ts("2026-04-04T20:00:00Z"));
	}

	#[test]
	fn an_hour_before_8am_belongs_to_the_day_before() {
		assert_eq!(
			write_up_date(ts("2026-01-10T18:30:00Z")).unwrap(),
			civil::date(2026, 1, 10),
			"07:30 on the 11th in Auckland"
		);
		assert_eq!(
			write_up_date(ts("2026-01-10T19:00:00Z")).unwrap(),
			civil::date(2026, 1, 11)
		);
	}

	#[test]
	fn severity_ranks_an_escalating_failure_above_a_failure_above_a_warning() {
		assert_eq!(
			Severity::of(Some(CheckResult::Broken), true),
			Some(Severity::Warning)
		);
		assert!(Severity::EscalatingFailure > Severity::Failure);
		assert!(Severity::Failure > Severity::Warning);
		assert_eq!(Severity::of(Some(CheckResult::Passed), true), None);
	}
}
