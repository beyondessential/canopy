//! Checks with instances on the private API: the instances a target's checks
//! present, silencing one instance at each scope a check is silenced at, the
//! silences listing which instance each quiets, the issue's degraded instances,
//! and the rule-authoring sample.
//!
//! spec: CHK#checks-with-instances, CHK#silencing-one-instance

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use commons_types::device::DeviceRole;
use commons_types::source::SUBSTRATE_SOURCE;
use commons_types::status::CheckResult;
use database::devices::Device;
use database::issues::{CheckInstance, CheckOutcome, InstancedCheckFiling, Scope};
use serde_json::{Value, json};
use uuid::Uuid;

const CHECK: &str = "sync_facility_stale";
const REF: &str = "health/sync_facility_stale";

/// A tamanu-central in a group, on its own machine, with the calling device
/// enrolled on that machine. Returns the application's id (also the machine's)
/// and the group's.
async fn central(conn: &mut AsyncPgConnection, device_id: Uuid) -> (Uuid, Uuid) {
	let id = Uuid::new_v4();
	let group_id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'instanced-group');
		 WITH m AS (INSERT INTO machines (id, group_id, device_id) VALUES ('{id}', '{group_id}', '{device_id}') RETURNING id)
		 INSERT INTO applications (id, host, type, group_id, machine_id) VALUES
			('{id}', 'https://central.example.com', 'tamanu-central', '{group_id}', '{id}');"
	))
	.await
	.expect("seed central");
	(id, group_id)
}

/// Review every catalog entry for `check` at a `failed` ceiling, so an
/// instance's failure is not capped at the unreviewed warning.
async fn review_at_failed(conn: &mut AsyncPgConnection, check: &str) {
	conn.batch_execute(&format!(
		"UPDATE check_policies SET ceiling = 'failed', reviewed_at = NOW(), reviewed_by = 'test' \
		 WHERE check_name = '{check}'"
	))
	.await
	.expect("review policy");
}

async fn push(public: &commons_tests::axum_test::TestServer, cert: &str, id: Uuid, body: Value) {
	public
		.post(&format!("/status/{id}"))
		.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
		.json(&body)
		.await
		.assert_status_ok();
}

/// One check reported with instances: `(key, result, label)`, an empty label
/// leaving the instance named by its key.
fn instanced(check: &str, instances: &[(&str, &str, &str)]) -> Value {
	json!({
		"check": check,
		"detail": { "fail_minutes": 30 },
		"instances": instances
			.iter()
			.map(|(key, result, label)| {
				let mut instance = json!({ "result": result, "detail": { "minutes": key.len() } });
				if !label.is_empty() {
					instance["label"] = json!(label);
				}
				(key.to_string(), instance)
			})
			.collect::<serde_json::Map<_, _>>(),
	})
}

/// Central reporting its facility staleness: Northgate failing, Ridge warning,
/// Eastbay passing.
fn three_facilities() -> Value {
	json!({ "health": [instanced(CHECK, &[
		("north", "failed", "Northgate"),
		("ridge", "warning", "Ridge"),
		("east", "passed", "Eastbay"),
	])] })
}

/// A central with its facility staleness pushed and reviewed, so Northgate's
/// failure is a failure.
async fn central_reporting(
	conn: &mut AsyncPgConnection,
	public: &commons_tests::axum_test::TestServer,
	cert: &str,
	device_id: Uuid,
) -> (Uuid, Uuid) {
	let (id, group_id) = central(conn, device_id).await;
	push(public, cert, id, three_facilities()).await;
	review_at_failed(conn, CHECK).await;
	push(public, cert, id, three_facilities()).await;
	(id, group_id)
}

async fn post(private: &commons_tests::axum_test::TestServer, path: &str, body: Value) -> Value {
	let response = private.post(path).json(&body).await;
	response.assert_status_success();
	if response.as_bytes().is_empty() {
		Value::Null
	} else {
		response.json()
	}
}

/// One check out of a consolidated checks list.
fn check_in<'a>(checks: &'a Value, name: &str) -> &'a Value {
	checks["checks"]
		.as_array()
		.expect("checks list")
		.iter()
		.find(|c| c["check"] == name)
		.unwrap_or_else(|| panic!("{name} not listed in {checks}"))
}

