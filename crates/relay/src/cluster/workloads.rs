//! Workloads broadly not running: the share of what the cluster's workloads ask
//! for that is ready (spec `K8S`, "Workloads broadly not running").
//!
//! Two halves, both pure so they can be exercised without a cluster: the share
//! is summed from what each kind of workload asks for and has ready, and the
//! [`Grader`] turns a stream of shares into a result through bands, hysteresis
//! and holds.

use std::time::{Duration, Instant};

use commons_types::status::CheckResult;

/// The annotation CNPG reads to hibernate a database cluster. A hibernated
/// cluster keeps naming its instances while the operator has removed their
/// pods, so it is left out rather than counted as not running.
pub const CNPG_HIBERNATION: &str = "cnpg.io/hibernation";

/// What one workload asks for and has ready.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Replicas {
	pub desired: u64,
	pub ready: u64,
}

impl Replicas {
	/// A workload's contribution. Readiness is capped at what it asks for, so a
	/// rollout surging above its replica count is not counted as more than
	/// fully running.
	pub fn new(desired: u64, ready: u64) -> Self {
		Self {
			desired,
			ready: ready.min(desired),
		}
	}
}

/// The cluster's workloads summed. A workload scaled to zero asks for nothing,
/// so it adds nothing to either side without needing to be recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Share {
	pub desired: u64,
	pub ready: u64,
}

impl Share {
	pub fn sum(workloads: impl IntoIterator<Item = Replicas>) -> Self {
		workloads.into_iter().fold(Self::default(), |acc, w| Self {
			desired: acc.desired + w.desired,
			ready: acc.ready + w.ready,
		})
	}

	/// The share ready, as a percentage. A cluster asking for nothing at all
	/// has nothing failing to run.
	pub fn percent(self) -> f64 {
		if self.desired == 0 {
			100.0
		} else {
			self.ready as f64 * 100.0 / self.desired as f64
		}
	}
}

/// A Deployment or a StatefulSet: its replica count, defaulting to one as the
/// API does, against its ready replicas.
pub fn scaled(replicas: Option<i32>, ready: Option<i32>) -> Replicas {
	Replicas::new(
		non_negative(replicas.unwrap_or(1)),
		non_negative(ready.unwrap_or(0)),
	)
}

/// A DaemonSet: the nodes it should be scheduled on, against those where it is
/// ready.
pub fn daemon(desired_scheduled: i32, ready: i32) -> Replicas {
	Replicas::new(non_negative(desired_scheduled), non_negative(ready))
}

/// A CNPG database cluster, or nothing where it is hibernated.
pub fn database(
	instances: Option<i64>,
	ready_instances: Option<i64>,
	hibernation: Option<&str>,
) -> Option<Replicas> {
	if hibernation == Some("on") {
		return None;
	}
	Some(Replicas::new(
		instances.unwrap_or(0).max(0) as u64,
		ready_instances.unwrap_or(0).max(0) as u64,
	))
}

fn non_negative(n: i32) -> u64 {
	n.max(0) as u64
}

/// The lines the share is graded against, and how long it must stay under
/// each before the result follows.
const PASSED_AT: f64 = 90.0;
const FAILED_BELOW: f64 = 80.0;
/// Crossing back over an edge takes this much more than falling across it.
const HYSTERESIS: f64 = 5.0;
const FAIL_AT_ONCE_BELOW: f64 = 50.0;
const FAIL_SOON_BELOW: f64 = 70.0;
const FAIL_SOON_AFTER: Duration = Duration::from_secs(2 * 60);
const FAIL_AFTER: Duration = Duration::from_secs(5 * 60);
const WARN_AFTER: Duration = Duration::from_secs(5 * 60);
/// The longest hold, which is how much history a freshly started relay needs
/// before any result is determined by it.
const LONGEST_HOLD: Duration = FAIL_AFTER;

/// Turns the share over time into the check's result.
///
/// Holds are each timed from when the share went under that line and stayed
/// there, and the most urgent that holds is the result. Recovery is not held,
/// but it is hysteretic: a warning passes again only above 95%, and a failure
/// lifts only above 85%.
#[derive(Debug, Clone)]
pub struct Grader {
	started: Instant,
	result: Option<CheckResult>,
	/// When the share went under 50, 70, 80 and 90, and has stayed there.
	under: [Option<Instant>; 4],
}

const LINES: [f64; 4] = [FAIL_AT_ONCE_BELOW, FAIL_SOON_BELOW, FAILED_BELOW, PASSED_AT];

