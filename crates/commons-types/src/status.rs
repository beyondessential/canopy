use std::fmt::Display;

use serde::{Deserialize, Serialize};

use crate::namespace::NamespaceRef;

/// How a server should treat one of its healthchecks, distilled from
/// canopy's operator-side configuration (the policy catalog and the
/// silences) into a three-level device-facing vocabulary.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckSeverity {
	/// This check's failures are ignored on the canopy side — it is
	/// silenced for this server, or its policy grades it below warning.
	Skip,
	/// This check's failures are treated as warnings.
	Warn,
	/// This check's failures are treated as errors (incident-opening).
	Fail,
}

impl From<CheckResult> for CheckSeverity {
	/// Distill a policy ceiling into the device-facing vocabulary: a
	/// `failed` ceiling means failures count as failures, `warning` and
	/// `broken` cap below that, and `passed`/`skipped` mean the check
	/// never alerts (so the device may skip it).
	fn from(ceiling: CheckResult) -> Self {
		match ceiling {
			CheckResult::Failed => Self::Fail,
			CheckResult::Warning | CheckResult::Broken => Self::Warn,
			CheckResult::Passed | CheckResult::Skipped => Self::Skip,
		}
	}
}

impl Display for CheckSeverity {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			CheckSeverity::Skip => write!(f, "skip"),
			CheckSeverity::Warn => write!(f, "warn"),
			CheckSeverity::Fail => write!(f, "fail"),
		}
	}
}

/// Reachability of a target, from how recently it last reported.
///
/// Three states and no degrees between them: a target is reachable,
/// unreachable, or has never reported. How long it has been quiet is measured
/// against that target's own configured threshold rather than any fixed one, so
/// a target that reports every few minutes and one that reports hourly are
/// each judged on what is normal for them.
// spec: CHK#reachability
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ShortStatus {
	/// Something is currently reporting about the target: its last report is
	/// within its own down threshold.
	Up,
	/// Nothing is reporting about the target any more — it is unreachable.
	Down,
	/// No status has ever been reported for this target.
	#[default]
	Gone,
}

impl ShortStatus {
	/// Grade a target from when it last reported and its own down threshold.
	///
	/// `last_reported_at` is when anything last reported about the target,
	/// however long ago, so `None` means never — not merely nothing recent.
	/// Sourcing it from a windowed read of status history makes a target
	/// quiet for longer than the window read as never heard from, which is
	/// the state that outranks every other on every surface.
	///
	/// Every grain grades the same way. Which threshold applies is the
	/// target's own, which is why the models call this rather than each
	/// caller reaching for a clock of its own.
	// spec: CHK#reachability
	pub fn grade(
		last_reported_at: Option<jiff::Timestamp>,
		down_after: jiff::SignedDuration,
	) -> Self {
		last_reported_at.map_or(Self::Gone, |at| {
			if at.duration_since(jiff::Timestamp::now()).abs() >= down_after {
				Self::Down
			} else {
				Self::Up
			}
		})
	}
}

impl Display for ShortStatus {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			ShortStatus::Up => write!(f, "up"),
			ShortStatus::Down => write!(f, "down"),
			ShortStatus::Gone => write!(f, "gone"),
		}
	}
}

/// Outcome of a single health check reported in a server's status update.
///
/// Older reports may send a plain pass/fail flag instead of one of these
/// outcomes; when that happens a passing flag is treated as `passed` and a
/// failing flag as `failed`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum CheckResult {
	/// The check ran and the system it tested is healthy.
	Passed,
	/// The check ran and the system it tested is degraded but not failing.
	Warning,
	/// The check ran and the system it tested is unhealthy.
	Failed,
	/// The check itself failed to run or is misconfigured; this says nothing
	/// about the health of the system it was meant to test.
	Broken,
	/// A precondition for the check wasn't met, so it didn't run.
	Skipped,
}

impl CheckResult {
	/// Normalise a `health[]` entry to its result. Prefers a valid
	/// `result` string, falls back to the legacy `healthy: bool`
	/// (true → Passed, false → Failed). Returns `None` for malformed
	/// entries (neither field readable) — callers ignore those, same
	/// as the historical behaviour for entries missing `healthy`.
	pub fn from_entry(entry: &serde_json::Map<String, serde_json::Value>) -> Option<Self> {
		if let Some(result) = entry.get("result").and_then(|v| v.as_str()) {
			return result.parse().ok();
		}
		entry
			.get("healthy")
			.and_then(|v| v.as_bool())
			.map(|healthy| if healthy { Self::Passed } else { Self::Failed })
	}
}

