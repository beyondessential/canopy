//! Membership rows must not outlive the incident they name.
//!
//! An incident close only retires the `incidents` row. The issue whose
//! recovery triggered the close is stamped by the leave arm, but sub-failure
//! contributors are held attached for context, and nothing released them —
//! their `incident_issues` rows survived the incident claiming a membership
//! that had ended.
//!
//! That stranded a check outside the incident workflow for good. Live
//! membership is what decides whether a failure opens an incident, and an
//! issue that already looks attached never opens one, so the server sat red
//! with nothing paging and no later event could clear the stale row.
//!
//! Covered here from both sides: the close paths now release whoever is left,
//! and the membership read consults the incident's `closed_at` so any row
//! that predates the fix is inert.

use commons_types::status::CheckResult;
use database::issues::NewEvent;
use diesel::prelude::*;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const BACKFILL_UP: &str = include_str!(
	"../../../../migrations/2026-08-04-220209-0000_release_stranded_incident_members/up.sql"
);

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

/// Application in a fresh group whose linger window is zero, so a recovery closes
/// the incident on the spot rather than leaving it to the sweep.
async fn insert_grouped_server(conn: &mut diesel_async::AsyncPgConnection) -> (Uuid, Uuid) {
	let group: RowId = sql_query(
		"INSERT INTO server_groups (name, slack_close_delay) \
		 VALUES ('stranded-group', INTERVAL '0') RETURNING id",
	)
	.get_result(conn)
	.await
	.expect("group");
	let server: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (group_id) VALUES ($1) RETURNING id) INSERT INTO applications (type, host, group_id, machine_id) SELECT 'tamanu-central', 'http://stranded.invalid/', $1, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group.id)
	.get_result(conn)
	.await
	.expect("server");
	(group.id, server.id)
}

async fn save_event(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
	result: CheckResult,
) {
	let active = matches!(
		result,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let stamp = database::issues::CheckStateStamp {
		check: r#ref.into(),
		observed: result,
		effective: result,
		escalates: false,
		detail: None,
		title: None,
		instanced: None,
	};
	NewEvent {
		source: "test".into(),
		r#ref: r#ref.into(),
		description: None,
		message: format!("{ref_} is {result:?}", ref_ = r#ref),
		active: Some(active),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), false)
	.await
	.expect("save event");
}

async fn open_incident_count(conn: &mut diesel_async::AsyncPgConnection, group_id: Uuid) -> i64 {
	use database::schema::incidents::dsl;
	dsl::incidents
		.filter(dsl::server_group_id.eq(group_id))
		.filter(dsl::closed_at.is_null())
		.count()
		.get_result(conn)
		.await
		.expect("count open incidents")
}

async fn issue_id(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
) -> Uuid {
	let row: RowId = sql_query("SELECT id FROM issues WHERE application_id = $1 AND ref = $2")
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Text, _>(r#ref)
		.get_result(conn)
		.await
		.expect("issue id");
	row.id
}

/// Membership rows for `ref` that are still unstamped, regardless of whether
/// their incident is closed.
async fn unstamped_memberships(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
) -> i64 {
	let row: Count = sql_query(
		"SELECT count(*) AS n FROM incident_issues ii \
		 JOIN issues i ON i.id = ii.issue_id \
		 WHERE i.application_id = $1 AND i.ref = $2 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result(conn)
	.await
	.expect("count memberships");
	row.n
}

/// Re-open every membership row for `ref`, reproducing the rows the close
/// paths used to leave behind.
async fn stranded_as_legacy_data(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
) {
	sql_query(
		"UPDATE incident_issues ii SET left_at = NULL \
		 FROM issues i WHERE i.id = ii.issue_id \
		 AND i.application_id = $1 AND i.ref = $2",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(r#ref)
	.execute(conn)
	.await
	.expect("strand membership");
}

/// The close side: a contributor that never left of its own accord is
/// released when the incident closes, so its membership ends with the
/// incident rather than outliving it.
#[tokio::test(flavor = "multi_thread")]
async fn closing_an_incident_releases_the_members_that_never_left() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		let (group_id, server_id) = insert_grouped_server(&mut conn).await;

		// A failure opens the incident; a warning joins it as a lesser
		// contributor because the target already has one open.
		save_event(
			&mut conn,
			server_id,
			"health/disk_free",
			CheckResult::Failed,
		)
		.await;
		save_event(
			&mut conn,
			server_id,
			"health/pg_tuning",
			CheckResult::Warning,
		)
		.await;
		assert_eq!(
			open_incident_count(&mut conn, group_id).await,
			1,
			"the failure should have opened an incident",
		);
		assert_eq!(
			unstamped_memberships(&mut conn, server_id, "health/pg_tuning").await,
			1,
			"the warning should be a live member while the incident is open",
		);

		// The failure recovers and, with a zero linger window, closes the
		// incident on the spot. The warning never left by itself.
		save_event(
			&mut conn,
			server_id,
			"health/disk_free",
			CheckResult::Passed,
		)
		.await;
		assert_eq!(
			open_incident_count(&mut conn, group_id).await,
			0,
			"the recovery should have closed the incident",
		);
		assert_eq!(
			unstamped_memberships(&mut conn, server_id, "health/pg_tuning").await,
			0,
			"the close should have released the warning contributor",
		);
	})
	.await;
}

/// The read side: a row left over from before the close paths released their
/// members names a closed incident, so it is not live membership and must not
/// stop a later failure from opening an incident.
#[tokio::test(flavor = "multi_thread")]
async fn a_stranded_member_can_still_open_a_later_incident() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		let (group_id, server_id) = insert_grouped_server(&mut conn).await;

		save_event(
			&mut conn,
			server_id,
			"health/disk_free",
			CheckResult::Failed,
		)
		.await;
		save_event(
			&mut conn,
			server_id,
			"health/pg_tuning",
			CheckResult::Warning,
		)
		.await;
		save_event(
			&mut conn,
			server_id,
			"health/disk_free",
			CheckResult::Passed,
		)
		.await;
		assert_eq!(
			open_incident_count(&mut conn, group_id).await,
			0,
			"the recovery should have closed the incident",
		);

		// Put the warning's membership back the way the old close paths left
		// it: attached to an incident that closed weeks ago.
		stranded_as_legacy_data(&mut conn, server_id, "health/pg_tuning").await;
		assert_eq!(
			unstamped_memberships(&mut conn, server_id, "health/pg_tuning").await,
			1,
			"the stranded row is the state under test",
		);

		save_event(
			&mut conn,
			server_id,
			"health/pg_tuning",
			CheckResult::Failed,
		)
		.await;
		assert_eq!(
			open_incident_count(&mut conn, group_id).await,
			1,
			"a failure on a stranded issue must open a new incident",
		);
	})
	.await;
}

