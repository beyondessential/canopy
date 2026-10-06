//! `Incident::get_with_issues` must dedupe `(incident_id, issue_id)`
//! before returning rows. `incident_issues` is keyed on `(incident_id,
//! issue_id, joined_at)`, so an issue that leaves and rejoins the same
//! incident produces a row per rejoin. Without dedup the incident detail
//! timeline renders the same issue hundreds of times for a flapping
//! source.

use database::issues::Incident;
use diesel_async::SimpleAsyncConnection as _;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread")]
async fn get_with_issues_dedupes_repeat_join_rows() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let group_id = Uuid::new_v4();
		let server_id = Uuid::new_v4();
		let device_id = Uuid::new_v4();
		let issue_a = Uuid::new_v4();
		let issue_b = Uuid::new_v4();
		let incident_id = Uuid::new_v4();

		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{device_id}', 'machine'); \
			 INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g'); \
			 WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{server_id}', '{group_id}', '{device_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
				('{server_id}', 'https://example.com', 'tamanu-central', '{group_id}', 'production', '{server_id}'); \
			 INSERT INTO issues \
				(id, application_id, device_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen) \
			   VALUES \
				('{issue_a}', '{server_id}', '{device_id}', 'test', 'a', 'a', 'failed', 'failed', 'm', true, NOW(), NOW()), \
				('{issue_b}', '{server_id}', '{device_id}', 'test', 'b', 'b', 'failed', 'failed', 'm', true, NOW(), NOW()); \
			 INSERT INTO incidents (id, server_group_id, rank, opened_at) \
			   VALUES ('{incident_id}', '{group_id}', 'production', NOW()); \
			 INSERT INTO incident_issues (incident_id, issue_id, joined_at, left_at) VALUES \
				('{incident_id}', '{issue_a}', NOW() - interval '5 min', NOW() - interval '4 min'), \
				('{incident_id}', '{issue_a}', NOW() - interval '4 min', NOW() - interval '3 min'), \
				('{incident_id}', '{issue_a}', NOW() - interval '3 min', NOW() - interval '2 min'), \
				('{incident_id}', '{issue_a}', NOW() - interval '2 min', NOW() - interval '1 min'), \
				('{incident_id}', '{issue_a}', NOW(), NULL), \
				('{incident_id}', '{issue_b}', NOW() - interval '10 min', NOW() - interval '9 min');"
		))
		.await
		.expect("seed");

		let (incident, rows) = Incident::get_with_issues(&mut conn, incident_id)
			.await
			.expect("get_with_issues");
		assert_eq!(incident.id, incident_id);
		assert_eq!(rows.len(), 2, "two distinct issues despite 5 rejoin rows");

		// Most recent link metadata wins for the flapping issue: the open
		// link (left_at NULL) is what the UI should see.
		let a = rows
			.iter()
			.find(|(l, _)| l.issue_id == issue_a)
			.expect("issue a present");
		assert!(a.0.left_at.is_none(), "kept the most recent open link");

		let b = rows
			.iter()
			.find(|(l, _)| l.issue_id == issue_b)
			.expect("issue b present");
		assert!(b.0.left_at.is_some(), "single historical link preserved");
	})
	.await
}
