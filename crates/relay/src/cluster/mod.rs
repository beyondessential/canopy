//! The checks the relay determines about its own cluster, filed under the
//! substrate source at the cluster grain (spec `K8S`, "Checks about the whole
//! cluster").
//!
//! Each check is read from the cluster's own objects through its API, never
//! through another monitoring system, so a check rests on the state of the
//! cluster itself. The relay watches what each check needs, holds the current
//! result, files when it changes, and refiles everything it holds every
//! minute, so canopy's view is re-established after a missed observation, a
//! restart, or a reconnection.
//!
//! The evaluators are pure functions of the objects they read, and the
//! Kubernetes plumbing is in [`watch`], so what a check concludes can be
//! exercised without a cluster.

use std::collections::BTreeMap;

use commons_types::status::CheckResult;
use relay_protocol::{Filing, FilingTarget, SubstrateFiling, SubstrateInstance, SubstrateOutcome};
use serde_json::json;

pub mod node_pools;
pub mod tailscale;
pub mod watch;
pub mod workloads;

/// What a check is, independent of any one observation of it: the fields a
/// filing carries so canopy's catalog entry registers already reviewed.
#[derive(Debug, Clone, Copy)]
pub struct CheckSpec {
	pub name: &'static str,
	/// The headline a degraded filing carries.
	pub title: &'static str,
	pub documentation: &'static str,
}

/// The relay grades these itself, so the catalog does not cap them; and a
/// cluster's issues open no incident, so escalation would be inert anyway.
const DEFAULT_CEILING: CheckResult = CheckResult::Failed;
const DEFAULT_ESCALATES: bool = false;

pub const NODE_POOLS: CheckSpec = CheckSpec {
	name: "node-pools",
	title: "A node pool cannot provision nodes",
	documentation: include_str!("docs/node-pools.md"),
};

pub const TAILSCALE_API_PROXY: CheckSpec = CheckSpec {
	name: "tailscale-api-proxy",
	title: "Operators cannot reach the cluster's API over the tailnet",
	documentation: include_str!("docs/tailscale-api-proxy.md"),
};

pub const WORKLOADS_RUNNING: CheckSpec = CheckSpec {
	name: "workloads-running",
	title: "Much of the cluster's workload is not running",
	documentation: include_str!("docs/workloads-running.md"),
};

/// One observation of a check: what the relay determined of it, and what an
/// operator reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Determination {
	pub outcome: SubstrateOutcome,
	/// What an operator reads, for a check that holds once or could not be
	/// read. `None` for a check with instances, whose message canopy writes
	/// from the instances as it graded them.
	pub message: Option<String>,
}

impl Determination {
	/// A check that holds once, with what an operator reads of it.
	pub fn once(
		observed: CheckResult,
		detail: serde_json::Value,
		message: impl Into<String>,
	) -> Self {
		Self {
			outcome: SubstrateOutcome::Instances(vec![SubstrateInstance::only(
				observed,
				Some(detail),
			)]),
			message: Some(message.into()),
		}
	}

	/// A check with instances. Canopy names the degraded ones in the message
	/// it writes, so the relay supplies none.
	pub fn instances(instances: Vec<SubstrateInstance>) -> Self {
		Self {
			outcome: SubstrateOutcome::Instances(instances),
			message: None,
		}
	}

	/// A check the relay could not read. Brokenness is the whole check's, so
	/// it carries no instance: canopy keeps the instances it held.
	pub fn broken(detail: serde_json::Value, message: impl Into<String>) -> Self {
		Self {
			outcome: SubstrateOutcome::Broken {
				detail: Some(detail),
			},
			message: Some(message.into()),
		}
	}

	/// The one instance of a check that holds once and was read.
	#[cfg(test)]
	pub(crate) fn single(&self) -> &SubstrateInstance {
		match &self.outcome {
			SubstrateOutcome::Instances(instances) if instances.len() == 1 => &instances[0],
			other => panic!("expected a check that holds once, got {other:?}"),
		}
	}

	/// A check the relay could not read because a permission it needed was
	/// not granted. Broken rather than absent: absent would say the condition
	/// does not exist on this cluster, which the relay cannot know.
	pub fn refused(refusal: &Refusal) -> Self {
		Self::broken(
			json!({
				"refused": { "verb": refusal.verb, "resource": refusal.resource },
				"message": refusal.message,
			}),
			format!(
				"the relay is not permitted to {} {}",
				refusal.verb, refusal.resource
			),
		)
	}
}

/// A permission the cluster refused the relay.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
	pub verb: &'static str,
	pub resource: String,
	pub message: String,
}

impl CheckSpec {
	/// The filing for one observation of this check.
	pub fn filing(&self, determination: Determination) -> Filing {
		Filing::Substrate(SubstrateFiling {
			target: FilingTarget::Cluster,
			check: self.name.into(),
			outcome: determination.outcome,
			title: Some(self.title.into()),
			message: determination.message,
			default_ceiling: DEFAULT_CEILING,
			default_escalates: DEFAULT_ESCALATES,
			documentation: Some(self.documentation.into()),
		})
	}
}