impl CheckResult {
	/// Position on the urgency ordering used by policy ceilings and
	/// display sorting: failed > warning > broken > passed > skipped
	/// (lower rank = more urgent).
	pub fn urgency_rank(self) -> u8 {
		match self {
			CheckResult::Failed => 0,
			CheckResult::Warning => 1,
			CheckResult::Broken => 2,
			CheckResult::Passed => 3,
			CheckResult::Skipped => 4,
		}
	}

	/// Cap this result at `ceiling` on the urgency ordering: a result
	/// more urgent than the ceiling grades down to it; anything at or
	/// below the ceiling passes through unchanged.
	pub fn capped_at(self, ceiling: CheckResult) -> CheckResult {
		if self.urgency_rank() < ceiling.urgency_rank() {
			ceiling
		} else {
			self
		}
	}
}

impl Display for CheckResult {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			CheckResult::Passed => write!(f, "passed"),
			CheckResult::Warning => write!(f, "warning"),
			CheckResult::Failed => write!(f, "failed"),
			CheckResult::Broken => write!(f, "broken"),
			CheckResult::Skipped => write!(f, "skipped"),
		}
	}
}

impl TryFrom<String> for CheckResult {
	type Error = String;

	fn try_from(value: String) -> Result<Self, String> {
		value.parse()
	}
}

impl From<CheckResult> for String {
	fn from(result: CheckResult) -> Self {
		result.to_string()
	}
}

impl std::str::FromStr for CheckResult {
	type Err = String;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s {
			"passed" => Ok(CheckResult::Passed),
			"warning" => Ok(CheckResult::Warning),
			"failed" => Ok(CheckResult::Failed),
			"broken" => Ok(CheckResult::Broken),
			"skipped" => Ok(CheckResult::Skipped),
			other => Err(format!(
				"unknown check result '{other}' (expected passed, warning, failed, broken, or skipped)"
			)),
		}
	}
}

/// One person currently connected to a server, identified by their
/// Tailscale login.
///
/// Only sessions that could be tied to an authenticated Tailscale identity
/// are reported here — local console access and other unauthenticated
/// sessions don't produce one of these. A person with several simultaneous
/// sessions appears once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct OperatorPresence {
	/// The person's Tailscale login (an email address).
	pub login: String,
	/// The person's display name, if known. `None` if this login has never
	/// been seen before.
	pub name: Option<String>,
	/// URL of the person's profile picture, if known.
	pub profile_pic: Option<String>,
	/// When the person's current connection began. If they have multiple
	/// simultaneous sessions, this is the earliest start time among them.
	pub connected_since: Option<jiff::Timestamp>,
}

/// Distil a status row's `health[]` into the set of identified operators.
///
/// Finds the `external_users` entry and reads its sessions. Lenient on shape:
/// a malformed entry yields nobody rather than an error.
pub fn operators_from_health(health: &serde_json::Value) -> Vec<OperatorPresence> {
	let Some(entry) = health
		.as_array()
		.into_iter()
		.flatten()
		.filter_map(|e| e.as_object())
		.find(|e| e.get("check").and_then(|v| v.as_str()) == Some("external_users"))
	else {
		return Vec::new();
	};
	operators_from_sessions(entry.get("users"))
}

/// Distil the `users[]` of an `external_users` check into identified
/// operators.
///
/// The sessions are the same whichever way the check is read — out of a
/// status row's `health[]` or out of the detail on a stored check state — so
/// both go through here and a person reads the same in either place.
///
/// Collects the sessions carrying a `tailscale` login, deduplicating by login
/// and keeping the earliest `connected_since`. Display fields are left
/// unfilled: looking up the `tailscale_users` cache is the private-server's
/// job. Lenient on shape, so a malformed session is skipped rather than
/// losing the rest.
pub fn operators_from_sessions(users: Option<&serde_json::Value>) -> Vec<OperatorPresence> {
	let Some(users) = users.and_then(|u| u.as_array()) else {
		return Vec::new();
	};

	let mut out: Vec<OperatorPresence> = Vec::new();
	for session in users.iter().filter_map(|u| u.as_object()) {
		let Some(login) = session.get("tailscale").and_then(|v| v.as_str()) else {
			continue;
		};
		let since = session
			.get("connected_since")
			.and_then(|v| v.as_str())
			.and_then(|s| s.parse::<jiff::Timestamp>().ok());
		if let Some(existing) = out.iter_mut().find(|o| o.login == login) {
			existing.connected_since = match (existing.connected_since, since) {
				(Some(a), Some(b)) => Some(a.min(b)),
				(a, b) => a.or(b),
			};
		} else {
			out.push(OperatorPresence {
				login: login.to_string(),
				name: None,
				profile_pic: None,
				connected_since: since,
			});
		}
	}
	out
}

