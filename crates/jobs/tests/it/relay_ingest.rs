//! Where a relay's substrate filing lands, what is refused before it does, and
//! how its instances and brokenness are held once it has.

use commons_types::{device::DeviceRole, status::CheckResult};
use database::{
	KubernetesCluster, devices::Device, diesel_async::AsyncPgConnection, issues::Issue,
	silenced_refs::ClusterSilencedRef,
};
use jobs::relay::ingest::{self, Placement};
use relay_protocol::{
	Filing, FilingTarget, SUBSTRATE_SOURCE, SubstrateFiling, SubstrateInstance, SubstrateOutcome,
};

fn filing(check: &str, outcome: SubstrateOutcome, message: Option<&str>) -> Filing {
	Filing::Substrate(SubstrateFiling {
		target: FilingTarget::Cluster,
		check: check.into(),
		outcome,
		title: None,
		message: message.map(str::to_owned),
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	})
}

fn pool(key: &str, label: Option<&str>, observed: CheckResult) -> SubstrateInstance {
	SubstrateInstance {
		key: key.into(),
		label: label.map(str::to_owned),
		observed,
		detail: Some(serde_json::json!({ "pool": key })),
	}
}

/// A registered cluster, with the relay identity that files for it.
async fn registered(conn: &mut AsyncPgConnection) -> (uuid::Uuid, uuid::Uuid) {
	let relay = Device::create_at_role(
		conn,
		uuid::Uuid::new_v4().as_bytes().to_vec(),
		DeviceRole::Relay,
		None,
	)
	.await
	.unwrap();
	let draft = KubernetesCluster::create_draft(conn, "ops-main", relay.id)
		.await
		.unwrap();
	KubernetesCluster::register(conn, draft.id).await.unwrap();
	(relay.id, draft.id)
}