impl Grader {
	pub fn new(started: Instant) -> Self {
		Self {
			started,
			result: None,
			under: [None; 4],
		}
	}

	/// The result for this observation, or `None` while a freshly started
	/// relay does not yet hold the history the result depends on.
	///
	/// Filing nothing is safe in that window: each substrate filing stands on
	/// its own, so canopy keeps whatever was last filed, and a relay restart
	/// neither recovers the check nor reopens it minutes later.
	pub fn observe(&mut self, percent: f64, now: Instant) -> Option<CheckResult> {
		for (line, since) in LINES.iter().zip(self.under.iter_mut()) {
			if percent < *line {
				// A relay that has just started has seen the share under the
				// line since it started, and no longer.
				since.get_or_insert(now.max(self.started));
			} else {
				*since = None;
			}
		}

		let held = |i: usize, hold: Duration| {
			self.under[i].is_some_and(|since| now.saturating_duration_since(since) >= hold)
		};
		let degraded =
			if percent < FAIL_AT_ONCE_BELOW || held(1, FAIL_SOON_AFTER) || held(2, FAIL_AFTER) {
				Some(CheckResult::Failed)
			} else if held(3, WARN_AFTER) {
				Some(CheckResult::Warning)
			} else {
				None
			};

		let result = match self.result {
			Some(last) => Some(most_urgent(recovered(last, percent), degraded)),
			// No history: only what does not depend on it, or a hold that has
			// run its full length since the relay started.
			None if percent > PASSED_AT + HYSTERESIS => Some(CheckResult::Passed),
			None if degraded.is_some() => degraded,
			None if now.saturating_duration_since(self.started) >= LONGEST_HOLD => {
				Some(band(percent))
			}
			None => None,
		};
		if result.is_some() {
			self.result = result;
		}
		result
	}
}

/// Where the last result goes on this share, recovering only past its edge by
/// the hysteresis margin.
fn recovered(last: CheckResult, percent: f64) -> CheckResult {
	match last {
		CheckResult::Failed if percent > PASSED_AT + HYSTERESIS => CheckResult::Passed,
		CheckResult::Failed if percent > FAILED_BELOW + HYSTERESIS => CheckResult::Warning,
		CheckResult::Warning if percent > PASSED_AT + HYSTERESIS => CheckResult::Passed,
		other => other,
	}
}

fn band(percent: f64) -> CheckResult {
	if percent >= PASSED_AT {
		CheckResult::Passed
	} else if percent >= FAILED_BELOW {
		CheckResult::Warning
	} else {
		CheckResult::Failed
	}
}

