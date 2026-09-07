//! The `2026-09-03-205207-0000_incident_environment` migration places each
//! existing incident on the environment its members resolve to.
//!
//! Replays it for real: reverts the migration, seeds incidents in the shape they
//! had before it, then re-applies it. An incident left without a rank reads as
//! targeting the group, so the monitor's startup reconcile would close every one
//! of them and open a replacement, notifying twice for the same live trouble.

use commons_types::status::CheckResult;
use database::issues::{CheckStateStamp, NewEvent, reconcile_open_incidents};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str =
	include_str!("../../../../migrations/2026-09-03-205207-0000_incident_environment/up.sql");
const DOWN: &str =
	include_str!("../../../../migrations/2026-09-03-205207-0000_incident_environment/down.sql");

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct MaybeRank {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	rank: Option<String>,
}

#[derive(QueryableByName)]
struct Kind {
	#[diesel(sql_type = sql_types::Text)]
	kind: String,
}

async fn group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('site') RETURNING id")
		.get_result(conn)
		.await
		.expect("group");
	row.id
}

/// One application on a box of its own, carrying `rank` exactly as given so a
/// legacy or unrecognised spelling can be seeded.
async fn member(
	conn: &mut diesel_async::AsyncPgConnection,
	group: Uuid,
	rank: Option<&str>,
	host: &str,
) -> Uuid {
	let machine: RowId = sql_query("INSERT INTO machines (group_id) VALUES ($1) RETURNING id")
		.bind::<sql_types::Uuid, _>(group)
		.get_result(conn)
		.await
		.expect("machine");
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, group_id, rank, machine_id, is_monitored) \
		 VALUES ('tamanu-central', $1, $2, $3, $4, true) RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(group))
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("application");
	row.id
}

async fn fail(conn: &mut diesel_async::AsyncPgConnection, application: Uuid, check: &str) {
	NewEvent {
		source: "alertd".into(),
		r#ref: check.into(),
		description: None,
		message: format!("{check} is failing"),
		active: Some(true),
		occurred_at: None,
	}
	.save_with_state(
		conn,
		application,
		None,
		Some(&CheckStateStamp {
			check: check.into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
		}),
		false,
	)
	.await
	.expect("file a check");
}

/// Put the database in the state the deployed one is in when the new code
/// starts: incidents exist and none carries a rank. Seed through the model layer
/// first and revert after, since filing a check reads `incidents.rank` and the
/// revert drops it.
async fn revert(conn: &mut diesel_async::AsyncPgConnection) {
	conn.batch_execute(DOWN).await.expect("revert");
}

async fn apply(conn: &mut diesel_async::AsyncPgConnection) {
	conn.batch_execute(UP).await.expect("apply");
}

async fn open_ranks(
	conn: &mut diesel_async::AsyncPgConnection,
	group: Uuid,
) -> Vec<Option<String>> {
	let rows: Vec<MaybeRank> = sql_query(
		"SELECT rank FROM incidents WHERE server_group_id = $1 AND closed_at IS NULL \
		 ORDER BY rank NULLS LAST",
	)
	.bind::<sql_types::Uuid, _>(group)
	.load(conn)
	.await
	.expect("open incidents");
	rows.into_iter().map(|r| r.rank).collect()
}

async fn pending_notices(conn: &mut diesel_async::AsyncPgConnection) -> Vec<String> {
	let rows: Vec<Kind> = sql_query(
		"SELECT kind FROM slack_outbox WHERE delivered_at IS NULL \
		 AND kind IN ('incident_open', 'incident_resolve') ORDER BY kind",
	)
	.load(conn)
	.await
	.expect("outbox");
	rows.into_iter().map(|r| r.kind).collect()
}

/// Mark what the group's incident already told Slack, so a close here would be
/// a resolve the channel actually sees.
async fn mark_open_delivered(conn: &mut diesel_async::AsyncPgConnection) {
	sql_query("UPDATE slack_outbox SET delivered_at = NOW() WHERE delivered_at IS NULL")
		.execute(conn)
		.await
		.expect("mark delivered");
}