async fn application_check(
	private: &commons_tests::axum_test::TestServer,
	id: Uuid,
	name: &str,
) -> Value {
	let detail = post(
		private,
		"/api/fleet/applications/get_detail",
		json!({ "server_id": id }),
	)
	.await;
	check_in(&detail["checks"], name).clone()
}

/// The keys an entry lists, in the order listed.
fn keys(entry: &Value) -> Vec<&str> {
	entry["instances"]
		.as_array()
		.expect("instances list")
		.iter()
		.map(|i| i["key"].as_str().expect("key"))
		.collect()
}

fn instance<'a>(entry: &'a Value, key: &str) -> &'a Value {
	entry["instances"]
		.as_array()
		.expect("instances list")
		.iter()
		.find(|i| i["key"] == key)
		.unwrap_or_else(|| panic!("{key} not listed in {entry}"))
}

/// A target's check lists its degraded instances, most urgent first, each with
/// its own result and fields, counts its passing ones, and keeps the shared
/// fields as the check's detail. A plain check lists none.
#[tokio::test(flavor = "multi_thread")]
async fn a_targets_check_lists_its_degraded_instances_and_counts_the_passing() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;
			push(
				&public,
				&cert,
				id,
				json!({ "health": [
					instanced(CHECK, &[
						("north", "failed", "Northgate"),
						("ridge", "warning", "Ridge"),
						("east", "passed", "Eastbay"),
					]),
					{ "check": "postgres", "result": "failed", "latency_ms": 9 },
				] }),
			)
			.await;

			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(entry["effective"], "failed");
			assert_eq!(keys(&entry), ["north", "ridge"]);
			let north = instance(&entry, "north");
			assert_eq!(north["label"], "Northgate");
			assert_eq!(north["observed"], "failed");
			assert_eq!(north["effective"], "failed");
			assert_eq!(north["detail"], json!({ "minutes": 5 }));
			assert_eq!(north["silenced_on_target"], false);
			assert_eq!(north["silenced_on_group"], false);
			assert_eq!(instance(&entry, "ridge")["effective"], "warning");
			assert_eq!(entry["passing_instances"], 1);
			assert_eq!(entry["skipped_instances"], 0);
			assert_eq!(entry["detail"], json!({ "fail_minutes": 30 }));

			let plain = application_check(&private, id, "postgres").await;
			assert_eq!(plain["instances"], json!([]));
			assert_eq!(plain["passing_instances"], 0);
			assert_eq!(plain["skipped_instances"], 0);
			assert_eq!(plain["detail"], json!({ "latency_ms": 9 }));
		},
	)
	.await;
}

/// Instances skipped other than by a silence, reported skipped or graded so by
/// a rule, are counted apart from the passing ones, now and as of the push;
/// silenced instances are listed instead, and a check silenced whole counts
/// none.
#[tokio::test(flavor = "multi_thread")]
async fn instances_skipped_other_than_by_a_silence_are_counted() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;
			// `minutes` is the key's length, so this rule skips `westfield` alone.
			conn.batch_execute(&format!(
				"UPDATE check_policies SET rules = '{}'::jsonb WHERE check_name = '{CHECK}'",
				json!({ "if": [{ "==": [{ "var": "check.minutes" }, 9] }, "skipped"] }),
			))
			.await
			.expect("rule");
			push(
				&public,
				&cert,
				id,
				json!({ "health": [instanced(CHECK, &[
					("north", "failed", "Northgate"),
					("westfield", "failed", "Westfield"),
					("dormant", "skipped", ""),
					("ridge", "warning", "Ridge"),
					("east", "passed", "Eastbay"),
				])] }),
			)
			.await;
			post(
				&private,
				"/api/silenced_refs/silence_server",
				json!({ "server_id": id, "source": "alertd", "ref": REF, "instance": "ridge" }),
			)
			.await;

			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(keys(&entry), ["north", "ridge"]);
			assert_eq!(entry["passing_instances"], 1);
			assert_eq!(entry["skipped_instances"], 2);

			let snapshot = post(&private, "/api/statuses/snapshot", json!({ "server_id": id })).await;
			let past = check_in(&snapshot["checks"], CHECK);
			assert_eq!(keys(past), ["north", "ridge"]);
			assert_eq!(past["passing_instances"], 1);
			assert_eq!(past["skipped_instances"], 2);

			conn.batch_execute(&format!(
				"INSERT INTO scoped_check_policies (application_id, source, subject, application_type, check_name, ceiling) VALUES \
					('{id}', 'alertd', 'application', 'tamanu-central', '{CHECK}', 'skipped');"
			))
			.await
			.expect("silence the check whole");
			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(entry["silenced"], true);
			assert_eq!(keys(&entry), ["ridge"]);
			assert_eq!(entry["skipped_instances"], 0);
		},
	)
	.await;
}

