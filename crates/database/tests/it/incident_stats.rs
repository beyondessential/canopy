//! `Incident::stats_for` must dedupe `(incident_id, issue_id)` before
//! counting. `incident_issues` is keyed on `(incident_id, issue_id,
//! joined_at)`, so an issue that leaves and rejoins the same incident
//! produces multiple rows for the same pair. Without dedup, the issue
//! count inflates by the number of rejoins, and the note counts get
//! multiplied by the same factor.

use database::issues::Incident;
use diesel_async::SimpleAsyncConnection as _;
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread")]
async fn stats_for_dedupes_repeat_join_rows() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		let group_id = Uuid::new_v4();
		let server_id = Uuid::new_v4();
		let device_id = Uuid::new_v4();
		let issue_id = Uuid::new_v4();
		let incident_id = Uuid::new_v4();
		let issue_note = Uuid::new_v4();
		let incident_note = Uuid::new_v4();

		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{device_id}', 'machine'); \
			 INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g'); \
			 WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{server_id}', '{group_id}', '{device_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{server_id}', 'https://example.com', 'tamanu-central', '{group_id}', '{server_id}'); \
			 INSERT INTO issues \
				(id, application_id, device_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen) \
			   VALUES \
				('{issue_id}', '{server_id}', '{device_id}', 'test', 'r', 'r', 'failed', 'failed', 'm', true, NOW(), NOW()); \
			 INSERT INTO incidents (id, server_group_id, opened_at) \
			   VALUES ('{incident_id}', '{group_id}', NOW()); \
			 INSERT INTO issue_notes (id, issue_id, author, body) \
			   VALUES ('{issue_note}', '{issue_id}', 'op', 'jn'); \
			 INSERT INTO incident_notes (id, incident_id, author, body) \
			   VALUES ('{incident_note}', '{incident_id}', 'op', 'in'); \
			 INSERT INTO incident_issues (incident_id, issue_id, joined_at, left_at) VALUES \
				('{incident_id}', '{issue_id}', NOW() - interval '5 min', NOW() - interval '4 min'), \
				('{incident_id}', '{issue_id}', NOW() - interval '4 min', NOW() - interval '3 min'), \
				('{incident_id}', '{issue_id}', NOW() - interval '3 min', NOW() - interval '2 min'), \
				('{incident_id}', '{issue_id}', NOW() - interval '2 min', NOW() - interval '1 min'), \
				('{incident_id}', '{issue_id}', NOW(), NULL);"
		))
		.await
		.expect("seed");
		drop(conn);

		let pool = database::init_to(&url);
		let stats = Incident::stats_for(&pool, &[incident_id])
			.await
			.expect("stats_for");
		let s = stats.get(&incident_id).expect("stats present");
		assert_eq!(s.issue_count, 1, "one distinct issue despite 5 join rows");
		assert_eq!(
			s.note_count, 2,
			"one incident_note + one issue_note, not 1 + 5"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn stats_for_handles_missing_and_empty_inputs() {
	commons_tests::db::TestDb::run(async |_conn, url| {
		let pool = database::init_to(&url);
		assert!(
			Incident::stats_for(&pool, &[])
				.await
				.expect("empty")
				.is_empty()
		);

		let phantom = Uuid::new_v4();
		let stats = Incident::stats_for(&pool, &[phantom])
			.await
			.expect("phantom");
		let s = stats.get(&phantom).expect("phantom entry");
		assert_eq!(s.issue_count, 0);
		assert_eq!(s.note_count, 0);
	})
	.await
}