#[cfg(test)]
mod tests {
	use super::CheckResult;

	fn entry(json: serde_json::Value) -> serde_json::Map<String, serde_json::Value> {
		json.as_object().unwrap().clone()
	}

	#[test]
	fn from_entry_reads_result() {
		for (s, expected) in [
			("passed", CheckResult::Passed),
			("warning", CheckResult::Warning),
			("failed", CheckResult::Failed),
			("broken", CheckResult::Broken),
			("skipped", CheckResult::Skipped),
		] {
			assert_eq!(
				CheckResult::from_entry(&entry(serde_json::json!({"result": s}))),
				Some(expected),
			);
		}
	}

	#[test]
	fn from_entry_falls_back_to_legacy_healthy() {
		assert_eq!(
			CheckResult::from_entry(&entry(serde_json::json!({"healthy": true}))),
			Some(CheckResult::Passed),
		);
		assert_eq!(
			CheckResult::from_entry(&entry(serde_json::json!({"healthy": false}))),
			Some(CheckResult::Failed),
		);
	}

	#[test]
	fn from_entry_result_wins_over_healthy() {
		assert_eq!(
			CheckResult::from_entry(&entry(
				serde_json::json!({"result": "broken", "healthy": true})
			)),
			Some(CheckResult::Broken),
		);
	}

	#[test]
	fn from_entry_malformed_is_none() {
		assert_eq!(CheckResult::from_entry(&entry(serde_json::json!({}))), None);
		assert_eq!(
			CheckResult::from_entry(&entry(serde_json::json!({"result": "exploded"}))),
			None,
		);
		assert_eq!(
			CheckResult::from_entry(&entry(serde_json::json!({"healthy": "yes"}))),
			None,
		);
	}

	mod operators {
		use crate::status::operators_from_health;

		#[test]
		fn collects_tailscale_logins_in_order() {
			let health = serde_json::json!([
				{"check": "postgres", "result": "passed"},
				{"check": "external_users", "result": "passed", "count": 2, "users": [
					{"name": "ubuntu", "line": "pts/0", "tailscale": "alice@example.com",
					 "connected_since": "2026-06-01T03:56:40.073731977Z"},
					{"name": "ubuntu", "line": "pts/1", "tailscale": "bob@example.com",
					 "connected_since": "2026-06-01T04:01:53Z"},
				]},
			]);
			let ops = operators_from_health(&health);
			assert_eq!(ops.len(), 2);
			assert_eq!(ops[0].login, "alice@example.com");
			assert_eq!(ops[1].login, "bob@example.com");
			assert_eq!(
				ops[0].connected_since,
				Some("2026-06-01T03:56:40.073731977Z".parse().unwrap()),
			);
			assert!(ops[0].name.is_none());
			assert!(ops[0].profile_pic.is_none());
		}

		#[test]
		fn dedupes_by_login_keeping_earliest_since() {
			// Same person on three ttys; the middle session has the
			// earliest connected_since and must win regardless of order.
			let health = serde_json::json!([
				{"check": "external_users", "result": "passed", "users": [
					{"line": "pts/3", "tailscale": "chris@example.com",
					 "connected_since": "2026-06-02T03:26:00Z"},
					{"line": "pts/8", "tailscale": "chris@example.com",
					 "connected_since": "2026-06-01T06:58:49Z"},
					{"line": "pts/9", "tailscale": "chris@example.com",
					 "connected_since": "2026-06-04T23:47:00Z"},
				]},
			]);
			let ops = operators_from_health(&health);
			assert_eq!(ops.len(), 1);
			assert_eq!(
				ops[0].connected_since,
				Some("2026-06-01T06:58:49Z".parse().unwrap()),
			);
		}

