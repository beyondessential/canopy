//! Periodic checks driven by the cached `TailnetDirectory`. Each sweep
//! reconciles the directory's view of a node against canopy's
//! persisted device rows and files / closes the relevant issues.

use commons_errors::Result;
use commons_types::{Uuid, status::CheckResult};
use database::{
	devices::Device,
	issues::{CheckFiling, Issue, Scope, file_check},
};
use diesel_async::AsyncPgConnection;

use crate::tailnet_directory::TailnetDirectory;

/// Source value canopy uses when it files Tailscale-driven issues.
pub const TAILSCALE_SOURCE: &str = "canopy";

/// Ref value for the one "tailnet key will expire" issue per
/// `(machine, device)` pair.
pub const KEY_EXPIRY_REF: &str = "tailscale-key-expiry";

pub const KEY_EXPIRY_DOC: &str = "## Description

The machine's Tailscale node key is nearing expiry (and key expiry isn't disabled for the node). When it lapses, the device drops off the tailnet and canopy loses its management path.

## Results

- **fail** — the node key expires within the alert lead. Escalates: an expired key severs connectivity. Recovers when the key is rotated or expiry is disabled.

## Solve

Re-authenticate the node (`tailscale up`) or disable key expiry for it in the Tailscale admin console.";

/// Sweep every tailnet-bound device and file (or close) the key-expiry
/// check per `(machine, device)` pair based on the node's
/// `keyExpiryDisabled`. The check registers as an escalating failure —
/// losing contact with a node is stop-the-world — but operators can
/// regrade it from the catalog.
///
/// The machine is the subject. A node key expiring severs canopy's path to
/// the box, so it is one finding about the box rather than one per workload
/// running on it.
///
/// Tailnet devices bound to no machine are intentionally skipped: the
/// tailnet hosts plenty of nodes that aren't canopy-managed boxes (operator
/// laptops, other infra, …) and we have nothing to say about those — the
/// sweep is scoped to the headless devices canopy actually runs.
///
/// Returns the number of events filed in this pass.
// spec: DTR, CHK
pub async fn sweep_key_expiry(
	db: &mut AsyncPgConnection,
	directory: &TailnetDirectory,
) -> Result<usize> {
	let pairs = Device::list_tailnet_bound_machines(db).await?;
	if pairs.is_empty() {
		return Ok(0);
	}

	let snapshot = directory.snapshot_by_node_id().await;
	let machine_ids: Vec<Uuid> = pairs.iter().map(|(_, m, _)| *m).collect();
	let existing =
		Issue::list_by_source_ref_for_machines(db, TAILSCALE_SOURCE, KEY_EXPIRY_REF, &machine_ids)
			.await?;
	// The read is filtered by `machine_ids`, so every row is machine-scoped
	// (`machine_id` is `Some`); drop any defensively.
	let issue_map: std::collections::HashMap<Uuid, &Issue> = existing
		.iter()
		.filter_map(|i| i.machine_id.map(|mid| (mid, i)))
		.collect();

	let mut filed = 0usize;
	for (device, machine_id, node_id) in &pairs {
		let entry = snapshot.get(node_id);
		let existing_issue = issue_map.get(machine_id).copied();

		// Node not in the directory (left the tailnet / not yet
		// refreshed) — leave any existing issue alone and don't file a
		// new one. Detach is the operator action that would actually
		// clear this state.
		let Some(entry) = entry else {
			continue;
		};

		let (observed, title, message) = match (entry.key_expiry_disabled, existing_issue) {
			// Healthy state, no issue: nothing to do.
			(true, None) => continue,
			// Healthy state, issue already inactive: nothing to do.
			(true, Some(issue)) if !issue.active => continue,
			// Healthy state, but an active issue exists: close it.
			(true, Some(_)) => (
				CheckResult::Passed,
				None,
				format!("Tailscale key expiry disabled for {}", entry.node_name),
			),
			// Unhealthy state: file or refresh the failure.
			(false, _) => (
				CheckResult::Failed,
				Some(format!("Tailscale key will expire for {}", entry.node_name)),
				format!(
					"Tailnet node {} has key expiry enabled. When the \
					 node's key expires, it will drop off the tailnet and \
					 canopy will lose contact.",
					entry.node_name,
				),
			),
		};

		file_check(
			db,
			CheckFiling {
				source: TAILSCALE_SOURCE,
				scope: Scope::Machine(*machine_id),
				device_id: Some(device.id),
				check: KEY_EXPIRY_REF,
				observed,
				title: title.as_deref(),
				message: &message,
				detail: Some(database::check_detail! {
					"node_id": entry.node_id,
					"node_name": entry.node_name,
				}),
				default_ceiling: CheckResult::Failed,
				default_escalates: true,
				documentation: Some(KEY_EXPIRY_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	Ok(filed)
}
