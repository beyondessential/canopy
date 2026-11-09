//! Replicas that Canopy declares, end to end: the stub offers a worklist entry,
//! and pgro provisions what the entry names, restores it, and reports the result.

use std::time::Duration;

use bes_canopy_api::schema::WorklistEntry;
use k8s_openapi::api::core::v1::Namespace;
use kube::{Api, Client, ResourceExt, api::ListParams};
use pgro::{
	controllers::canopy::labels,
	types::{PostgresPhysicalReplica, ReplicaPhase},
};
use serde_json::{Map, Value};
use tokio::time::{Instant, sleep, timeout};
use uuid::Uuid;

use crate::{
	helpers::{LONG_PHASE_TIMEOUT, POLL_INTERVAL, make_client, snapshot_id_for},
	stub_canopy::{Recorded, stub},
};

const BUCKET: &str = "test-bucket";
const NAMESPACE_TIMEOUT: Duration = Duration::from_secs(120);

/// A `verify` entry for the fixture repository, which restores the snapshot the
/// setup Job made, proves it healthy and discards it.
async fn verify_entry(name: &str, group: Uuid, replica: Uuid) -> WorklistEntry {
	WorklistEntry::builder()
		.name(name.to_owned())
		.intent("verify".to_owned())
		.replica_id(replica)
		.group_id(group)
		.machine_id(Uuid::new_v4())
		.server_id(Uuid::new_v4())
		.type_("tamanu-postgres".to_owned())
		.storage("s3".to_owned())
		.bucket(BUCKET.to_owned())
		.region("us-east-1".to_owned())
		.prefix(String::new())
		.snapshot_id(snapshot_id_for(BUCKET).await)
		.params(Map::new())
		.build()
}

/// The namespace pgro made for the declaration `replica`, once it exists.
async fn wait_for_namespace(client: &Client, replica: Uuid) -> Namespace {
	let namespaces: Api<Namespace> = Api::all(client.clone());
	let selector = ListParams::default().labels(&format!("{}={replica}", labels::DECLARATION_ID));
	let deadline = Instant::now() + NAMESPACE_TIMEOUT;
	loop {
		if let Ok(list) = namespaces.list(&selector).await
			&& let Some(namespace) = list.items.into_iter().next()
		{
			return namespace;
		}
		assert!(
			Instant::now() < deadline,
			"no namespace appeared for declaration {replica} within {NAMESPACE_TIMEOUT:?}"
		);
		sleep(POLL_INTERVAL).await;
	}
}

/// The reports pgro has made for the declaration `replica`, once there is one.
async fn wait_for_report(replica: Uuid) -> Recorded {
	timeout(LONG_PHASE_TIMEOUT, async {
		loop {
			let report = stub()
				.requests_to("/restore-verification")
				.into_iter()
				.find(|report| report.body["replica_id"] == Value::String(replica.to_string()));
			if let Some(report) = report {
				return report;
			}
			sleep(POLL_INTERVAL).await;
		}
	})
	.await
	.unwrap_or_else(|_| panic!("no report for declaration {replica} within {LONG_PHASE_TIMEOUT:?}"))
}

/// The happy path: the entry becomes a namespace carrying the entry's identity,
/// a replica restored for it, and a report of the result back to Canopy.
#[tokio::test]
#[ignore = "requires a running Kubernetes cluster with MinIO and kopia"]
async fn worklist_entry_is_restored_and_reported() {
	let client = make_client().await;
	let (group, replica) = (Uuid::new_v4(), Uuid::new_v4());
	let entry = verify_entry("int-test", group, replica).await;
	let snapshot = entry.snapshot_id.clone().unwrap();
	stub().put_entry("int-test", &entry);

	let namespace = wait_for_namespace(&client, replica).await;
	assert!(
		namespace.name_any().starts_with("int-test-"),
		"unexpected namespace name: {}",
		namespace.name_any()
	);
	let namespace_labels = namespace.labels();
	for (key, expected) in [
		(labels::MANAGED_BY, labels::MANAGED_BY_VALUE.to_owned()),
		(labels::GROUP, group.to_string()),
		(labels::TYPE, "tamanu-postgres".to_owned()),
		(labels::INTENT, "verify".to_owned()),
	] {
		assert_eq!(
			namespace_labels.get(key),
			Some(&expected),
			"label {key} on the namespace"
		);
	}

	let report = wait_for_report(replica).await;
	assert_eq!(report.body["outcome"], "success", "report: {}", report.body);
	assert_eq!(report.body["snapshot_id"], snapshot.as_str());

	let credentials = stub().requests_to("/restore-credentials");
	assert!(
		credentials
			.iter()
			.any(|request| request.body["group"] == group.to_string().as_str()),
		"credentials were never requested for the group: {credentials:?}"
	);

	stub().remove_entry("int-test");
}

/// Where Canopy refuses credentials for the group, nothing is restored and pgro
/// carries on rather than retrying into a crash loop.
#[tokio::test]
#[ignore = "requires a running Kubernetes cluster with MinIO and kopia"]
async fn refused_credentials_start_no_restore() {
	let client = make_client().await;
	let (group, replica) = (Uuid::new_v4(), Uuid::new_v4());
	stub().deny_credentials(group);
	stub().put_entry(
		"int-denied",
		&verify_entry("int-denied", group, replica).await,
	);

	let namespace = wait_for_namespace(&client, replica).await;
	sleep(Duration::from_secs(30)).await;

	let replicas: Api<PostgresPhysicalReplica> =
		Api::namespaced(client.clone(), &namespace.name_any());
	let made = replicas.list(&ListParams::default()).await.unwrap().items;
	assert!(
		made.iter().all(|replica| {
			replica
				.status
				.as_ref()
				.and_then(|status| status.phase.as_ref())
				!= Some(&ReplicaPhase::Ready)
		}),
		"a replica was restored without credentials: {made:?}"
	);
	assert!(
		stub()
			.requests_to("/restore-verification")
			.iter()
			.all(|report| report.body["replica_id"] != replica.to_string().as_str()),
		"a report was made for a declaration that could not be restored"
	);

	stub().remove_entry("int-denied");
}
