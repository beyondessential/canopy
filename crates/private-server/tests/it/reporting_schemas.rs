//! Asking for a pair's build.
//!
//! spec: RPT

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use uuid::Uuid;

/// A group whose central reports 2.60.0, and a published 2.59.0 nothing runs.
async fn seed(conn: &mut AsyncPgConnection) -> (Uuid, Uuid, Uuid) {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let central = Uuid::new_v4();
	let consumer = Uuid::new_v4();
	let ran = Uuid::new_v4();
	let unrun = Uuid::new_v4();

	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
		   ('{ran}', 2, 60, 0, '', 'published'),
		   ('{unrun}', 2, 59, 0, '', 'published');

		 INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka');
		 INSERT INTO machines (id, group_id) VALUES ('{machine}', '{group}');
		 INSERT INTO applications (id, type, name, host, machine_id, group_id) VALUES
		   ('{central}', 'tamanu-central', 'central', 'https://c', '{machine}', '{group}');
		 INSERT INTO application_reported_detail (application_id, source, reported_at, version)
		   VALUES ('{central}', 'tamanu', NOW(), '2.60.0');

		 INSERT INTO devices (id, role) VALUES ('{consumer}', 'backup-restore');
		 INSERT INTO restore_consumer_capabilities
		   (consumer_device_id, intent, description, semantics, params)
		   VALUES ('{consumer}', 'reporting-schema', '',
		           '[\"once\",\"migrate\",\"reporting-schema\"]'::jsonb, '[]'::jsonb);
		 INSERT INTO restore_replicas
		   (consumer_device_id, group_id, type, intent, name, enabled, params, publishes_schemas)
		   VALUES ('{consumer}', '{group}', 'tamanu-postgres', 'reporting-schema', 'builds',
		           true, '{{}}'::jsonb, true)"
	))
	.await
	.expect("seed");

	(group, ran, unrun)
}

/// A version the group neither runs nor is moving to is not one of its pairs,
/// and an ask against it would stand for good: nothing dispatches it and
/// nothing clears it.
#[tokio::test(flavor = "multi_thread")]
async fn an_ask_for_a_version_the_group_does_not_run_is_refused() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, ran, unrun) = seed(&mut conn).await;

		private
			.post("/api/reporting_schemas/build")
			.json(&serde_json::json!({ "group_id": group, "version_id": ran }))
			.await
			.assert_status_ok();

		private
			.post("/api/reporting_schemas/build")
			.json(&serde_json::json!({ "group_id": group, "version_id": unrun }))
			.await
			.assert_status_bad_request();
	})
	.await;
}