/// The current result of each check, as last determined.
///
/// Both halves of the filing discipline read from here: a change is filed at
/// once, and the refile sends everything held.
#[derive(Debug, Default)]
pub struct Held {
	checks: BTreeMap<&'static str, (CheckSpec, Determination)>,
}

impl Held {
	/// Hold a check's latest determination, returning its filing if its result
	/// changed and it should be filed now.
	///
	/// A change is one an operator acts on: an instance appearing, going, or
	/// changing result, or the check becoming readable or unreadable. Detail
	/// that moves without the result moving (a ready count ticking during a
	/// rollout) is held and carried by the next refile, so a busy cluster does
	/// not file on every pod.
	pub fn set(&mut self, spec: CheckSpec, determination: Determination) -> Option<Filing> {
		let changed = self
			.checks
			.get(spec.name)
			.is_none_or(|(_, held)| results(held) != results(&determination));
		self.checks.insert(spec.name, (spec, determination.clone()));
		changed.then(|| spec.filing(determination))
	}

	/// Every held check's filing, for the refile.
	pub fn all(&self) -> Vec<Filing> {
		self.checks
			.values()
			.map(|(spec, d)| spec.filing(d.clone()))
			.collect()
	}
}

/// Each instance's result by key, or `None` for a check that could not be
/// read.
fn results(d: &Determination) -> Option<Vec<(&str, CheckResult)>> {
	let SubstrateOutcome::Instances(instances) = &d.outcome else {
		return None;
	};
	let mut r: Vec<(&str, CheckResult)> = instances
		.iter()
		.map(|i| (i.key.as_str(), i.observed))
		.collect();
	r.sort_unstable_by(|a, b| a.0.cmp(b.0));
	Some(r)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn only(result: CheckResult, detail: serde_json::Value) -> Determination {
		Determination::once(result, detail, "m")
	}

	#[test]
	fn a_first_determination_files() {
		let mut held = Held::default();
		assert!(
			held.set(WORKLOADS_RUNNING, only(CheckResult::Passed, json!({})))
				.is_some()
		);
	}

	#[test]
	fn a_changed_result_files_and_an_unchanged_one_waits_for_the_refile() {
		let mut held = Held::default();
		held.set(
			WORKLOADS_RUNNING,
			only(CheckResult::Passed, json!({"ready": 10})),
		);
		assert!(
			held.set(
				WORKLOADS_RUNNING,
				only(CheckResult::Passed, json!({"ready": 11}))
			)
			.is_none(),
			"detail moving without the result is not a change",
		);
		assert!(
			held.set(
				WORKLOADS_RUNNING,
				only(CheckResult::Failed, json!({"ready": 2}))
			)
			.is_some()
		);

		// The refile carries the latest detail.
		let all: [Filing; 1] = held.all().try_into().unwrap();
		let [Filing::Substrate(filing)] = all else {
			panic!("one substrate filing held");
		};
		let SubstrateOutcome::Instances(instances) = &filing.outcome else {
			panic!("the check was read");
		};
		assert_eq!(instances[0].detail, Some(json!({"ready": 2})));
		assert_eq!(filing.check, "workloads-running");
		assert_eq!(filing.target, FilingTarget::Cluster);
	}

	#[test]
	fn an_instance_appearing_is_a_change() {
		let mut held = Held::default();
		let pool = |name: &str| SubstrateInstance {
			key: name.into(),
			label: None,
			observed: CheckResult::Passed,
			detail: None,
		};
		held.set(NODE_POOLS, Determination::instances(vec![pool("general")]));
		assert!(
			held.set(
				NODE_POOLS,
				Determination::instances(vec![pool("general"), pool("gpu")]),
			)
			.is_some()
		);
	}

	#[test]
	fn becoming_unreadable_is_a_change_and_staying_so_is_not() {
		let mut held = Held::default();
		let refused = Refusal {
			verb: "list",
			resource: "nodepools.karpenter.sh".into(),
			message: "forbidden".into(),
		};
		held.set(
			NODE_POOLS,
			Determination::instances(vec![SubstrateInstance {
				key: "general".into(),
				label: None,
				observed: CheckResult::Passed,
				detail: None,
			}]),
		);
		assert!(
			held.set(NODE_POOLS, Determination::refused(&refused))
				.is_some()
		);
		assert!(
			held.set(NODE_POOLS, Determination::refused(&refused))
				.is_none()
		);
	}

	#[test]
	fn a_refused_permission_breaks_the_whole_check_and_names_the_refusal() {
		let d = Determination::refused(&Refusal {
			verb: "list",
			resource: "nodepools.karpenter.sh".into(),
			message: "forbidden".into(),
		});
		let SubstrateOutcome::Broken {
			detail: Some(detail),
		} = &d.outcome
		else {
			panic!("a check the relay cannot read is broken at check level, not in an instance");
		};
		assert_eq!(detail["refused"]["resource"], "nodepools.karpenter.sh");
		assert!(
			d.message
				.as_deref()
				.unwrap()
				.contains("nodepools.karpenter.sh")
		);
	}
}
