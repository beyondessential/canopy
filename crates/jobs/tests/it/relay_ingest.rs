//! Where a relay's substrate filing lands, and what is refused before it does.

use commons_types::{device::DeviceRole, status::CheckResult};
use database::{KubernetesCluster, devices::Device};
use jobs::relay::ingest::{self, Placement};
use relay_protocol::{Filing, FilingTarget, SUBSTRATE_SOURCE, SubstrateFiling, SubstrateInstance};

fn filing(instances: Vec<SubstrateInstance>) -> Filing {
	Filing::Substrate(SubstrateFiling {
		target: FilingTarget::Cluster,
		check: "node-pools".into(),
		instances,
		title: None,
		message: "m".into(),
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	})
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
		KubernetesCluster::register(&mut conn, draft.id)
			.await
			.unwrap();

		assert!(
			ingest::ingest(
				&mut conn,
				relay.id,
				filing(vec![]),
				Placement::Cluster(draft.id)
			)
			.await
			.is_err()
		);
		ingest::ingest(
			&mut conn,
			relay.id,
			filing(vec![SubstrateInstance::only(CheckResult::Passed, None)]),
			Placement::Cluster(draft.id),
		)
		.await
		.unwrap();
		let issues = database::issues::Issue::list_by_source_ref_for_clusters(
			&mut conn,
			SUBSTRATE_SOURCE,
			"node-pools",
			&[draft.id],
		)
		.await
		.unwrap();
		assert_eq!(issues.len(), 1);
	})
	.await
}
