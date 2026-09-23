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
use relay_protocol::{Filing, FilingTarget, SubstrateFiling, SubstrateInstance};
use serde_json::json;

pub mod node_pools;
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

pub const WORKLOADS_RUNNING: CheckSpec = CheckSpec {
	name: "workloads-running",
	title: "Much of the cluster's workload is not running",
	documentation: include_str!("docs/workloads-running.md"),
};

/// One observation of a check: its instances and what an operator reads.
#[derive(Debug, Clone, PartialEq)]
pub struct Determination {
	pub instances: Vec<SubstrateInstance>,
	pub message: String,
}

impl Determination {
	/// A check the relay could not read because a permission it needed was
	/// not granted. Broken rather than absent: absent would say the condition
	/// does not exist on this cluster, which the relay cannot know.
	pub fn refused(refusal: &Refusal) -> Self {
		Self {
			instances: vec![SubstrateInstance::only(
				CheckResult::Broken,
				Some(json!({
					"refused": { "verb": refusal.verb, "resource": refusal.resource },
					"message": refusal.message,
				})),
			)],
			message: format!(
				"the relay is not permitted to {} {}",
				refusal.verb, refusal.resource
			),
		}
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
			instances: determination.instances,
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
	/// changing result. Detail that moves without the result moving (a ready
	/// count ticking during a rollout) is held and carried by the next
	/// refile, so a busy cluster does not file on every pod.
	pub fn set(&mut self, spec: CheckSpec, determination: Determination) -> Option<Filing> {
		let changed = self
			.checks
			.get(spec.name)
			.is_none_or(|(_, held)| results(held) != results(&determination));
		self.checks
			.insert(spec.name, (spec, determination.clone()));
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

fn results(d: &Determination) -> Vec<(&str, CheckResult)> {
	let mut r: Vec<(&str, CheckResult)> = d
		.instances
		.iter()
		.map(|i| (i.label.as_str(), i.observed))
		.collect();
	r.sort_unstable_by(|a, b| a.0.cmp(b.0));
	r
}

#[cfg(test)]
mod tests {
	use super::*;

	fn only(result: CheckResult, detail: serde_json::Value) -> Determination {
		Determination {
			instances: vec![SubstrateInstance::only(result, Some(detail))],
			message: "m".into(),
		}
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
		held.set(WORKLOADS_RUNNING, only(CheckResult::Passed, json!({"ready": 10})));
		assert!(
			held.set(WORKLOADS_RUNNING, only(CheckResult::Passed, json!({"ready": 11})))
				.is_none(),
			"detail moving without the result is not a change",
		);
		assert!(
			held.set(WORKLOADS_RUNNING, only(CheckResult::Failed, json!({"ready": 2})))
				.is_some()
		);

		// The refile carries the latest detail.
		let [Filing::Substrate(filing)] = held.all().try_into().unwrap() else {
			panic!("one substrate filing held");
		};
		assert_eq!(filing.instances[0].detail, Some(json!({"ready": 2})));
		assert_eq!(filing.check, "workloads-running");
		assert_eq!(filing.target, FilingTarget::Cluster);
	}

	#[test]
	fn an_instance_appearing_is_a_change() {
		let mut held = Held::default();
		let pool = |name: &str| SubstrateInstance {
			label: name.into(),
			observed: CheckResult::Passed,
			detail: None,
		};
		held.set(
			NODE_POOLS,
			Determination {
				instances: vec![pool("general")],
				message: String::new(),
			},
		);
		assert!(
			held.set(
				NODE_POOLS,
				Determination {
					instances: vec![pool("general"), pool("gpu")],
					message: String::new(),
				},
			)
			.is_some()
		);
	}

	#[test]
	fn a_refused_permission_is_broken_and_names_the_refusal() {
		let d = Determination::refused(&Refusal {
			verb: "list",
			resource: "nodepools.karpenter.sh".into(),
			message: "forbidden".into(),
		});
		assert_eq!(d.instances[0].observed, CheckResult::Broken);
		assert_eq!(
			d.instances[0].detail.as_ref().unwrap()["refused"]["resource"],
			"nodepools.karpenter.sh"
		);
	}
}
