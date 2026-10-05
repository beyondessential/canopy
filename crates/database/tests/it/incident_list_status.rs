//! `Incident::list_open_since` applies its status filter in SQL, so `limit`
//! bounds the *filtered* set.
//!
//! The lookback window routinely holds far more sub-grace flap rows than the
//! limit, and they sort by `opened_at` alongside everything else. Narrowing a
//! page afterwards therefore hides a still-open incident that opened a few
//! days ago behind a wall of recent noise — the MCP `find_incidents` tool
//! reports it neither in its list nor its counts.

use commons_tests::db::TestDb;
use database::issues::{Incident, IncidentStatusFilter};
use diesel_async::SimpleAsyncConnection as _;
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

#[tokio::test(flavor = "multi_thread")]
async fn the_status_filter_is_applied_before_the_limit() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = Uuid::new_v4();
		let old_open = Uuid::new_v4();

		// One incident still open, opened 6 days ago, plus 10 closed flap rows
		// opened in the last hour. Newest-opened first, the open one sorts last.
		let mut sql = format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g'); \
			 INSERT INTO incidents (id, server_group_id, rank, opened_at) \
			   VALUES ('{old_open}', '{group_id}', 'production', NOW() - interval '6 days');"
		);
		for i in 0..10 {
			let id = Uuid::new_v4();
			sql.push_str(&format!(
				"INSERT INTO incidents (id, server_group_id, rank, opened_at, closed_at) VALUES \
				 ('{id}', '{group_id}', 'production', NOW() - interval '{i} minutes', NOW());"
			));
		}
		conn.batch_execute(&sql).await.expect("seed incidents");

		let since = Timestamp::now() - SignedDuration::from_hours(7 * 24);
		let open = Incident::list_open_since(
			&mut conn,
			since,
			Some(group_id),
			IncidentStatusFilter::Open,
			5,
		)
		.await
		.expect("list");

		assert_eq!(
			open.len(),
			1,
			"the still-open incident must survive a limit smaller than the flap count",
		);
		assert_eq!(open[0].id, old_open);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_filter_selects_the_right_incidents() {
	TestDb::run(|mut conn, _url| async move {
		// A partial unique index allows only one open incident per group, so
		// the two open ones live in different groups and the listing isn't
		// group-filtered.
		let group_a = Uuid::new_v4();
		let group_b = Uuid::new_v4();
		let still_open = Uuid::new_v4();
		let closed = Uuid::new_v4();
		let resolved = Uuid::new_v4();

		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group_a}', 'a'), ('{group_b}', 'b'); \
			 INSERT INTO incidents (id, server_group_id, rank, opened_at) \
			   VALUES ('{still_open}', '{group_a}', 'production', NOW() - interval '3 hours'); \
			 INSERT INTO incidents (id, server_group_id, rank, opened_at, closed_at) \
			   VALUES ('{closed}', '{group_a}', 'production', NOW() - interval '2 hours', NOW()); \
			 INSERT INTO incidents (id, server_group_id, rank, opened_at, resolved_at, resolved_by) \
			   VALUES ('{resolved}', '{group_b}', 'production', NOW() - interval '1 hour', NOW(), 'op');"
		))
		.await
		.expect("seed incidents");

		let since = Timestamp::now() - SignedDuration::from_hours(7 * 24);
		let ids = async |conn: &mut _, status| -> Vec<Uuid> {
			Incident::list_open_since(conn, since, None, status, 100)
				.await
				.expect("list")
				.into_iter()
				.map(|i| i.id)
				.collect()
		};

		let all = ids(&mut conn, IncidentStatusFilter::All).await;
		assert_eq!(all.len(), 3);

		let open = ids(&mut conn, IncidentStatusFilter::Open).await;
		// An operator-resolved incident that hasn't closed is still open.
		assert_eq!(open.len(), 2);
		assert!(open.contains(&still_open) && open.contains(&resolved));

		let res = ids(&mut conn, IncidentStatusFilter::Resolved).await;
		assert_eq!(res, vec![resolved]);
	})
	.await;
}
