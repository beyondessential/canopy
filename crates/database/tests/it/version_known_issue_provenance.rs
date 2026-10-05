//! A known issue can name the server whose data provoked it. The reference is
//! provenance, so removing the server clears the reference and leaves the issue
//! standing against its version range.

use database::version_known_issues::VersionKnownIssue;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[tokio::test(flavor = "multi_thread")]
async fn server_reference_survives_as_provenance() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let group: RowId =
			sql_query("INSERT INTO server_groups (name) VALUES ('kamaka') RETURNING id")
				.get_result(&mut conn)
				.await
				.expect("group");
		let server: RowId =
			sql_query("WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $2) RETURNING id) INSERT INTO applications (type, host, group_id, machine_id) SELECT 'tamanu-central', $1, $2, m.id FROM m RETURNING id")
				.bind::<sql_types::Text, _>("kamaka-central")
				.bind::<sql_types::Uuid, _>(group.id)
				.get_result(&mut conn)
				.await
				.expect("server");

		let filed = VersionKnownIssue::add(
			&mut conn,
			(2, 63, 0),
			"migration-test",
			"backfillNoteTypeIds did not complete.",
			Some(server.id),
		)
		.await
		.expect("add with provenance");
		assert_eq!(filed.application_id, Some(server.id));

		assert!(
			!VersionKnownIssue::version_is_ready(&mut conn, 2, 63, 0)
				.await
				.expect("readiness"),
			"a filed issue holds the version back"
		);

		sql_query("DELETE FROM applications WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server.id)
			.execute(&mut conn)
			.await
			.expect("delete server");

		let issues = VersionKnownIssue::list_for_minor(&mut conn, 2, 63)
			.await
			.expect("list");
		let still_there = issues
			.iter()
			.find(|issue| issue.id == filed.id)
			.expect("the issue outlives the server");
		assert_eq!(
			still_there.application_id, None,
			"provenance clears rather than cascading the issue away"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_filed_issue_names_no_server() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let filed = VersionKnownIssue::add(
			&mut conn,
			(2, 62, 0),
			"someone@example.com",
			"Reported by hand.",
			None,
		)
		.await
		.expect("add without provenance");
		assert_eq!(filed.application_id, None);
	})
	.await
}