/// Silencing one instance on the application quiets that instance alone: the
/// check is graded on the rest, the instance stays listed as silenced at the
/// application, and the silence lists the instance's label and that it is
/// reported. Unsilencing brings it back.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silenced_on_the_application_is_listed_and_lifted() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;

			let silence = post(
				&private,
				"/api/silenced_refs/silence_server",
				json!({ "server_id": id, "source": "alertd", "ref": REF, "instance": "north" }),
			)
			.await;
			assert_eq!(silence["instance"], "north");
			assert_eq!(silence["instance_label"], "Northgate");
			assert_eq!(silence["instance_reported"], true);

			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(entry["effective"], "warning", "graded on the rest");
			assert_eq!(entry["silenced"], false, "the check itself is not silenced");
			assert_eq!(keys(&entry), ["ridge", "north"]);
			let north = instance(&entry, "north");
			assert_eq!(north["effective"], "skipped");
			assert_eq!(north["observed"], "failed");
			assert_eq!(north["silenced_on_target"], true);
			assert_eq!(north["silenced_on_group"], false);
			assert_eq!(entry["passing_instances"], 1);

			let listed = post(
				&private,
				"/api/silenced_refs/list_for_server",
				json!({ "server_id": id }),
			)
			.await;
			assert_eq!(listed.as_array().unwrap().len(), 1);
			assert_eq!(listed[0]["ref"], REF);
			assert_eq!(listed[0]["instance"], "north");
			assert_eq!(listed[0]["instance_label"], "Northgate");
			assert_eq!(listed[0]["instance_reported"], true);

			post(
				&private,
				"/api/silenced_refs/unsilence_server",
				json!({ "server_id": id, "source": "alertd", "ref": REF, "instance": "north" }),
			)
			.await;
			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(entry["effective"], "failed");
			assert_eq!(keys(&entry), ["north", "ridge"]);
			let listed = post(
				&private,
				"/api/silenced_refs/list_for_server",
				json!({ "server_id": id }),
			)
			.await;
			assert_eq!(listed, json!([]));
		},
	)
	.await;
}

/// A whole-check silence lists with no instance, and an instance silence whose
/// key the check stops reporting is listed as not reported, without a label,
/// and kept until an operator clears it.
#[tokio::test(flavor = "multi_thread")]
async fn a_silence_outliving_its_instance_is_marked_not_reported() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;
			post(
				&private,
				"/api/silenced_refs/silence_server",
				json!({ "server_id": id, "source": "alertd", "ref": REF, "instance": "north" }),
			)
			.await;
			conn.batch_execute(&format!(
				"INSERT INTO check_policies (source, subject, application_type, check_name) VALUES \
					('alertd', 'application', 'tamanu-central', 'postgres');
				 INSERT INTO scoped_check_policies (application_id, source, subject, application_type, check_name, ceiling) VALUES \
					('{id}', 'alertd', 'application', 'tamanu-central', 'postgres', 'skipped');"
			))
			.await
			.expect("silence postgres whole");

			push(
				&public,
				&cert,
				id,
				json!({ "health": [instanced(CHECK, &[("ridge", "warning", "Ridge")])] }),
			)
			.await;

			let listed = post(
				&private,
				"/api/silenced_refs/list_for_server",
				json!({ "server_id": id }),
			)
			.await;
			let listed = listed.as_array().unwrap();
			assert_eq!(listed.len(), 2);
			let gone = listed
				.iter()
				.find(|s| s["instance"] == "north")
				.expect("the instance silence is kept");
			assert_eq!(gone["instance_reported"], false);
			assert_eq!(gone["instance_label"], Value::Null);
			let whole = listed
				.iter()
				.find(|s| s["ref"] == "health/postgres")
				.expect("the whole-check silence");
			assert_eq!(whole["instance"], Value::Null);
			assert_eq!(whole["instance_label"], Value::Null);
			assert_eq!(whole["instance_reported"], Value::Null);
		},
	)
	.await;
}