/// The headline case: the reconcile that follows the deploy has nothing to move,
/// so the incident keeps its identity and the channel hears nothing.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn the_backfill_leaves_the_reconcile_nothing_to_do() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let production = member(&mut conn, g, Some("production"), "http://prod.invalid/").await;

		fail(&mut conn, production, "app_down").await;
		mark_open_delivered(&mut conn).await;
		revert(&mut conn).await;
		let before: Vec<RowId> =
			sql_query("SELECT id FROM incidents WHERE closed_at IS NULL AND server_group_id = $1")
				.bind::<sql_types::Uuid, _>(g)
				.load(&mut conn)
				.await
				.expect("incidents");
		assert_eq!(before.len(), 1, "one open incident before the migration");
		let held = before[0].id;

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![Some("production".to_string())],
			"the backfill placed it on its member's environment",
		);

		reconcile_open_incidents(&mut conn)
			.await
			.expect("reconcile");

		let after: Vec<RowId> =
			sql_query("SELECT id FROM incidents WHERE closed_at IS NULL AND server_group_id = $1")
				.bind::<sql_types::Uuid, _>(g)
				.load(&mut conn)
				.await
				.expect("incidents");
		assert_eq!(
			after.iter().map(|r| r.id).collect::<Vec<_>>(),
			vec![held],
			"the same incident is still open, not a replacement",
		);
		assert!(
			pending_notices(&mut conn).await.is_empty(),
			"and the channel is told nothing, neither a resolve nor a fresh open",
		);
	})
	.await
}

/// Without the backfill this is what the deploy does, which is the reason for
/// it: a resolve for trouble nobody has touched, then a fresh page for it.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn an_unranked_incident_would_close_and_reopen() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let production = member(&mut conn, g, Some("production"), "http://prod.invalid/").await;

		fail(&mut conn, production, "app_down").await;
		mark_open_delivered(&mut conn).await;
		sql_query("UPDATE incidents SET rank = NULL WHERE server_group_id = $1")
			.bind::<sql_types::Uuid, _>(g)
			.execute(&mut conn)
			.await
			.expect("strip the rank the backfill would have set");

		reconcile_open_incidents(&mut conn)
			.await
			.expect("reconcile");

		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![Some("production".to_string())],
			"a replacement opened on the environment",
		);
		let closed: Vec<MaybeRank> = sql_query(
			"SELECT rank FROM incidents WHERE server_group_id = $1 AND closed_at IS NOT NULL",
		)
		.bind::<sql_types::Uuid, _>(g)
		.load(&mut conn)
		.await
		.expect("closed incidents");
		assert_eq!(closed.len(), 1, "and the group's own closed behind it");
		assert_eq!(
			pending_notices(&mut conn).await,
			vec!["incident_open".to_string(), "incident_resolve".to_string()],
			"both notices are queued for one unbroken span of trouble",
		);
	})
	.await
}

/// A group check asserts something held once for the group, so an incident made
/// only of them has no environment to be placed on.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_groups_own_incident_stays_on_the_group() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		member(&mut conn, g, Some("production"), "http://prod.invalid/").await;

		database::issues::raise_group_event_with_state(
			&mut conn,
			g,
			"backup_stale",
			None,
			"the repository is stale",
			true,
			Some(&CheckStateStamp {
				check: "backup_stale".into(),
				observed: CheckResult::Failed,
				effective: CheckResult::Failed,
				escalates: false,
				detail: None,
			}),
		)
		.await
		.expect("file a group check");
		revert(&mut conn).await;

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![None],
			"the group's own trouble is not placed on its production",
		);
	})
	.await
}

/// Ranks stored before the canonical spellings landed still resolve, since the
/// column they are read into normalises them and the new constraint does not.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_spelling_lands_on_its_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let legacy = member(&mut conn, g, Some("live"), "http://legacy.invalid/").await;

		fail(&mut conn, legacy, "app_down").await;
		revert(&mut conn).await;

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![Some("production".to_string())],
			"a box stored as live is production",
		);
	})
	.await
}

/// A spelling the mapping does not know must not fail the deploy. The incident
/// stays on the group, which is where it already was.
///
/// `ServerRank` refuses to read such a value, so the application layer cannot
/// file against one either: the row is seeded canonical and rewritten once the
/// revert has taken the model layer out of the way.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_spelling_stays_on_the_group() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let odd = member(&mut conn, g, Some("production"), "http://odd.invalid/").await;

		fail(&mut conn, odd, "app_down").await;
		revert(&mut conn).await;
		sql_query("UPDATE applications SET rank = 'preprod' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(odd)
			.execute(&mut conn)
			.await
			.expect("store a spelling the mapping does not know");

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![None],
			"the migration ran and left it on the group",
		);
	})
	.await
}

