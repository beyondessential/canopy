//! A cluster as a check target: its relay's substrate filings landing on it,
//! its reachability swept from their freshness, and its health and silences
//! read at the cluster alone.

use commons_types::device::DeviceRole;
use commons_types::namespace::Namespace;
use commons_types::source::SUBSTRATE_SOURCE;
use commons_types::status::{CheckResult, HealthState};
use database::{
	KubernetesCluster,
	check_policies::CheckPolicy,
	devices::Device,
	issues::{
		CheckInstance, InstancedCheckFiling, Issue, Scope, consolidated_checks_latest_for_cluster,
		file_check_instances,
	},
	silenced_refs::ClusterSilencedRef,
	statuses::{CANOPY_SOURCE, REACHABILITY_REF, Status},
};
use diesel::{sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

async fn relay_identity(conn: &mut AsyncPgConnection) -> Uuid {
	Device::create_at_role(
		conn,
		Uuid::new_v4().as_bytes().to_vec(),
		DeviceRole::Relay,
		Some("relay".into()),
	)
	.await
	.expect("enrol a relay")
	.id
}

async fn registered_cluster(conn: &mut AsyncPgConnection, name: &str) -> KubernetesCluster {
	let identity = relay_identity(conn).await;
	let draft = KubernetesCluster::create_draft(conn, name, identity)
		.await
		.expect("draft");
	KubernetesCluster::register(conn, draft.id)
		.await
		.expect("register")
}

async fn file_substrate(
	conn: &mut AsyncPgConnection,
	cluster_id: Uuid,
	check: &str,
	instances: Vec<CheckInstance>,
) -> Issue {
	file_check_instances(
		conn,
		InstancedCheckFiling {
			source: SUBSTRATE_SOURCE,
			scope: Scope::Cluster(cluster_id),
			device_id: None,
			check,
			title: Some("A cluster condition"),
			default_ceiling: CheckResult::Failed,
			default_escalates: false,
			documentation: Some("What this check means."),
			instances,
		},
		&|_| "message".into(),
	)
	.await
	.expect("file at the cluster")
}

fn only(observed: CheckResult) -> Vec<CheckInstance> {
	vec![CheckInstance {
		label: String::new(),
		observed,
		detail: None,
	}]
}

async fn reachability(conn: &mut AsyncPgConnection, cluster_id: Uuid) -> Option<Issue> {
	Issue::list_by_source_ref_for_clusters(conn, CANOPY_SOURCE, REACHABILITY_REF, &[cluster_id])
		.await
		.unwrap()
		.into_iter()
		.next()
}

/// Age every filing on a cluster, as if its relay had gone quiet that long ago.
async fn age_filings(conn: &mut AsyncPgConnection, cluster_id: Uuid, minutes: i32) {
	sql_query(
		"UPDATE issues SET last_seen = NOW() - ($2 || ' minutes')::INTERVAL \
		 WHERE kubernetes_cluster_id = $1 AND source <> 'canopy'",
	)
	.bind::<sql_types::Uuid, _>(cluster_id)
	.bind::<sql_types::Text, _>(minutes.to_string())
	.execute(conn)
	.await
	.expect("age filings");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_substrate_check_lands_flat_on_the_cluster() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		let issue = file_substrate(
			&mut conn,
			cluster.id,
			"tailscale-api-proxy",
			only(CheckResult::Failed),
		)
		.await;
		assert_eq!(issue.kubernetes_cluster_id, Some(cluster.id));
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));

		// One catalog entry fleet-wide, not qualified by any application type.
		let policy = CheckPolicy::get(
			&mut conn,
			SUBSTRATE_SOURCE,
			&Namespace::Flat,
			"tailscale-api-proxy",
		)
		.await
		.unwrap()
		.expect("catalogued flat");
		assert!(policy.reviewed_at.is_some(), "registers already reviewed");
		assert_eq!(policy.ceiling, CheckResult::Failed);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_later_filing_does_not_overwrite_an_operators_policy() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			only(CheckResult::Passed),
		)
		.await;
		CheckPolicy::update(
			&mut conn,
			SUBSTRATE_SOURCE,
			&Namespace::Flat,
			"node-pools",
			CheckResult::Warning,
			false,
			None,
			"operator",
		)
		.await
		.unwrap();

		let issue = file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			only(CheckResult::Failed),
		)
		.await;
		assert_eq!(
			issue.effective_result,
			Some(CheckResult::Warning),
			"the operator's ceiling grades the filing",
		);
		let policy = CheckPolicy::get(&mut conn, SUBSTRATE_SOURCE, &Namespace::Flat, "node-pools")
			.await
			.unwrap()
			.unwrap();
		assert_eq!(policy.ceiling, CheckResult::Warning);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn instances_are_graded_together_and_the_most_urgent_wins() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		let issue = file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			vec![
				CheckInstance {
					label: "general".into(),
					observed: CheckResult::Passed,
					detail: Some(serde_json::json!({"pool": "general"})),
				},
				CheckInstance {
					label: "gpu".into(),
					observed: CheckResult::Failed,
					detail: Some(serde_json::json!({"pool": "gpu"})),
				},
			],
		)
		.await;
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		let detail = issue.detail.expect("instance detail");
		assert_eq!(detail["total"], 2);
		assert_eq!(detail["degraded"], 1);
		assert_eq!(detail["instances"][0]["pool"], "gpu");

		// The set is complete each time: a second filing replaces the first.
		let issue = file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			vec![CheckInstance {
				label: "general".into(),
				observed: CheckResult::Passed,
				detail: Some(serde_json::json!({"pool": "general"})),
			}],
		)
		.await;
		assert_eq!(issue.effective_result, Some(CheckResult::Passed));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_issue_opens_no_incident_and_counts_towards_its_health() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		file_substrate(
			&mut conn,
			cluster.id,
			"workloads-running",
			only(CheckResult::Failed),
		)
		.await;

		assert!(
			Scope::Cluster(cluster.id)
				.resolve_incident_target(&mut conn)
				.await
				.unwrap()
				.is_none(),
			"a cluster belongs to no group, so its issues belong to no target",
		);
		let checks = consolidated_checks_latest_for_cluster(&mut conn, cluster.id)
			.await
			.unwrap();
		assert_eq!(checks.health_state, HealthState::Unhealthy);
		assert!(
			checks
				.checks
				.iter()
				.any(|c| c.source == SUBSTRATE_SOURCE && c.check == "workloads-running")
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_silenced_cluster_check_presents_skipped_and_leaves_health() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			only(CheckResult::Failed),
		)
		.await;

		let silence =
			ClusterSilencedRef::add(&mut conn, cluster.id, SUBSTRATE_SOURCE, "node-pools", None)
				.await
				.unwrap();
		assert_eq!(
			silence.r#ref, "node-pools",
			"a reserved source's ref is bare"
		);

		let checks = consolidated_checks_latest_for_cluster(&mut conn, cluster.id)
			.await
			.unwrap();
		let pools = checks
			.checks
			.iter()
			.find(|c| c.check == "node-pools")
			.unwrap();
		assert!(pools.silenced);
		assert_eq!(pools.effective, CheckResult::Skipped);
		assert_eq!(checks.health_state, HealthState::Healthy);

		// Another cluster is untouched by it.
		let other = registered_cluster(&mut conn, "ops-other").await;
		file_substrate(&mut conn, other.id, "node-pools", only(CheckResult::Failed)).await;
		let checks = consolidated_checks_latest_for_cluster(&mut conn, other.id)
			.await
			.unwrap();
		assert_eq!(checks.health_state, HealthState::Unhealthy);

		assert_eq!(
			ClusterSilencedRef::list_for_cluster(&mut conn, cluster.id)
				.await
				.unwrap()
				.len(),
			1
		);
		ClusterSilencedRef::remove(&mut conn, cluster.id, SUBSTRATE_SOURCE, "node-pools")
			.await
			.unwrap();
		assert!(
			ClusterSilencedRef::list_for_cluster(&mut conn, cluster.id)
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_whose_relay_goes_quiet_reads_unreachable_and_recovers() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			only(CheckResult::Passed),
		)
		.await;

		Status::sweep_staleness(&mut conn).await.unwrap();
		assert!(
			reachability(&mut conn, cluster.id)
				.await
				.is_none_or(|i| !i.active),
			"a fresh cluster is reachable",
		);

		// Past the five-minute default.
		age_filings(&mut conn, cluster.id, 6).await;
		Status::sweep_staleness(&mut conn).await.unwrap();
		let issue = reachability(&mut conn, cluster.id).await.expect("filed");
		assert!(issue.active);
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));

		// Filings resume.
		file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			only(CheckResult::Passed),
		)
		.await;
		Status::sweep_staleness(&mut conn).await.unwrap();
		let issue = reachability(&mut conn, cluster.id).await.unwrap();
		assert!(!issue.active);
		assert_eq!(issue.effective_result, Some(CheckResult::Passed));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_cluster_never_filed_against_reads_as_never_reported() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		assert_eq!(cluster.last_reported_at(&mut conn).await.unwrap(), None);
		assert_eq!(
			cluster.reachability(None),
			commons_types::status::ShortStatus::Gone,
		);
		Status::sweep_staleness(&mut conn).await.unwrap();
		let issue = reachability(&mut conn, cluster.id).await.expect("filed");
		assert!(
			issue.message.contains("has never reported"),
			"{}",
			issue.message
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_draft_cluster_is_not_swept() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let identity = relay_identity(&mut conn).await;
		let draft = KubernetesCluster::create_draft(&mut conn, "half-done", identity)
			.await
			.unwrap();
		assert_eq!(Status::sweep_staleness(&mut conn).await.unwrap(), 0);
		assert!(reachability(&mut conn, draft.id).await.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_clusters_threshold_defaults_to_five_minutes_and_governs_the_sweep() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let cluster = registered_cluster(&mut conn, "ops-main").await;
		assert_eq!(cluster.alert_when_down_for.0.as_secs(), 300);

		KubernetesCluster::set_alert_when_down_for(
			&mut conn,
			cluster.id,
			jiff::SignedDuration::from_mins(30),
		)
		.await
		.unwrap();
		file_substrate(
			&mut conn,
			cluster.id,
			"node-pools",
			only(CheckResult::Passed),
		)
		.await;
		age_filings(&mut conn, cluster.id, 10).await;
		Status::sweep_staleness(&mut conn).await.unwrap();
		assert!(
			reachability(&mut conn, cluster.id)
				.await
				.is_none_or(|i| !i.active),
			"ten minutes is inside a thirty-minute threshold",
		);

		assert!(
			KubernetesCluster::set_alert_when_down_for(
				&mut conn,
				cluster.id,
				jiff::SignedDuration::ZERO,
			)
			.await
			.is_err()
		);
	})
	.await
}