		#[test]
		fn skips_sessions_without_tailscale_identity() {
			let health = serde_json::json!([
				{"check": "external_users", "result": "passed", "users": [
					{"name": "root", "line": "tty1"},
					{"name": "ubuntu", "line": "pts/0", "source": "203.0.113.5"},
					{"name": "ubuntu", "line": "pts/1", "tailscale": "alice@example.com"},
				]},
			]);
			let ops = operators_from_health(&health);
			assert_eq!(ops.len(), 1);
			assert_eq!(ops[0].login, "alice@example.com");
			assert_eq!(ops[0].connected_since, None);
		}

		#[test]
		fn unparseable_since_does_not_drop_the_operator() {
			let health = serde_json::json!([
				{"check": "external_users", "result": "passed", "users": [
					{"tailscale": "alice@example.com", "connected_since": "yesterday-ish"},
					{"tailscale": "alice@example.com", "connected_since": "2026-06-01T06:58:49Z"},
				]},
			]);
			let ops = operators_from_health(&health);
			assert_eq!(ops.len(), 1);
			assert_eq!(
				ops[0].connected_since,
				Some("2026-06-01T06:58:49Z".parse().unwrap()),
			);
		}

		#[test]
		fn tolerates_absent_or_malformed_shapes() {
			for health in [
				serde_json::json!(null),
				serde_json::json!([]),
				serde_json::json!([{"check": "postgres", "result": "passed"}]),
				serde_json::json!([{"check": "external_users", "result": "skipped"}]),
				serde_json::json!([{"check": "external_users", "users": "not-an-array"}]),
				serde_json::json!([{"check": "external_users", "users": [42, "str", null]}]),
			] {
				assert_eq!(operators_from_health(&health), Vec::new(), "for {health}");
			}
		}
	}

	/// A person is logged in to a box, so the sessions read are the box's.
	mod consolidated_operators {
		use crate::status::{CheckResult, ConsolidatedCheck, ConsolidatedChecks, HealthState};

		pub(super) fn check(name: &str, logins: &[&str]) -> ConsolidatedCheck {
			let users: Vec<serde_json::Value> = logins
				.iter()
				.map(|l| serde_json::json!({"tailscale": l}))
				.collect();
			ConsolidatedCheck {
				source: "alertd".into(),
				check: name.into(),
				namespace: (&crate::namespace::Namespace::of("alertd", None)).into(),
				qualified_name: name.into(),
				observed: Some(CheckResult::Passed),
				effective: CheckResult::Passed,
				silenced: false,
				last_reported_at: None,
				quiet: false,
				detail: serde_json::json!({ "users": users }),
				instances: Vec::new(),
				passing_instances: 0,
				skipped_instances: 0,
			}
		}

		fn checks(checks: Vec<ConsolidatedCheck>) -> ConsolidatedChecks {
			ConsolidatedChecks {
				health_state: HealthState::Healthy,
				checks,
			}
		}

		#[test]
		fn reads_the_machines_sessions() {
			let c = checks(vec![check(
				"external_users",
				&["alice@example.com", "bob@example.com"],
			)]);
			let ops = c.operators();
			assert_eq!(ops.len(), 2);
			assert_eq!(ops[0].login, "alice@example.com");
		}

		#[test]
		fn nobody_when_the_check_is_absent() {
			let c = checks(vec![check("load", &["alice@example.com"])]);
			assert_eq!(c.operators(), Vec::new());
		}
	}

	/// Most urgent first, then by the name a check presents as: an application
	/// type's `tamanu-central:caddy_certs` sorts after a bare `caddy_version`.
	mod consolidated_order {
		use super::consolidated_operators::check;
		use crate::status::{CheckResult, ConsolidatedChecks};

		fn named(
			qualified: &str,
			source: &str,
			effective: CheckResult,
		) -> crate::status::ConsolidatedCheck {
			let mut c = check(qualified, &[]);
			c.qualified_name = qualified.into();
			c.source = source.into();
			c.effective = effective;
			c
		}