/// An incident holding members at two ranks cannot be one incident under the new
/// model. It takes the more urgent of them, and the reconcile moves the rest.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn an_incident_spanning_two_ranks_takes_the_higher() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let production = member(&mut conn, g, Some("production"), "http://prod.invalid/").await;
		let test = member(&mut conn, g, Some("test"), "http://test.invalid/").await;

		fail(&mut conn, test, "app_down").await;
		fail(&mut conn, production, "app_down").await;
		revert(&mut conn).await;
		// The revert leaves one open incident per group, holding whichever
		// member was filed first. Before the split it held both, so re-attach
		// the other one.
		sql_query(
			"INSERT INTO incident_issues (incident_id, issue_id, joined_at) \
			 SELECT i.id, s.id, NOW() FROM incidents i, issues s \
			 WHERE i.server_group_id = $1 AND i.closed_at IS NULL \
			   AND s.application_id = $2 \
			   AND NOT EXISTS (SELECT 1 FROM incident_issues x \
			                   WHERE x.incident_id = i.id AND x.issue_id = s.id \
			                     AND x.left_at IS NULL)",
		)
		.bind::<sql_types::Uuid, _>(g)
		.bind::<sql_types::Uuid, _>(production)
		.execute(&mut conn)
		.await
		.expect("re-attach the second member");
		let before: Vec<RowId> =
			sql_query("SELECT id FROM incidents WHERE closed_at IS NULL AND server_group_id = $1")
				.bind::<sql_types::Uuid, _>(g)
				.load(&mut conn)
				.await
				.expect("incidents");
		assert_eq!(
			before.len(),
			1,
			"one incident holds both before the migration"
		);

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![Some("production".to_string())],
			"production is the one it keeps",
		);

		reconcile_open_incidents(&mut conn)
			.await
			.expect("reconcile");
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec![Some("production".to_string()), Some("test".to_string())],
			"and the test box's issue moves to its own",
		);
	})
	.await
}

/// Rolling back has to collapse the environments onto the group, which can hold
/// only one open incident. The earliest survives and the rest close, since the
/// alternative is a schema the old code cannot index.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn the_revert_collapses_two_environments_onto_the_group() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let production = member(&mut conn, g, Some("production"), "http://prod.invalid/").await;
		let test = member(&mut conn, g, Some("test"), "http://test.invalid/").await;

		fail(&mut conn, production, "app_down").await;
		fail(&mut conn, test, "app_down").await;
		let open: Vec<RowId> = sql_query(
			"SELECT id FROM incidents WHERE server_group_id = $1 AND closed_at IS NULL \
			 ORDER BY opened_at",
		)
		.bind::<sql_types::Uuid, _>(g)
		.load(&mut conn)
		.await
		.expect("incidents");
		assert_eq!(open.len(), 2, "one per environment before the revert");
		let earliest = open[0].id;

		revert(&mut conn).await;

		let still_open: Vec<RowId> =
			sql_query("SELECT id FROM incidents WHERE server_group_id = $1 AND closed_at IS NULL")
				.bind::<sql_types::Uuid, _>(g)
				.load(&mut conn)
				.await
				.expect("incidents");
		assert_eq!(
			still_open.iter().map(|r| r.id).collect::<Vec<_>>(),
			vec![earliest],
			"the earliest open incident is the one the group keeps",
		);

		let stranded: Vec<RowId> = sql_query(
			"SELECT ii.issue_id AS id FROM incident_issues ii \
			 JOIN incidents i ON i.id = ii.incident_id \
			 WHERE i.closed_at IS NOT NULL AND ii.left_at IS NULL",
		)
		.load(&mut conn)
		.await
		.expect("stranded members");
		assert!(
			stranded.is_empty(),
			"a closed incident holds no live members, or the old code reads it as still open",
		);
	})
	.await
}

/// The revert restores the one-open-incident-per-group index it replaced, so the
/// old code's guarantee is back rather than the column merely being dropped.
// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn the_revert_restores_the_group_wide_uniqueness() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let production = member(&mut conn, g, Some("production"), "http://prod.invalid/").await;
		fail(&mut conn, production, "app_down").await;

		revert(&mut conn).await;

		let refused =
			sql_query("INSERT INTO incidents (server_group_id, opened_at) VALUES ($1, NOW())")
				.bind::<sql_types::Uuid, _>(g)
				.execute(&mut conn)
				.await;
		assert!(
			refused.is_err(),
			"a second open incident on the group is refused once the index is back",
		);
	})
	.await
}