async fn state(conn: &mut AsyncPgConnection, cluster: uuid::Uuid, check: &str) -> Issue {
	let mut issues =
		Issue::list_by_source_ref_for_clusters(conn, SUBSTRATE_SOURCE, check, &[cluster])
			.await
			.unwrap();
	assert_eq!(issues.len(), 1, "one state for {check}");
	issues.remove(0)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_drafts_relay_places_nothing_and_a_registered_ones_places_the_cluster() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let relay = Device::create_at_role(
			&mut conn,
			uuid::Uuid::new_v4().as_bytes().to_vec(),
			DeviceRole::Relay,
			None,
		)
		.await
		.unwrap();
		let draft = KubernetesCluster::create_draft(&mut conn, "ops-main", relay.id)
			.await
			.unwrap();

		assert!(
			ingest::resolve(&mut conn, relay.id, &FilingTarget::Cluster)
				.await
				.unwrap()
				.is_none(),
			"a draft carries no checks",
		);

		KubernetesCluster::register(&mut conn, draft.id)
			.await
			.unwrap();
		let placement = ingest::resolve(&mut conn, relay.id, &FilingTarget::Cluster)
			.await
			.unwrap();
		assert!(matches!(placement, Some(Placement::Cluster(id)) if id == draft.id));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filing_naming_no_instances_is_refused() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (relay, cluster) = registered(&mut conn).await;

		assert!(
			ingest::ingest(
				&mut conn,
				relay,
				filing("node-pools", SubstrateOutcome::Instances(vec![]), None),
				Placement::Cluster(cluster)
			)
			.await
			.is_err()
		);
		ingest::ingest(
			&mut conn,
			relay,
			filing(
				"node-pools",
				SubstrateOutcome::Instances(vec![SubstrateInstance::only(
					CheckResult::Passed,
					None,
				)]),
				Some("m"),
			),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();
		state(&mut conn, cluster, "node-pools").await;
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn instances_canopy_cannot_hold_are_refused() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (relay, cluster) = registered(&mut conn).await;

		for (why, instances) in [
			(
				"an instance is never broken",
				vec![pool("general", None, CheckResult::Broken)],
			),
			(
				"a key names one instance",
				vec![
					pool("general", None, CheckResult::Passed),
					pool("general", None, CheckResult::Failed),
				],
			),
			(
				"only a check that holds once has an empty key",
				vec![
					pool("", None, CheckResult::Passed),
					pool("gpu", None, CheckResult::Failed),
				],
			),
		] {
			assert!(
				ingest::ingest(
					&mut conn,
					relay,
					filing("node-pools", SubstrateOutcome::Instances(instances), None),
					Placement::Cluster(cluster),
				)
				.await
				.is_err(),
				"{why}",
			);
		}
		assert!(
			Issue::list_by_source_ref_for_clusters(
				&mut conn,
				SUBSTRATE_SOURCE,
				"node-pools",
				&[cluster],
			)
			.await
			.unwrap()
			.is_empty(),
			"nothing refused is filed",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn substrate_instances_are_held_by_key_with_their_labels() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (relay, cluster) = registered(&mut conn).await;
		ingest::ingest(
			&mut conn,
			relay,
			filing(
				"node-pools",
				SubstrateOutcome::Instances(vec![
					pool("general", None, CheckResult::Passed),
					pool("gpu", Some("GPU pool"), CheckResult::Failed),
				]),
				None,
			),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();

		let issue = state(&mut conn, cluster, "node-pools").await;
		let instances = issue.stored_instances().expect("held by key");
		assert_eq!(
			instances.0.keys().map(String::as_str).collect::<Vec<_>>(),
			["general", "gpu"]
		);
		assert_eq!(instances.0["gpu"].label.as_deref(), Some("GPU pool"));
		assert_eq!(instances.0["general"].label, None);
		assert_eq!(instances.0["gpu"].effective, CheckResult::Failed);
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
	})
	.await
}

/// Canopy writes an instanced check's message from the instances as it graded
/// them, so the relay's own account of them, which would count an instance a
/// silence has since taken out, is not what an operator reads.
#[tokio::test(flavor = "multi_thread")]
async fn an_instanced_substrate_checks_message_is_canopys() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (relay, cluster) = registered(&mut conn).await;
		let pools = || {
			SubstrateOutcome::Instances(vec![
				pool("general", None, CheckResult::Failed),
				pool("gpu", Some("GPU pool"), CheckResult::Failed),
			])
		};
		let relays_account = "general: not ready; gpu: not ready";
		ingest::ingest(
			&mut conn,
			relay,
			filing("node-pools", pools(), Some(relays_account)),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();
		let issue = state(&mut conn, cluster, "node-pools").await;
		assert_ne!(issue.message, relays_account);
		assert!(
			issue.message.contains("general") && issue.message.contains("GPU pool"),
			"names each degraded instance, by label where it has one: {}",
			issue.message,
		);

		// Silencing one pool takes it out of the message on the next filing.
		ClusterSilencedRef::add(
			&mut conn,
			cluster,
			SUBSTRATE_SOURCE,
			"node-pools",
			Some("gpu"),
			Some("op@example.com"),
		)
		.await
		.unwrap();
		ingest::ingest(
			&mut conn,
			relay,
			filing("node-pools", pools(), Some(relays_account)),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();
		let issue = state(&mut conn, cluster, "node-pools").await;
		assert!(
			issue.message.contains("general") && !issue.message.contains("GPU pool"),
			"a silenced instance is not counted: {}",
			issue.message,
		);

		// A check that holds once is described by the relay, as canopy's own
		// plain determinations are by whatever files them.
		ingest::ingest(
			&mut conn,
			relay,
			filing(
				"tailscale-api-proxy",
				SubstrateOutcome::Instances(vec![SubstrateInstance::only(
					CheckResult::Failed,
					None,
				)]),
				Some("the API proxy is not connected to the tailnet"),
			),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();
		assert_eq!(
			state(&mut conn, cluster, "tailscale-api-proxy")
				.await
				.message,
			"the API proxy is not connected to the tailnet"
		);
	})
	.await
}

/// A check the relay cannot read is broken as a whole: it keeps the instances
/// it held, each presented as broken, recovers none of them, and retains the
/// failure it had. The relay's account of why is the message, as for a check
/// that holds once.
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_substrate_check_is_broken_at_check_level() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (relay, cluster) = registered(&mut conn).await;
		ingest::ingest(
			&mut conn,
			relay,
			filing(
				"node-pools",
				SubstrateOutcome::Instances(vec![
					pool("general", None, CheckResult::Passed),
					pool("gpu", None, CheckResult::Failed),
				]),
				None,
			),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();

		let refused = serde_json::json!({
			"refused": {"verb": "list", "resource": "nodepools.karpenter.sh"},
		});
		ingest::ingest(
			&mut conn,
			relay,
			filing(
				"node-pools",
				SubstrateOutcome::Broken {
					detail: Some(refused.clone()),
				},
				Some("the relay is not permitted to list nodepools.karpenter.sh"),
			),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();

		let issue = state(&mut conn, cluster, "node-pools").await;
		assert_eq!(issue.observed_result, Some(CheckResult::Broken));
		assert_eq!(
			issue.effective_result,
			Some(CheckResult::Failed),
			"brokenness neither confirms nor clears the failure it had",
		);
		let instances = issue.stored_instances().expect("its instances are kept");
		assert_eq!(
			instances.0.keys().map(String::as_str).collect::<Vec<_>>(),
			["general", "gpu"],
			"a broken check recovers none of its instances",
		);
		assert!(
			instances
				.0
				.values()
				.all(|i| i.observed == CheckResult::Broken),
			"each presented as broken",
		);
		assert_eq!(
			issue.detail,
			Some(refused),
			"what was refused is the check's"
		);
		assert_eq!(
			issue.message, "the relay is not permitted to list nodepools.karpenter.sh",
			"the relay's account of why it could not read the check is what an operator reads, \
			 though the check held instances",
		);

		// A check that held no instances is broken as the check that holds
		// once, and the relay's account of it is what an operator reads.
		ingest::ingest(
			&mut conn,
			relay,
			filing(
				"workloads-running",
				SubstrateOutcome::Broken {
					detail: Some(serde_json::json!({"message": "watch failing"})),
				},
				Some("the relay cannot read the cluster: watch failing"),
			),
			Placement::Cluster(cluster),
		)
		.await
		.unwrap();
		let issue = state(&mut conn, cluster, "workloads-running").await;
		assert_eq!(issue.effective_result, Some(CheckResult::Broken));
		assert!(issue.stored_instances().is_none());
		assert_eq!(
			issue.message,
			"the relay cannot read the cluster: watch failing"
		);
	})
	.await
}