/// The backfill retires rows whose incident has closed and leaves live
/// membership alone.
#[tokio::test(flavor = "multi_thread")]
async fn the_backfill_releases_stranded_members_and_spares_live_ones() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		let (group_id, server_id) = insert_grouped_server(&mut conn).await;

		// Two warnings, so neither opens an incident on its own and both are
		// free to be linked by hand below.
		save_event(&mut conn, server_id, "health/stale", CheckResult::Warning).await;
		save_event(&mut conn, server_id, "health/live", CheckResult::Warning).await;
		let stale_issue = issue_id(&mut conn, server_id, "health/stale").await;
		let live_issue = issue_id(&mut conn, server_id, "health/live").await;

		let closed: RowId = sql_query(
			"INSERT INTO incidents (server_group_id, opened_at, closed_at) \
			 VALUES ($1, now() - INTERVAL '3 days', now() - INTERVAL '2 days') RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(group_id)
		.get_result(&mut conn)
		.await
		.expect("closed incident");
		let open: RowId = sql_query(
			"INSERT INTO incidents (server_group_id, opened_at) \
			 VALUES ($1, now() - INTERVAL '1 hour') RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(group_id)
		.get_result(&mut conn)
		.await
		.expect("open incident");

		for (incident, issue) in [(closed.id, stale_issue), (open.id, live_issue)] {
			sql_query(
				"INSERT INTO incident_issues (incident_id, issue_id, joined_at) \
				 VALUES ($1, $2, now() - INTERVAL '3 days')",
			)
			.bind::<sql_types::Uuid, _>(incident)
			.bind::<sql_types::Uuid, _>(issue)
			.execute(&mut conn)
			.await
			.expect("link issue");
		}

		conn.batch_execute(BACKFILL_UP).await.expect("backfill");

		let stale_matches_close: Count = sql_query(
			"SELECT count(*) AS n FROM incident_issues ii JOIN incidents i ON i.id = ii.incident_id \
			 WHERE ii.issue_id = $1 AND ii.left_at = i.closed_at",
		)
		.bind::<sql_types::Uuid, _>(stale_issue)
		.get_result(&mut conn)
		.await
		.expect("stale row");
		assert_eq!(
			stale_matches_close.n, 1,
			"the stranded row should have left when its incident closed",
		);
		assert_eq!(
			unstamped_memberships(&mut conn, server_id, "health/live").await,
			1,
			"membership of a still-open incident is live and must be untouched",
		);
	})
	.await;
}