		#[test]
		fn by_result_then_presented_name_then_source() {
			let mut checks = vec![
				named("tamanu-central:caddy_certs", "alertd", CheckResult::Passed),
				named("reachability", "canopy", CheckResult::Skipped),
				named("caddy_version", "alertd", CheckResult::Passed),
				named(
					"tamanu-central:sync_facility_stale",
					"alertd",
					CheckResult::Failed,
				),
				named("caddy_version", "aaa", CheckResult::Passed),
				named("backup-staleness", "canopy", CheckResult::Warning),
			];
			ConsolidatedChecks::sort(&mut checks);
			let order: Vec<(&str, &str)> = checks
				.iter()
				.map(|c| (c.qualified_name.as_str(), c.source.as_str()))
				.collect();
			assert_eq!(
				order,
				vec![
					("tamanu-central:sync_facility_stale", "alertd"),
					("backup-staleness", "canopy"),
					("caddy_version", "aaa"),
					("caddy_version", "alertd"),
					("tamanu-central:caddy_certs", "alertd"),
					("reachability", "canopy"),
				]
			);
		}
	}
}

/// A server's self-reported health, derived from the outcomes of its own
/// health checks.
///
/// This is independent of reachability: a server can be online and
/// reachable while reporting itself unhealthy, or unreachable while its
/// last report was healthy.
#[derive(
	Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum HealthState {
	/// Every health check passed or was skipped. Also the default when no
	/// status has ever been reported for a server, since there's no signal
	/// to say otherwise.
	#[default]
	Healthy,
	/// At least one health check reported a warning. Worth investigating,
	/// but not considered an incident.
	Warning,
	/// At least one health check failed. Considered an incident.
	Unhealthy,
}

/// One check in a server's consolidated state: a single check identity with
/// its results already graded by policy, ready to present. The same shape
/// whether taken from current state or reconstructed as of a past time, and
/// across every reporting source — the presentation never sees a single
/// source's raw report.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConsolidatedCheck {
	/// The source that reports this check.
	pub source: String,
	/// The check's name, as reported.
	pub check: String,
	/// Which catalog entry this check resolves to. Two application types
	/// reporting one name are two checks, so the name alone does not address
	/// a policy, a silence or a document — this does.
	pub namespace: NamespaceRef,
	/// How the check reads to an operator: `<type>:<check>` where it is one
	/// application type's, the bare name otherwise.
	pub qualified_name: String,
	/// What the source reported, before policy. `None` if the stored state
	/// carried no observed result.
	#[schema(value_type = Option<String>)]
	pub observed: Option<CheckResult>,
	/// The result after policy grading — what everything acts on and what
	/// the presentation colours by.
	#[schema(value_type = String)]
	pub effective: CheckResult,
	/// Whether this check is silenced at server or group scope.
	pub silenced: bool,
	/// When the check's source last reported it about this target. `None`
	/// for a state Canopy presents without one having been filed, such as a
	/// reachability that has never degraded.
	pub last_reported_at: Option<jiff::Timestamp>,
	/// Whether the check's source has gone quiet about this target: its last
	/// report is older than the target's down threshold, on the same clock
	/// reachability reads. The check is presented at its last result, muted.
	/// Always false for Canopy's own determinations and manual conditions,
	/// which no report carries.
	// spec: CHK#presentation
	pub quiet: bool,
	/// The detail the source attached to the check (its extra fields), as an
	/// object. Empty object when the check carried none. For a check with
	/// instances, the fields its instances share; each instance's own are on
	/// the instance.
	#[schema(value_type = Object)]
	pub detail: serde_json::Value,
	/// The check's degraded and silenced instances, most urgent first, each
	/// graded on its own. Empty for a check without instances, and for one
	/// none of whose instances is degraded or silenced.
	// spec: CHK#silencing-one-instance
	#[serde(default)]
	pub instances: Vec<ConsolidatedInstance>,
	/// How many of the check's instances passed, counted rather than listed.
	/// 0 for a check without instances.
	#[serde(default)]
	pub passing_instances: usize,
	/// How many of the check's instances were skipped other than by a silence
	/// (reported skipped, or graded skipped by a rule), counted rather than
	/// listed. Silenced instances are listed in `instances` instead, and a
	/// check silenced whole counts none. 0 for a check without instances.
	// spec: CHK#silencing-one-instance
	#[serde(default)]
	pub skipped_instances: usize,
}