/// A group silence on an instance quiets it on the group's application, lists
/// with the instance's label read off the group's states, and lifts.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silenced_on_the_group_is_listed_and_lifted() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, group_id) = central_reporting(&mut conn, &public, &cert, device_id).await;
			let args = json!({
				"server_group_id": group_id,
				"source": "alertd",
				"ref": REF,
				"application_type": "tamanu-central",
				"instance": "north",
			});
			let silence = post(&private, "/api/silenced_refs/silence_group", args.clone()).await;
			assert_eq!(silence["instance"], "north");
			assert_eq!(silence["instance_label"], "Northgate");
			assert_eq!(silence["instance_reported"], true);

			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(entry["effective"], "warning");
			let north = instance(&entry, "north");
			assert_eq!(north["effective"], "skipped");
			assert_eq!(north["silenced_on_target"], false);
			assert_eq!(north["silenced_on_group"], true);

			let listed = post(
				&private,
				"/api/silenced_refs/list_for_group",
				json!({ "server_group_id": group_id }),
			)
			.await;
			assert_eq!(listed[0]["instance"], "north");
			assert_eq!(listed[0]["instance_label"], "Northgate");
			assert_eq!(listed[0]["instance_reported"], true);

			post(&private, "/api/silenced_refs/unsilence_group", args).await;
			let entry = application_check(&private, id, CHECK).await;
			assert_eq!(entry["effective"], "failed");
			assert_eq!(instance(&entry, "north")["silenced_on_group"], false);
		},
	)
	.await;
}

/// A machine check with instances is silenced one instance at a time on the
/// machine, and presents so on the machine and on the application it carries.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silenced_on_the_machine_is_listed_and_lifted() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central(&mut conn, device_id).await;
			let disks = json!({ "health": [instanced("disk_free", &[
				("/", "failed", "Root"),
				("/data", "passed", ""),
			])] });
			push(&public, &cert, id, disks.clone()).await;
			review_at_failed(&mut conn, "disk_free").await;
			push(&public, &cert, id, disks).await;

			let args = json!({
				"machine_id": id,
				"source": "alertd",
				"ref": "health/disk_free",
				"instance": "/",
			});
			let silence = post(&private, "/api/silenced_refs/silence_machine", args.clone()).await;
			assert_eq!(silence["instance"], "/");
			assert_eq!(silence["instance_label"], "Root");
			assert_eq!(silence["instance_reported"], true);

			let machine = post(
				&private,
				"/api/fleet/machines/get_detail",
				json!({ "machine_id": id }),
			)
			.await;
			let entry = check_in(&machine["checks"], "disk_free");
			assert_eq!(entry["effective"], "passed");
			assert_eq!(keys(entry), ["/"]);
			assert_eq!(instance(entry, "/")["silenced_on_target"], true);
			assert_eq!(entry["passing_instances"], 1);
			// The box's check presents on its application the same way.
			let on_application = application_check(&private, id, "disk_free").await;
			assert_eq!(on_application["subject"], "machine");
			assert_eq!(instance(&on_application, "/")["silenced_on_target"], true);

			let listed = post(
				&private,
				"/api/silenced_refs/list_for_machine",
				json!({ "machine_id": id }),
			)
			.await;
			assert_eq!(listed[0]["instance"], "/");
			assert_eq!(listed[0]["instance_reported"], true);

			post(&private, "/api/silenced_refs/unsilence_machine", args).await;
			let machine = post(
				&private,
				"/api/fleet/machines/get_detail",
				json!({ "machine_id": id }),
			)
			.await;
			let entry = check_in(&machine["checks"], "disk_free");
			assert_eq!(entry["effective"], "failed");
			assert_eq!(instance(entry, "/")["silenced_on_target"], false);
		},
	)
	.await;
}

