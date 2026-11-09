use std::time::Duration;

use kube::Api;
use pgro::types::PostgresPhysicalReplica;

use crate::helpers::*;

/// An `ephemeral` replica should restore, come up healthy, then tear the
/// restore down — leaving no running database — and record the verified
/// snapshot so it doesn't immediately re-restore.
#[tokio::test]
#[ignore = "requires a running Kubernetes cluster with MinIO and kopia"]
async fn ephemeral_replica_tears_down_after_verify() {
	let client = make_client().await;
	let ns = "test-ephemeral";

	setup_namespace(&client, ns).await;
	cleanup_namespace(&client, ns, &["ephemeral-replica"]).await;

	let _secrets: Api<k8s_openapi::api::core::v1::Secret> = Api::namespaced(client.clone(), ns);
	let replicas: Api<PostgresPhysicalReplica> = Api::namespaced(client.clone(), ns);

	println!("--- creating ephemeral PostgresPhysicalReplica");
	// Long schedule so the only restore we observe is the initial one; after
	// teardown it must NOT restore again within the test window.
	let replica = build_replica(
		"ephemeral-replica",
		ReplicaOpts {
			schedule: "0 0 1 1 *".into(), // once a year
			ephemeral: true,
			..Default::default()
		},
	);
	let _replica = start_replica(&client, ns, replica, "test-bucket").await;

	// Don't wait for the `Ready` phase: on an ephemeral replica it's
	// transient — the switchover sets Ready and the very next reconcile
	// (~tens of ms later) tears the restore down and the replica drops back
	// to Pending. Polling every couple of seconds reliably misses that
	// window. Wait directly for the durable end state instead: the restore
	// gone, `verifiedSnapshotId` recorded, `currentRestore` cleared.
	//
	// The window before that end state covers the whole restore cycle
	// (snapshot-list → restore Job → deployment ready → switchover →
	// teardown), so allow the full phase timeout.
	println!("--- waiting for the restore to be verified and torn down");
	let deadline = tokio::time::Instant::now() + PHASE_TIMEOUT;
	loop {
		let count = count_restores_for_replica(&client, ns, "ephemeral-replica").await;
		let replica = replicas
			.get("ephemeral-replica")
			.await
			.expect("get replica");
		let verified = replica
			.status
			.as_ref()
			.and_then(|s| s.verified_snapshot_id.clone());
		if count == 0 && verified.is_some() {
			println!("--- torn down; verifiedSnapshotId={verified:?}");
			// currentRestore must be cleared so the reconciler doesn't treat
			// the vanished restore as an accidental deletion.
			assert!(
				replica
					.status
					.as_ref()
					.and_then(|s| s.current_restore.as_ref())
					.is_none(),
				"currentRestore must be cleared after ephemeral teardown"
			);
			break;
		}
		if tokio::time::Instant::now() >= deadline {
			panic!(
				"restore was not torn down within timeout (count={count}, verified={verified:?})"
			);
		}
		tokio::time::sleep(POLL_INTERVAL).await;
	}

	println!("--- confirming it stays torn down (no re-restore loop)");
	tokio::time::sleep(Duration::from_secs(30)).await;
	let count = count_restores_for_replica(&client, ns, "ephemeral-replica").await;
	assert_eq!(
		count, 0,
		"ephemeral replica must not re-restore the same snapshot after teardown"
	);

	println!("--- all assertions passed, cleaning up");
	cleanup_namespace(&client, ns, &["ephemeral-replica"]).await;
}