/// One instance of a check, as a target's checks present it: one of a
/// central's devices, one of a machine's backup types.
// spec: CHK#checks-with-instances
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConsolidatedInstance {
	/// Which instance this is, unique within the check on its target. An
	/// instance silence names it.
	pub key: String,
	/// How the instance is named to an operator. Absent where the instance
	/// carries none, in which case it is named by its key.
	pub label: Option<String>,
	/// What the instance observed, before policy.
	#[schema(value_type = String)]
	pub observed: CheckResult,
	/// The instance's result after policy.
	#[schema(value_type = String)]
	pub effective: CheckResult,
	/// The instance's own fields, as an object; the ones it shares with the
	/// check's other instances are the check's `detail`.
	#[schema(value_type = Object)]
	pub detail: serde_json::Value,
	/// Whether an instance silence at the check's own target (its application,
	/// machine or cluster) quiets this instance.
	pub silenced_on_target: bool,
	/// Whether an instance silence at the target's group quiets this instance.
	pub silenced_on_group: bool,
}

impl ConsolidatedInstance {
	/// Whether an instance silence at either scope quiets this instance.
	pub fn silenced(&self) -> bool {
		self.silenced_on_target || self.silenced_on_group
	}
}

/// A server's checks across every source, graded and classified as one —
/// current or as of a past time.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct ConsolidatedChecks {
	/// The rolled-up health over these checks, by the one classifier.
	pub health_state: HealthState,
	/// Every source's checks, most urgent first, then by presented name.
	// spec: CHK#presentation
	pub checks: Vec<ConsolidatedCheck>,
}

impl ConsolidatedChecks {
	/// The people logged in to a machine right now, from its `external_users`
	/// check. Read from a machine's own checks: people are logged in to a box,
	/// and a box carrying two workloads has one set of sessions rather than one
	/// per workload, which is what the machine grain is for.
	// spec: FLT
	pub fn operators(&self) -> Vec<OperatorPresence> {
		self.checks
			.iter()
			.find(|c| c.check == "external_users")
			.map(|c| operators_from_sessions(c.detail.get("users")))
			.unwrap_or_default()
	}

	/// Order checks for presentation: most urgent effective result first, then
	/// by the name they present as, then by source.
	// spec: CHK#presentation
	pub fn sort(checks: &mut [ConsolidatedCheck]) {
		checks.sort_by(|a, b| {
			a.effective
				.urgency_rank()
				.cmp(&b.effective.urgency_rank())
				.then_with(|| a.qualified_name.cmp(&b.qualified_name))
				.then_with(|| a.source.cmp(&b.source))
		});
	}
}

impl HealthState {
	/// Roll a set of effective check results into a server health state:
	/// any failure ⇒ unhealthy; otherwise any warning or brokenness ⇒
	/// warning; otherwise healthy. Passed and skipped results don't count.
	/// The single source of truth for every health rollup — callers filter
	/// out silenced/resolved/decommissioned checks first, then pass the
	/// surviving effective results.
	pub fn from_results(results: impl IntoIterator<Item = CheckResult>) -> Self {
		let mut state = HealthState::Healthy;
		for result in results {
			match result {
				CheckResult::Failed => return HealthState::Unhealthy,
				CheckResult::Warning | CheckResult::Broken => state = HealthState::Warning,
				CheckResult::Passed | CheckResult::Skipped => {}
			}
		}
		state
	}
}

#[cfg(test)]
mod health_state_tests {
	use super::{CheckResult, HealthState};

	#[test]
	fn any_failure_is_unhealthy() {
		assert_eq!(
			HealthState::from_results([
				CheckResult::Passed,
				CheckResult::Warning,
				CheckResult::Failed,
			]),
			HealthState::Unhealthy,
		);
	}

	#[test]
	fn warning_or_broken_without_failure_is_warning() {
		assert_eq!(
			HealthState::from_results([CheckResult::Passed, CheckResult::Broken]),
			HealthState::Warning,
		);
		assert_eq!(
			HealthState::from_results([CheckResult::Warning, CheckResult::Passed]),
			HealthState::Warning,
		);
	}

	#[test]
	fn only_passed_and_skipped_is_healthy() {
		assert_eq!(
			HealthState::from_results([CheckResult::Passed, CheckResult::Skipped]),
			HealthState::Healthy,
		);
		assert_eq!(HealthState::from_results([]), HealthState::Healthy);
	}
}
