//! Node pools: one instance per Karpenter NodePool, graded on the pool's own
//! conditions (spec `K8S`, "Checks about the whole cluster").

use commons_types::status::CheckResult;
use relay_protocol::SubstrateInstance;
use serde_json::{Value, json};

/// A NodePool condition as Karpenter reports it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Condition {
	pub kind: String,
	pub status: String,
	pub reason: Option<String>,
	pub message: Option<String>,
}

impl Condition {
	fn is_false(&self) -> bool {
		self.status == "False"
	}
}

const READY: &str = "Ready";
const NODE_CLASS_READY: &str = "NodeClassReady";
const NODE_REGISTRATION_HEALTHY: &str = "NodeRegistrationHealthy";

/// The conditions on a NodePool object's status.
pub fn conditions(object: &Value) -> Vec<Condition> {
	object
		.pointer("/status/conditions")
		.and_then(Value::as_array)
		.into_iter()
		.flatten()
		.filter_map(|c| {
			Some(Condition {
				kind: c.get("type")?.as_str()?.to_owned(),
				status: c.get("status")?.as_str()?.to_owned(),
				reason: c.get("reason").and_then(Value::as_str).map(str::to_owned),
				message: c.get("message").and_then(Value::as_str).map(str::to_owned),
			})
		})
		.collect()
}

/// Grade one pool.
///
/// A pool fails when it is not ready, or when the nodes it launches do not
/// register. A pool that is not ready only because its node class is not is
/// passed here: that is the node class's condition, so one broken class never
/// reads as every pool referencing it failing. A condition Karpenter has not
/// settled yet (`Unknown`, or absent on a pool it has not reconciled) is not a
/// failure.
pub fn grade(pool: &str, conditions: &[Condition]) -> SubstrateInstance {
	let find = |kind: &str| conditions.iter().find(|c| c.kind == kind);

	let failing = find(NODE_REGISTRATION_HEALTHY)
		.filter(|c| c.is_false())
		.or_else(|| {
			let ready = find(READY).filter(|c| c.is_false())?;
			// What else is false says why the pool is not ready.
			let causes: Vec<&Condition> = conditions
				.iter()
				.filter(|c| c.kind != READY && c.is_false())
				.collect();
			let only_the_node_class =
				!causes.is_empty() && causes.iter().all(|c| c.kind == NODE_CLASS_READY);
			if only_the_node_class {
				None
			} else {
				Some(causes.first().copied().unwrap_or(ready))
			}
		});

	match failing {
		Some(condition) => SubstrateInstance {
			label: pool.to_owned(),
			observed: CheckResult::Failed,
			detail: Some(json!({
				"pool": pool,
				"condition": condition.kind,
				"reason": condition.reason,
				"message": condition.message,
			})),
		},
		None => SubstrateInstance {
			label: pool.to_owned(),
			observed: CheckResult::Passed,
			detail: Some(json!({ "pool": pool })),
		},
	}
}

/// What an operator reads for the check as a whole: the failing pools, each
/// with what Karpenter said about it.
pub fn message(instances: &[SubstrateInstance]) -> String {
	let failing: Vec<String> = instances
		.iter()
		.filter(|i| i.observed != CheckResult::Passed)
		.map(|i| {
			let said = i
				.detail
				.as_ref()
				.and_then(|d| d.get("message").and_then(Value::as_str))
				.or_else(|| {
					i.detail
						.as_ref()
						.and_then(|d| d.get("condition").and_then(Value::as_str))
				})
				.unwrap_or("not ready");
			format!("{}: {said}", i.label)
		})
		.collect();
	if failing.is_empty() {
		format!("{} node pools healthy", instances.len())
	} else {
		failing.join("; ")
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn cond(kind: &str, status: &str) -> Condition {
		Condition {
			kind: kind.into(),
			status: status.into(),
			reason: None,
			message: Some(format!("{kind} is {status}")),
		}
	}

	#[test]
	fn conditions_are_read_off_the_status() {
		let pool = json!({"status": {"conditions": [
			{"type": "Ready", "status": "True"},
			{"type": "NodeRegistrationHealthy", "status": "False", "reason": "Unhealthy", "message": "nodes did not register"},
		]}});
		let read = conditions(&pool);
		assert_eq!(read.len(), 2);
		assert_eq!(read[1].reason.as_deref(), Some("Unhealthy"));
	}

	#[test]
	fn a_ready_pool_passes() {
		let i = grade(
			"general",
			&[
				cond(READY, "True"),
				cond(NODE_CLASS_READY, "True"),
				cond(NODE_REGISTRATION_HEALTHY, "True"),
			],
		);
		assert_eq!(i.observed, CheckResult::Passed);
		assert_eq!(i.label, "general");
	}

	#[test]
	fn a_pool_that_is_not_ready_fails() {
		let i = grade(
			"general",
			&[cond(READY, "False"), cond("ValidationSucceeded", "False")],
		);
		assert_eq!(i.observed, CheckResult::Failed);
		assert_eq!(i.detail.unwrap()["condition"], "ValidationSucceeded");
	}

	#[test]
	fn a_pool_whose_launched_nodes_do_not_register_fails() {
		let i = grade(
			"general",
			&[cond(READY, "True"), cond(NODE_REGISTRATION_HEALTHY, "False")],
		);
		assert_eq!(i.observed, CheckResult::Failed);
		assert_eq!(i.detail.unwrap()["condition"], NODE_REGISTRATION_HEALTHY);
	}

	#[test]
	fn a_pool_not_ready_only_because_of_its_node_class_is_not_failed_for_it() {
		let i = grade(
			"general",
			&[cond(READY, "False"), cond(NODE_CLASS_READY, "False")],
		);
		assert_eq!(i.observed, CheckResult::Passed);
	}

	#[test]
	fn a_broken_node_class_does_not_hide_the_pools_own_failure() {
		let i = grade(
			"general",
			&[
				cond(READY, "False"),
				cond(NODE_CLASS_READY, "False"),
				cond(NODE_REGISTRATION_HEALTHY, "False"),
			],
		);
		assert_eq!(i.observed, CheckResult::Failed);
	}

	#[test]
	fn an_unsettled_pool_is_not_a_failure() {
		assert_eq!(grade("new", &[]).observed, CheckResult::Passed);
		assert_eq!(
			grade("new", &[cond(READY, "Unknown")]).observed,
			CheckResult::Passed
		);
	}

	#[test]
	fn the_message_names_each_failing_pool() {
		let instances = [
			grade("general", &[cond(READY, "True")]),
			grade("gpu", &[cond(NODE_REGISTRATION_HEALTHY, "False")]),
		];
		assert_eq!(message(&instances), "gpu: NodeRegistrationHealthy is False");
	}
}
