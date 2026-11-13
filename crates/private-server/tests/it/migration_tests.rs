//! The operator's view of where a group stands: one entry per server with a
//! candidate version, carrying its verdict and the test behind it.

use commons_tests::diesel_async::SimpleAsyncConnection;
use serde_json::{Value, json};

const GROUP: &str = "bbbbbbbb-0000-0000-0000-000000000001";
const CENTRAL: &str = "bbbbbbbb-0000-0000-0000-0000000000a0";
const FACILITY: &str = "bbbbbbbb-0000-0000-0000-0000000000b0";

const FLEET: &str = "INSERT INTO versions (major, minor, patch, changelog, status)
		VALUES (2, 62, 0, 'x', 'published'), (2, 63, 0, 'x', 'published');
	INSERT INTO server_groups (id, name)
		VALUES ('bbbbbbbb-0000-0000-0000-000000000001', 'Kamaka');
	INSERT INTO machines (name, id, group_id) VALUES
		('box', 'bbbbbbbb-0000-0000-0000-0000000000a0',
		 'bbbbbbbb-0000-0000-0000-000000000001'),
		('box', 'bbbbbbbb-0000-0000-0000-0000000000b0',
		 'bbbbbbbb-0000-0000-0000-000000000001');
	INSERT INTO applications (id, name, host, type, rank, group_id, machine_id) VALUES
		('bbbbbbbb-0000-0000-0000-0000000000a0', 'central',
		 'https://central.example.com', 'tamanu-central', 'production',
		 'bbbbbbbb-0000-0000-0000-000000000001',
		 'bbbbbbbb-0000-0000-0000-0000000000a0'),
		('bbbbbbbb-0000-0000-0000-0000000000b0', 'facility',
		 'https://facility.example.com', 'tamanu-facility', 'production',
		 'bbbbbbbb-0000-0000-0000-000000000001',
		 'bbbbbbbb-0000-0000-0000-0000000000b0');
	INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
		('bbbbbbbb-0000-0000-0000-0000000000a0', 'test', '{}'::jsonb, '2.62.0'),
		('bbbbbbbb-0000-0000-0000-0000000000b0', 'test', '{}'::jsonb, '2.62.0');";

#[tokio::test(flavor = "multi_thread")]
async fn for_group_reports_a_verdict_per_server() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(FLEET).await.unwrap();
		conn.batch_execute(
			"INSERT INTO upgrade_plans (group_id, rank, target_version_id, created_by)
				SELECT 'bbbbbbbb-0000-0000-0000-000000000001', 'production', id, 'test@example.com'
				FROM versions WHERE major = 2 AND minor = 63 AND patch = 0;",
		)
		.await
		.unwrap();

		let resp = private
			.post("/api/migration_tests/for_group")
			.json(&json!({ "group_id": GROUP }))
			.await;
		resp.assert_status_ok();
		let verdicts: Vec<Value> = resp.json();

		assert_eq!(
			verdicts.len(),
			2,
			"the plan covers the whole group: got {verdicts:?}"
		);
		for id in [CENTRAL, FACILITY] {
			let entry = verdicts
				.iter()
				.find(|v| v["server_id"] == id)
				.unwrap_or_else(|| panic!("no entry for {id}: {verdicts:?}"));
			assert_eq!(entry["target_version"], "2.63.0");
			assert_eq!(entry["verdict"], "nottested");
			assert!(entry["latest"].is_null());
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn for_group_reports_nothing_without_a_plan() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(FLEET).await.unwrap();

		let resp = private
			.post("/api/migration_tests/for_group")
			.json(&json!({ "group_id": GROUP }))
			.await;
		resp.assert_status_ok();
		let verdicts: Vec<Value> = resp.json();

		assert!(
			verdicts.is_empty(),
			"a newer version existing is not what asks for a test: {verdicts:?}"
		);
	})
	.await;
}

/// Only the servers a testing declaration restores are set up for testing, and
/// the fleet view counts those alone.
// spec: RST#verdicts
#[tokio::test(flavor = "multi_thread")]
async fn only_a_server_a_declaration_tests_is_set_up() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(FLEET).await.unwrap();
		conn.batch_execute(
			"INSERT INTO upgrade_plans (group_id, rank, target_version_id, created_by)
				SELECT 'bbbbbbbb-0000-0000-0000-000000000001', 'production', id, 'test@example.com'
				FROM versions WHERE major = 2 AND minor = 63 AND patch = 0;
			INSERT INTO devices (id, role) VALUES
				('bbbbbbbb-0000-0000-0000-0000000000d0', 'backup-restore');
			INSERT INTO restore_consumer_capabilities (consumer_device_id, intent, semantics)
				VALUES ('bbbbbbbb-0000-0000-0000-0000000000d0', 'upgrade', '[\"migrate\"]');
			INSERT INTO restore_replicas
				(consumer_device_id, group_id, machine_id, type, intent, name)
				VALUES ('bbbbbbbb-0000-0000-0000-0000000000d0',
					'bbbbbbbb-0000-0000-0000-000000000001',
					'bbbbbbbb-0000-0000-0000-0000000000a0',
					'tamanu-postgres', 'upgrade', 'central-upgrade');",
		)
		.await
		.unwrap();
		private
			.post("/api/migration_tests/request")
			.json(&json!({ "group_id": GROUP, "rank": "production" }))
			.await
			.assert_status_ok();

		let verdicts: Vec<Value> = private
			.post("/api/migration_tests/for_group")
			.json(&json!({ "group_id": GROUP }))
			.await
			.json();
		let entry = |id: &str| {
			verdicts
				.iter()
				.find(|v| v["server_id"] == id)
				.unwrap_or_else(|| panic!("no entry for {id}: {verdicts:?}"))
		};
		assert_eq!(entry(CENTRAL)["set_up"], true);
		assert_eq!(
			entry(CENTRAL)["test_request"]["requested_by"],
			"admin@localhost"
		);
		assert_eq!(entry(FACILITY)["set_up"], false);
		assert!(entry(FACILITY)["test_request"].is_null());

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = fleet.iter().find(|r| r["group_id"] == GROUP).unwrap();
		let tests = row["tests"].as_array().unwrap();
		assert_eq!(tests.len(), 1, "the facility is not counted: {row}");
		assert_eq!(tests[0]["server_id"], CENTRAL);
		assert_eq!(tests[0]["name"], "central");
	})
	.await;
}