fn most_urgent(a: CheckResult, b: Option<CheckResult>) -> CheckResult {
	match b {
		Some(b) if b.urgency_rank() < a.urgency_rank() => b,
		_ => a,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	const MIN: Duration = Duration::from_secs(60);

	/// A grader that already holds a result, as it does once it has run a
	/// while.
	fn settled(result: CheckResult, at: Instant) -> Grader {
		let mut g = Grader::new(at - 10 * MIN);
		g.result = Some(result);
		g
	}

	/// Hold a share from `from` for `span`, observing every ten seconds, and
	/// return the last result.
	fn hold(g: &mut Grader, percent: f64, from: Instant, span: Duration) -> Option<CheckResult> {
		let mut last = None;
		let mut t = Duration::ZERO;
		while t <= span {
			last = g.observe(percent, from + t);
			t += Duration::from_secs(10);
		}
		last
	}

	#[test]
	fn the_share_is_ready_over_desired_across_workloads() {
		let share = Share::sum([
			scaled(Some(3), Some(3)),
			scaled(Some(2), Some(1)),
			daemon(4, 4),
			database(Some(3), Some(2), None).unwrap(),
		]);
		assert_eq!(
			share,
			Share {
				desired: 12,
				ready: 10
			}
		);
	}

	#[test]
	fn a_workload_whose_pods_cannot_be_created_counts_against_the_share() {
		// Quota exceeded or an admission webhook down: no pods exist at all,
		// and the workload still asks for its replicas.
		let share = Share::sum([scaled(Some(4), None), scaled(Some(4), Some(4))]);
		assert_eq!(share.percent(), 50.0);
	}

	#[test]
	fn a_workload_scaled_to_zero_adds_nothing() {
		assert_eq!(scaled(Some(0), None), Replicas::default());
		assert_eq!(
			database(Some(1), Some(0), Some("on")),
			None,
			"a hibernated database cluster is left out",
		);
		// A sleeping environment: scaled down and hibernated beside a running
		// one, which is all that counts.
		let share = Share::sum(
			[scaled(Some(0), None), scaled(Some(2), Some(2))]
				.into_iter()
				.chain(database(Some(1), Some(0), Some("on"))),
		);
		assert_eq!(share.percent(), 100.0);
	}

	#[test]
	fn a_surging_rollout_is_not_more_than_fully_running() {
		assert_eq!(scaled(Some(2), Some(3)), Replicas::new(2, 2));
	}

	#[test]
	fn a_cluster_asking_for_nothing_is_fully_running() {
		assert_eq!(Share::default().percent(), 100.0);
	}

	#[test]
	fn bands_follow_their_holds() {
		let t = Instant::now();
		let mut g = settled(CheckResult::Passed, t);
		assert_eq!(hold(&mut g, 89.0, t, 4 * MIN), Some(CheckResult::Passed));
		assert_eq!(g.observe(89.0, t + 5 * MIN), Some(CheckResult::Warning));

		let mut g = settled(CheckResult::Passed, t);
		assert_eq!(hold(&mut g, 79.0, t, 5 * MIN), Some(CheckResult::Failed));

		let mut g = settled(CheckResult::Passed, t);
		assert_eq!(
			hold(&mut g, 65.0, t, 90 * Duration::from_secs(1)),
			Some(CheckResult::Passed)
		);
		assert_eq!(g.observe(65.0, t + 2 * MIN), Some(CheckResult::Failed));

		let mut g = settled(CheckResult::Passed, t);
		assert_eq!(g.observe(45.0, t), Some(CheckResult::Failed));
	}

	#[test]
	fn a_shallow_dip_that_recovers_inside_its_hold_files_nothing_degraded() {
		let t = Instant::now();
		let mut g = settled(CheckResult::Passed, t);
		assert_eq!(hold(&mut g, 85.0, t, 4 * MIN), Some(CheckResult::Passed));
		assert_eq!(
			g.observe(97.0, t + 4 * MIN + Duration::from_secs(30)),
			Some(CheckResult::Passed)
		);
		// And the clock restarts after it: another dip is timed afresh.
		assert_eq!(
			hold(&mut g, 85.0, t + 5 * MIN, 4 * MIN),
			Some(CheckResult::Passed)
		);
	}

	#[test]
	fn crossing_back_over_an_edge_takes_the_hysteresis_margin() {
		let t = Instant::now();
		let mut g = settled(CheckResult::Warning, t);
		assert_eq!(g.observe(92.0, t), Some(CheckResult::Warning));
		assert_eq!(g.observe(96.0, t), Some(CheckResult::Passed));

		let mut g = settled(CheckResult::Failed, t);
		assert_eq!(g.observe(83.0, t), Some(CheckResult::Failed));
		assert_eq!(g.observe(86.0, t), Some(CheckResult::Warning));

		let mut g = settled(CheckResult::Failed, t);
		assert_eq!(
			g.observe(97.0, t),
			Some(CheckResult::Passed),
			"recovery is not held",
		);
	}

	#[test]
	fn a_fresh_relay_files_at_once_only_where_history_does_not_matter() {
		let t = Instant::now();
		assert_eq!(Grader::new(t).observe(97.0, t), Some(CheckResult::Passed));
		assert_eq!(Grader::new(t).observe(40.0, t), Some(CheckResult::Failed));
		assert_eq!(Grader::new(t).observe(92.0, t), None);
	}

	#[test]
	fn a_fresh_relay_in_the_middle_waits_out_the_governing_hold() {
		let t = Instant::now();
		let mut g = Grader::new(t);
		assert_eq!(hold(&mut g, 85.0, t, 4 * MIN), None, "nothing filed yet");
		assert_eq!(g.observe(85.0, t + 5 * MIN), Some(CheckResult::Warning));

		let mut g = Grader::new(t);
		assert_eq!(hold(&mut g, 65.0, t, 90 * Duration::from_secs(1)), None);
		assert_eq!(g.observe(65.0, t + 2 * MIN), Some(CheckResult::Failed));

		let mut g = Grader::new(t);
		assert_eq!(hold(&mut g, 92.0, t, 4 * MIN), None);
		assert_eq!(g.observe(92.0, t + 5 * MIN), Some(CheckResult::Passed));
	}
}
