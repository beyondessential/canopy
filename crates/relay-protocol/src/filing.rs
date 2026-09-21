//! What the relay files upward: the two check families, each in the shape its
//! family already has.
//!
//! The relay determines both families itself and files the results; no message
//! here returns a Kubernetes object. It files when a check's result changes and
//! refiles what it holds periodically, so a filing is always the current state
//! of that check rather than an event to be accumulated.
//!
//! **Filings are unacknowledged.** The relay gets QUIC's delivery guarantee
//! but no application-level confirmation that a filing was ingested, so a
//! filing canopy accepts on the wire and then fails to ingest is lost until
//! the next refile. The periodic refile *is* the reconciliation mechanism —
//! it exists already to survive a missed observation, a restart, or a
//! reconnection — so per-filing acks would be machinery covering a window the
//! refile already closes.

use commons_types::status::CheckResult;
use serde::{Deserialize, Serialize};

/// One filing, on its own unidirectional stream.
///
/// The two families do not share a shape and are not forced into one: a
/// harvest filing is the status-push body a device would have sent, and a
/// substrate filing is a check canopy's own filing path already understands.
/// A common envelope would be a lowest-common-denominator type fitting
/// neither.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "family", rename_all = "kebab-case")]
pub enum Filing {
	/// A server's own checks, harvested from its database and workloads.
	Harvest(HarvestFiling),
	/// A check about the substrate: what the cluster does with the workloads.
	Substrate(SubstrateFiling),
}

/// Where a filing lands, in the coordinates the relay actually holds.
///
/// The relay names a namespace and an instance within it, and **canopy**
/// resolves that to the server or group the filing is about, from the
/// Kubernetes coordinates an operator set on the server record (spec `K8S`,
/// "Setting a server's identity"). So the relay never holds canopy's
/// identifiers and there is nothing to keep in step: identity stays where the
/// operator set it.
///
/// This is a cluster coordinate, not a check-state scope. Canopy maps it onto
/// the one `database::issues::Scope` vocabulary on arrival — instance to the
/// server, namespace to the server group, cluster to canopy-wide with the
/// cluster as the instance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "target", rename_all = "kebab-case")]
pub enum FilingTarget {
	/// One instance in a namespace: a check about a single server.
	Instance {
		namespace: String,
		instance: Instance,
	},
	/// A namespace, which is a server group at a rank.
	Namespace { namespace: String },
	/// The cluster the relay serves. Filed canopy-wide with the cluster as an
	/// instance of the check, the relay's own identity naming which cluster.
	Cluster,
}

/// Which instance within a namespace, in the terms the namespace itself uses.
///
/// A namespace holds one deployment: one central server and its facilities,
/// each with its own Postgres and its own workloads per duty (spec `K8S`,
/// "Deployment shape Canopy relies on").
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum Instance {
	/// The namespace's central server.
	Central,
	/// One facility, by the identity that locates its databases and workloads
	/// within the namespace.
	Facility { id: String },
}

/// A server's own checks, as the status-push body a device would have pushed.
///
/// The filing **is** the push body rather than a re-modelled filing type: the
/// relay runs the same check suite a bestool runs and produces the same
/// payload, and canopy feeds it to the same ingestion the HTTP push path
/// feeds. Parity is structural that way rather than something maintained on
/// two sides — and re-modelling it here is exactly the drift the relay design
/// exists to avoid.
///
/// So `push` is deliberately untyped at this layer: the ingestion core owns
/// the payload contract, validates it, and is the single place it is parsed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HarvestFiling {
	/// The instance this describes. A harvest filing is always about one
	/// server, so the target is always an instance in a namespace.
	pub namespace: String,
	pub instance: Instance,
	/// The status-push body, verbatim.
	pub push: serde_json::Value,
}

/// A check about the substrate, in the shape canopy's own filing path takes.
///
/// Filed under the `kubernetes` source (see [`crate::SUBSTRATE_SOURCE`]),
/// which no ordinary device may report. The policy fields seed the catalog
/// entry on first sight so a substrate check registers already reviewed, at
/// the grading its condition warrants; operator edits stick from then on.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubstrateFiling {
	pub target: FilingTarget,
	/// The check's stable name — a category an operator configures once, never
	/// a name with a parameter spelled into it. Whatever varies per instance
	/// goes in `detail`, where a policy rule reaches it.
	pub check: String,
	/// What the relay observed. Canopy grades it through the operator's
	/// policy from there; the relay does not decide severity.
	pub observed: CheckResult,
	/// Single-line headline for a degraded filing.
	pub title: Option<String>,
	/// What an operator reads.
	pub message: String,
	/// The check's own fields, available to policy rules as `check.*`.
	pub detail: Option<serde_json::Value>,
	/// The policy this check registers with on first sight.
	pub default_ceiling: CheckResult,
	pub default_escalates: bool,
	/// The documentation the check ships with, seeded into the catalog on
	/// first sight and never overwriting an operator's edit.
	pub documentation: Option<String>,
}

#[cfg(test)]
mod tests {
	use super::*;
	use crate::frame::{read_required_frame, write_frame};

	async fn round_trip(filing: &Filing) -> Filing {
		let mut buf = Vec::new();
		write_frame(&mut buf, filing).await.unwrap();
		read_required_frame(&mut buf.as_slice()).await.unwrap()
	}

	#[tokio::test]
	async fn a_harvest_filing_carries_the_push_body_unchanged() {
		let push = serde_json::json!({
			"source": "alertd",
			"health": [{"check": "sync", "result": "passed"}],
			"tamanuVersion": "2.30.1",
		});
		let filing = Filing::Harvest(HarvestFiling {
			namespace: "nauru-demo".into(),
			instance: Instance::Facility {
				id: "ward-a".into(),
			},
			push: push.clone(),
		});

		let Filing::Harvest(back) = round_trip(&filing).await else {
			panic!("family changed across the wire");
		};
		assert_eq!(
			back.push, push,
			"the push body must cross verbatim — re-modelling it is the drift this avoids",
		);
		assert_eq!(back.namespace, "nauru-demo");
		assert_eq!(
			back.instance,
			Instance::Facility {
				id: "ward-a".into()
			}
		);
	}

	#[tokio::test]
	async fn every_substrate_target_round_trips() {
		for target in [
			FilingTarget::Instance {
				namespace: "nauru-demo".into(),
				instance: Instance::Central,
			},
			FilingTarget::Namespace {
				namespace: "nauru-demo".into(),
			},
			FilingTarget::Cluster,
		] {
			let filing = Filing::Substrate(SubstrateFiling {
				target: target.clone(),
				check: "pod-unschedulable".into(),
				observed: CheckResult::Failed,
				title: Some("A pod cannot be placed".into()),
				message: "no node has capacity".into(),
				detail: Some(serde_json::json!({"pod": "central-api-0"})),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			});

			let Filing::Substrate(back) = round_trip(&filing).await else {
				panic!("family changed across the wire");
			};
			assert_eq!(back.target, target);
			assert_eq!(back.observed, CheckResult::Failed);
		}
	}
}
