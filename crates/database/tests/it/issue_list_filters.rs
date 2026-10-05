//! `Issue::list` filters in SQL, so `limit` bounds the *filtered* set.
//!
//! Narrowing a page after the fact instead lets a busy fleet push a quiet
//! server's issues past the limit, and the caller — the MCP `find_issues`
//! tool — reports that server clean.

use commons_tests::db::TestDb;
use database::diesel_async::AsyncPgConnection;
use database::issues::{Issue, IssueListFilters};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut AsyncPgConnection) -> Uuid {
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	sql_query("WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) INSERT INTO applications (host, type, machine_id) SELECT $1, 'tamanu-central', m.id FROM m RETURNING id")
		.bind::<sql_types::Text, _>(host)
		.get_result::<RowId>(conn)
		.await
		.expect("insert server")
		.id
}

/// Insert an active issue for `application_id`, with `last_seen` backdated by
/// `age_secs` so the ordering the limit applies to is controllable.
async fn insert_issue(
	conn: &mut AsyncPgConnection,
	application_id: Uuid,
	r#ref: &str,
	age_secs: i64,
) {
	sql_query(
		"INSERT INTO issues (application_id, source, \"ref\", message, active, last_seen, last_degraded_at) \
		 VALUES ($1, 'test', $2, 'boom', true, NOW() - ($3 || ' seconds')::INTERVAL, NOW())",
	)
	.bind::<sql_types::Uuid, _>(application_id)
	.bind::<sql_types::Text, _>(r#ref)
	.bind::<sql_types::Text, _>(age_secs.to_string())
	.execute(conn)
	.await
	.expect("insert issue");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_filter_is_applied_before_the_limit() {
	TestDb::run(|mut conn, _url| async move {
		let noisy = insert_server(&mut conn).await;
		let quiet = insert_server(&mut conn).await;

		// The quiet server's one issue was seen longest ago, so it sorts last
		// and a limit applied before filtering would cut it off.
		for i in 0..10 {
			insert_issue(&mut conn, noisy, &format!("noisy-{i}"), i).await;
		}
		insert_issue(&mut conn, quiet, "quiet-1", 1000).await;

		let found = Issue::list(
			&mut conn,
			IssueListFilters {
				application_id: Some(quiet),
				..Default::default()
			},
			5,
		)
		.await
		.expect("list");

		assert_eq!(
			found.len(),
			1,
			"the quiet server's issue must survive a limit smaller than the fleet's issue count",
		);
		assert_eq!(found[0].r#ref, "quiet-1");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_server_filter_excludes_other_servers() {
	TestDb::run(|mut conn, _url| async move {
		let a = insert_server(&mut conn).await;
		let b = insert_server(&mut conn).await;
		insert_issue(&mut conn, a, "a-1", 1).await;
		insert_issue(&mut conn, b, "b-1", 2).await;

		let found = Issue::list(
			&mut conn,
			IssueListFilters {
				application_id: Some(a),
				..Default::default()
			},
			100,
		)
		.await
		.expect("list");
		assert_eq!(found.len(), 1);
		assert_eq!(found[0].application_id, Some(a));
	})
	.await;
}
