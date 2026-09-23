//! The Tailscale API proxy: whether operators can get into the cluster's API
//! over the tailnet (spec `K8S`, "Checks about the whole cluster").
//!
//! The Tailscale operator runs the proxy one of two ways, and the check reads
//! whichever the cluster uses:
//!
//! - As a ProxyGroup of type `kube-apiserver`, which the operator reports on:
//!   whether a proxy is available to serve, and the tailnet devices it holds.
//!   Both halves of the check are read there.
//! - In-process, inside the operator itself. The operator reports nothing
//!   about its own tailnet connection there, so only its readiness can be
//!   read, and the detail says the tailnet half was not.

use commons_types::status::CheckResult;
use relay_protocol::SubstrateInstance;
use serde_json::{Value, json};

use super::{Determination, workloads::Replicas};

/// The ProxyGroup type that serves the Kubernetes API.
const KUBE_APISERVER: &str = "kube-apiserver";

/// The operator's environment variable naming its in-process proxy mode.
pub const APISERVER_PROXY_ENV: &str = "APISERVER_PROXY";

/// A `kube-apiserver` ProxyGroup, as much of it as the check reads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProxyGroup {
	pub name: String,
	/// Whether the operator reports a proxy ready to serve.
	pub available: bool,
	/// The tailnet devices the operator reports the group holding.
	pub devices: Vec<String>,
}

impl ProxyGroup {
	/// Read a ProxyGroup object, or `None` for one that is not an API proxy.
	pub fn read(name: &str, object: &Value) -> Option<Self> {
		if object.pointer("/spec/type").and_then(Value::as_str) != Some(KUBE_APISERVER) {
			return None;
		}
		let condition = |kind: &str| {
			object
				.pointer("/status/conditions")
				.and_then(Value::as_array)
				.into_iter()
				.flatten()
				.find(|c| c.get("type").and_then(Value::as_str) == Some(kind))
				.and_then(|c| c.get("status").and_then(Value::as_str))
				.map(|s| s == "True")
		};
		// Available is "at least one proxy can serve", which is what getting in
		// needs; an operator too old to report it has only Ready to go on.
		let available = condition("ProxyGroupAvailable")
			.or_else(|| condition("ProxyGroupReady"))
			.unwrap_or(false);
		let devices = object
			.pointer("/status/devices")
			.and_then(Value::as_array)
			.into_iter()
			.flatten()
			.filter(|d| {
				d.get("tailnetIPs")
					.and_then(Value::as_array)
					.is_some_and(|ips| !ips.is_empty())
			})
			.filter_map(|d| d.get("hostname").and_then(Value::as_str).map(str::to_owned))
			.collect();
		Some(Self {
			name: name.to_owned(),
			available,
			devices,
		})
	}
}

/// Whether an operator container runs the API proxy in-process, from the mode
/// its chart sets in its environment.
pub fn runs_proxy_in_process(mode: Option<&str>) -> bool {
	matches!(mode, Some("true" | "noauth"))
}

/// Grade the proxy from what the cluster has: its API-proxy ProxyGroups, else
/// the operator running it in-process. `None` where the cluster has neither,
/// the condition not existing there.
pub fn determine(
	groups: &[ProxyGroup],
	in_process: Option<(&str, Replicas)>,
) -> Option<Determination> {
	if !groups.is_empty() {
		return Some(proxy_groups(groups));
	}
	let (name, replicas) = in_process?;
	Some(operator(name, replicas))
}

/// Operators can get in when any one API proxy is serving and on the tailnet.
fn proxy_groups(groups: &[ProxyGroup]) -> Determination {
	let healthy = groups.iter().find(|g| g.available && !g.devices.is_empty());
	let shown = healthy.unwrap_or(&groups[0]);
	let (observed, message) = match healthy {
		Some(g) => (
			CheckResult::Passed,
			format!("the API proxy {} is serving on the tailnet", g.name),
		),
		None if !shown.available => (
			CheckResult::Failed,
			format!("the API proxy {} has no proxy ready to serve", shown.name),
		),
		None => (
			CheckResult::Failed,
			format!(
				"the API proxy {} is not connected to the tailnet",
				shown.name
			),
		),
	};
	Determination {
		instances: vec![SubstrateInstance::only(
			observed,
			Some(json!({
				"mode": "proxy-group",
				"proxy": shown.name,
				"ready": shown.available,
				"connected": !shown.devices.is_empty(),
				"devices": shown.devices,
			})),
		)],
		message,
	}
}

