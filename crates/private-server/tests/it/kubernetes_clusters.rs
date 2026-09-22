//! Cluster registry and connection (spec `K8S`, "Cluster registry"):
//! registering a cluster mints its relay's credential and leaves a draft, and
//! the draft becomes a registered cluster only once its relay is answering.

use commons_tests::server::run;
use jiff::Timestamp;
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn registering_mints_a_relay_and_leaves_a_draft() {
	run(async |mut conn, _public, private| {
		let started: Value = private
			.post("/api/kubernetes_clusters/register")
			.json(&json!({ "name": "Nauru cluster" }))
			.await
			.json();

		// The draft carries the name and the relay's identity, and is not yet
		// registered: nothing unconfirmed enters the registry.
		assert_eq!(started["cluster"]["name"], "Nauru cluster");
		assert_eq!(started["cluster"]["registered"], false);
		let cluster_id = started["cluster"]["id"].as_str().unwrap().to_owned();
		let relay_identity_id = started["cluster"]["relay_identity_id"]
			.as_str()
			.unwrap()
			.to_owned();

		// The relay's credential is returned once, and the identity carries the
		// relay role.
		assert_eq!(started["credential"]["device_id"], relay_identity_id);
		assert!(
			started["credential"]["passphrase"].as_str().unwrap().len() > 0,
			"the private key's passphrase is returned once",
		);
		let device: Value = private
			.post("/api/devices/get_device_by_id")
			.json(&json!({ "device_id": relay_identity_id }))
			.await
			.json();
		assert_eq!(device["device"]["role"], "relay");

		// The registry lists it as a draft, not a registered cluster.
		let listed: Value = private
			.post("/api/kubernetes_clusters/list")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(listed["registered"].as_array().unwrap().len(), 0);
		assert_eq!(listed["drafts"].as_array().unwrap().len(), 1);
		assert_eq!(listed["drafts"][0]["id"].as_str().unwrap(), cluster_id);

		// Confirming before the relay has answered leaves the draft a draft.
		let still_draft: Value = private
			.post("/api/kubernetes_clusters/confirm")
			.json(&json!({ "id": cluster_id }))
			.await
			.json();
		assert_eq!(still_draft["registered"], false);
		assert_eq!(still_draft["answering"], false);

		// The relay dials in and answers: the hub stamps `last_answered_at`.
		let relay_uuid: uuid::Uuid = relay_identity_id.parse().unwrap();
		let stamped =
			database::KubernetesCluster::stamp_answered(&mut conn, relay_uuid, Timestamp::now())
				.await
				.expect("stamp answered");
		assert!(stamped, "the stamp lands on the draft's cluster row");

		// Now confirmation registers the cluster.
		let confirmed: Value = private
			.post("/api/kubernetes_clusters/confirm")
			.json(&json!({ "id": cluster_id }))
			.await
			.json();
		assert_eq!(confirmed["answering"], true);
		assert_eq!(confirmed["registered"], true);

		// It now lists as registered, and no draft remains.
		let listed: Value = private
			.post("/api/kubernetes_clusters/list")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(listed["registered"].as_array().unwrap().len(), 1);
		assert_eq!(listed["drafts"].as_array().unwrap().len(), 0);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reissuing_retires_the_superseded_key_and_keeps_the_identity() {
	run(async |_conn, _public, private| {
		let started: Value = private
			.post("/api/kubernetes_clusters/register")
			.json(&json!({ "name": "Kiribati cluster" }))
			.await
			.json();
		let cluster_id = started["cluster"]["id"].as_str().unwrap().to_owned();
		let relay_identity_id = started["cluster"]["relay_identity_id"]
			.as_str()
			.unwrap()
			.to_owned();

		// Re-issue the credential (a key lost before it reached the cluster).
		let reissued: Value = private
			.post("/api/kubernetes_clusters/reissue")
			.json(&json!({ "id": cluster_id }))
			.await
			.json();

		// The relay identity is unchanged — an application's cluster reference
		// never moves — and the superseded key is retired, leaving one active.
		assert_eq!(reissued["device_id"], relay_identity_id);
		let device: Value = private
			.post("/api/devices/get_device_by_id")
			.json(&json!({ "device_id": relay_identity_id }))
			.await
			.json();
		let active = device["keys"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|k| k["is_active"].as_bool().unwrap_or(false))
			.count();
		assert_eq!(
			active, 1,
			"only the re-issued key is active; the prior is retired"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_is_removed_from_the_registry() {
	run(async |_conn, _public, private| {
		let started: Value = private
			.post("/api/kubernetes_clusters/register")
			.json(&json!({ "name": "Tuvalu cluster" }))
			.await
			.json();
		let cluster_id = started["cluster"]["id"].as_str().unwrap().to_owned();

		private
			.post("/api/kubernetes_clusters/remove")
			.json(&json!({ "id": cluster_id }))
			.await
			.assert_status_ok();

		let listed: Value = private
			.post("/api/kubernetes_clusters/list")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(listed["drafts"].as_array().unwrap().len(), 0);
		assert_eq!(listed["registered"].as_array().unwrap().len(), 0);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_nameless_registration_is_refused() {
	run(async |_conn, _public, private| {
		private
			.post("/api/kubernetes_clusters/register")
			.json(&json!({ "name": "   " }))
			.await
			.assert_status_bad_request();
	})
	.await;
}