/// A cluster's instanced check is silenced one instance at a time on the
/// cluster, its only scope.
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silenced_on_the_cluster_is_listed_and_lifted() {
	commons_tests::server::run(async |mut conn, _, private| {
		let identity = Device::create_at_role(
			&mut conn,
			Uuid::new_v4().as_bytes().to_vec(),
			DeviceRole::Relay,
			Some("relay".into()),
		)
		.await
		.expect("enrol a relay")
		.id;
		let draft = database::KubernetesCluster::create_draft(&mut conn, "ops-main", identity)
			.await
			.expect("draft");
		let cluster = database::KubernetesCluster::register(&mut conn, draft.id)
			.await
			.expect("register");
		let pool = |key: &str, observed| CheckInstance {
			key: key.into(),
			label: Some(format!("pool {key}")),
			observed,
			detail: None,
		};
		database::issues::file_check_instances(
			&mut conn,
			InstancedCheckFiling {
				source: SUBSTRATE_SOURCE,
				scope: Scope::Cluster(cluster.id),
				device_id: None,
				check: "node-pool-capacity",
				title: Some("Node pool capacity"),
				detail: None,
				outcome: CheckOutcome::Instances(vec![
					pool("general", CheckResult::Failed),
					pool("gpu", CheckResult::Passed),
				]),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
			&|graded| graded.message("node-pool-capacity"),
		)
		.await
		.expect("file at the cluster");

		let cluster_check = async || {
			let detail = post(
				&private,
				"/api/fleet/clusters/get_detail",
				json!({ "cluster_id": cluster.id }),
			)
			.await;
			check_in(&detail["checks"], "node-pool-capacity").clone()
		};
		let entry = cluster_check().await;
		assert_eq!(entry["effective"], "failed");
		assert_eq!(keys(&entry), ["general"]);
		assert_eq!(entry["passing_instances"], 1);

		let args = json!({
			"kubernetes_cluster_id": cluster.id,
			"source": SUBSTRATE_SOURCE,
			"ref": "node-pool-capacity",
			"instance": "general",
		});
		let silence = post(&private, "/api/silenced_refs/silence_cluster", args.clone()).await;
		assert_eq!(silence["instance_label"], "pool general");
		assert_eq!(silence["instance_reported"], true);

		let entry = cluster_check().await;
		assert_eq!(entry["effective"], "passed");
		assert_eq!(instance(&entry, "general")["silenced_on_target"], true);

		let listed = post(
			&private,
			"/api/silenced_refs/list_for_cluster",
			json!({ "kubernetes_cluster_id": cluster.id }),
		)
		.await;
		assert_eq!(listed[0]["instance"], "general");
		assert_eq!(listed[0]["instance_reported"], true);

		post(&private, "/api/silenced_refs/unsilence_cluster", args).await;
		let entry = cluster_check().await;
		assert_eq!(entry["effective"], "failed");
		assert_eq!(instance(&entry, "general")["silenced_on_target"], false);
	})
	.await;
}

/// An instance key is never empty, so an instance silence naming one is
/// refused rather than read as the whole check.
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_instance_key_is_refused() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;
			let response = private
				.post("/api/silenced_refs/silence_server")
				.json(&json!({ "server_id": id, "source": "alertd", "ref": REF, "instance": "" }))
				.await;
			response.assert_status_bad_request();
		},
	)
	.await;
}

/// A past moment is re-graded from the push it holds, instances included and
/// through the instance silences now in force, the same shape the live view
/// presents.
#[tokio::test(flavor = "multi_thread")]
async fn a_past_snapshot_presents_the_instances_it_held() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;
			post(
				&private,
				"/api/silenced_refs/silence_server",
				json!({ "server_id": id, "source": "alertd", "ref": REF, "instance": "ridge" }),
			)
			.await;
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
			let then = jiff::Timestamp::now();
			tokio::time::sleep(std::time::Duration::from_millis(20)).await;
			push(
				&public,
				&cert,
				id,
				json!({ "health": [instanced(CHECK, &[
					("north", "passed", "Northgate"),
					("ridge", "passed", "Ridge"),
					("east", "passed", "Eastbay"),
				])] }),
			)
			.await;

			let past = post(
				&private,
				"/api/statuses/snapshot",
				json!({ "server_id": id, "at": then }),
			)
			.await;
			let entry = check_in(&past["checks"], CHECK);
			assert_eq!(entry["observed"], "failed");
			assert_eq!(entry["effective"], "failed");
			assert_eq!(entry["detail"], json!({ "fail_minutes": 30 }));
			assert_eq!(keys(entry), ["north", "ridge"]);
			assert_eq!(instance(entry, "north")["effective"], "failed");
			assert_eq!(instance(entry, "north")["detail"], json!({ "minutes": 5 }));
			let ridge = instance(entry, "ridge");
			assert_eq!(ridge["observed"], "warning");
			assert_eq!(ridge["effective"], "skipped");
			assert_eq!(ridge["silenced_on_target"], true);
			assert_eq!(entry["passing_instances"], 1);
			assert_eq!(past["checks"]["health_state"], "unhealthy");

			let now = post(
				&private,
				"/api/statuses/snapshot",
				json!({ "server_id": id }),
			)
			.await;
			let entry = check_in(&now["checks"], CHECK);
			assert_eq!(entry["effective"], "passed");
			// Ridge passes now, but its silence still lists it.
			assert_eq!(keys(entry), ["ridge"]);
			assert_eq!(entry["passing_instances"], 2);
			assert_eq!(now["checks"]["health_state"], "healthy");
		},
	)
	.await;
}