fn operator(name: &str, replicas: Replicas) -> Determination {
	let ready = replicas.desired > 0 && replicas.ready >= replicas.desired;
	let (observed, message) = if ready {
		(
			CheckResult::Passed,
			format!("the Tailscale operator {name}, which serves the API proxy, is ready"),
		)
	} else {
		(
			CheckResult::Failed,
			format!("the Tailscale operator {name}, which serves the API proxy, is not ready"),
		)
	};
	Determination {
		instances: vec![SubstrateInstance::only(
			observed,
			Some(json!({
				"mode": "in-process",
				"proxy": name,
				"ready": ready,
				// The operator reports nothing about its own tailnet
				// connection, so this half is not read.
				"connected": null,
			})),
		)],
		message,
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn group(available: &str, devices: Value) -> Value {
		json!({
			"spec": {"type": "kube-apiserver"},
			"status": {
				"conditions": [{"type": "ProxyGroupAvailable", "status": available}],
				"devices": devices,
			},
		})
	}

	fn connected() -> Value {
		json!([{"hostname": "api.tailnet.ts.net", "tailnetIPs": ["100.64.0.1"]}])
	}

	#[test]
	fn only_an_api_proxy_group_is_read() {
		let egress = json!({"spec": {"type": "egress"}});
		assert_eq!(ProxyGroup::read("egress", &egress), None);
		let read = ProxyGroup::read("api", &group("True", connected())).unwrap();
		assert!(read.available);
		assert_eq!(read.devices, ["api.tailnet.ts.net"]);
	}

	#[test]
	fn a_proxy_group_serving_on_the_tailnet_passes() {
		let g = ProxyGroup::read("api", &group("True", connected())).unwrap();
		let d = determine(&[g], None).unwrap();
		assert_eq!(d.instances[0].observed, CheckResult::Passed);
		assert_eq!(
			d.instances[0].detail.as_ref().unwrap()["mode"],
			"proxy-group"
		);
	}

	#[test]
	fn a_proxy_group_not_ready_fails_and_says_so() {
		let g = ProxyGroup::read("api", &group("False", connected())).unwrap();
		let d = determine(&[g], None).unwrap();
		assert_eq!(d.instances[0].observed, CheckResult::Failed);
		assert_eq!(d.instances[0].detail.as_ref().unwrap()["ready"], false);
		assert!(d.message.contains("no proxy ready"));
	}

	#[test]
	fn a_proxy_group_ready_but_off_the_tailnet_fails_and_says_so() {
		let off = json!([{"hostname": "api.tailnet.ts.net", "tailnetIPs": []}]);
		let g = ProxyGroup::read("api", &group("True", off)).unwrap();
		let d = determine(&[g], None).unwrap();
		assert_eq!(d.instances[0].observed, CheckResult::Failed);
		let detail = d.instances[0].detail.as_ref().unwrap();
		assert_eq!(detail["ready"], true);
		assert_eq!(detail["connected"], false);
		assert!(d.message.contains("not connected to the tailnet"));
	}

	#[test]
	fn a_proxy_group_is_read_in_preference_to_the_operator() {
		let g = ProxyGroup::read("api", &group("True", connected())).unwrap();
		let d = determine(&[g], Some(("operator", Replicas::new(1, 0)))).unwrap();
		assert_eq!(d.instances[0].observed, CheckResult::Passed);
	}

	#[test]
	fn an_in_process_proxy_is_graded_on_the_operators_readiness() {
		let d = determine(&[], Some(("operator", Replicas::new(1, 1)))).unwrap();
		assert_eq!(d.instances[0].observed, CheckResult::Passed);
		let detail = d.instances[0].detail.as_ref().unwrap();
		assert_eq!(detail["mode"], "in-process");
		assert_eq!(detail["connected"], Value::Null);

		let d = determine(&[], Some(("operator", Replicas::new(1, 0)))).unwrap();
		assert_eq!(d.instances[0].observed, CheckResult::Failed);
		let d = determine(&[], Some(("operator", Replicas::new(0, 0)))).unwrap();
		assert_eq!(
			d.instances[0].observed,
			CheckResult::Failed,
			"an operator scaled away serves no proxy",
		);
	}

	#[test]
	fn a_cluster_with_no_api_proxy_files_nothing() {
		assert_eq!(determine(&[], None), None);
	}

	#[test]
	fn the_in_process_modes_are_the_ones_that_serve() {
		assert!(runs_proxy_in_process(Some("true")));
		assert!(runs_proxy_in_process(Some("noauth")));
		assert!(!runs_proxy_in_process(Some("false")));
		assert!(!runs_proxy_in_process(None));
	}
}