/// The snapshot reads a check's nested `detail` as its fields, as ingestion
/// does, rather than as one field named `detail`.
#[tokio::test(flavor = "multi_thread")]
async fn a_past_snapshot_reads_a_nested_detail() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central(&mut conn, device_id).await;
			push(
				&public,
				&cert,
				id,
				json!({ "health": [
					{ "check": "postgres", "result": "warning", "detail": { "latency_ms": 9 } },
				] }),
			)
			.await;
			let snapshot = post(
				&private,
				"/api/statuses/snapshot",
				json!({ "server_id": id }),
			)
			.await;
			let entry = check_in(&snapshot["checks"], "postgres");
			assert_eq!(entry["detail"], json!({ "latency_ms": 9 }));
			assert_eq!(entry["instances"], json!([]));
		},
	)
	.await;
}

/// An issue for a check with instances carries its degraded ones, most urgent
/// first, for silencing one from the issue; a plain check's carries none.
#[tokio::test(flavor = "multi_thread")]
async fn an_issue_carries_its_degraded_instances() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central_reporting(&mut conn, &public, &cert, device_id).await;
			push(
				&public,
				&cert,
				id,
				json!({ "health": [
					instanced(CHECK, &[
						("north", "failed", "Northgate"),
						("ridge", "warning", ""),
						("east", "passed", "Eastbay"),
					]),
					{ "check": "postgres", "result": "failed" },
				] }),
			)
			.await;
			let issues = post(
				&private,
				"/api/issues/list_for_server",
				json!({ "application_id": id }),
			)
			.await;
			let issue = |r#ref: &str| {
				issues
					.as_array()
					.unwrap()
					.iter()
					.find(|i| i["ref"] == r#ref)
					.unwrap_or_else(|| panic!("no issue for {ref}"))
					.clone()
			};
			assert_eq!(
				issue(REF)["instances"],
				json!([
					{ "key": "north", "label": "Northgate", "effective": "failed" },
					{ "key": "ridge", "label": null, "effective": "warning" },
				]),
			);
			assert_eq!(issue("health/postgres")["instances"], json!([]));
		},
	)
	.await;
}

/// The rule-authoring sample shows an instanced check as a rule reads its most
/// urgent instance: that instance's fields over the shared ones, and its result.
#[tokio::test(flavor = "multi_thread")]
async fn the_sample_for_an_instanced_check_merges_one_instance_over_the_shared_fields() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			let (id, _) = central(&mut conn, device_id).await;
			push(
				&public,
				&cert,
				id,
				json!({ "health": [{
					"check": CHECK,
					"detail": { "fail_minutes": 30, "minutes": 0 },
					"instances": {
						"east": { "result": "passed", "detail": { "minutes": 1 } },
						"north": { "result": "failed", "label": "Northgate", "detail": { "minutes": 2875 } },
						"ridge": { "result": "warning", "detail": { "minutes": 17 } },
					},
				}] }),
			)
			.await;
			let body = post(
				&private,
				"/api/healthchecks/sample",
				json!({
					"source": "alertd",
					"check_name": CHECK,
					"namespace": { "subject": "application", "application_type": "tamanu-central" },
				}),
			)
			.await;
			assert_eq!(
				body["sample"]["check_extra"],
				json!({ "fail_minutes": 30, "minutes": 2875, "result": "failed" }),
			);
		},
	)
	.await;
}
