//! Deferred incident (re-)evaluation. The status-ingest path records issue
//! state synchronously, then enqueues the server; the reeval worker drains
//! the queue and runs the incident work — which takes the per-group lock —
//! off the request path.
//!
//! These cover the database layer: draining the queue opens an incident, and
//! — the case [`reevaluate_open_issues_for_server`] can't, because it only
//! walks *active* issues — a recovered, now-inactive member is driven to
//! LEAVE its incident.

use commons_types::status::CheckResult;
use database::issues::{self, CheckStateStamp, NewEvent};
use diesel::prelude::*;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_grouped_server(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let group: RowId = sql_query(
		"INSERT INTO server_groups (name, slack_close_delay) \
		 VALUES ('reeval-group', INTERVAL '5 minutes') RETURNING id",
	)
	.get_result(conn)
	.await
	.expect("group");
	let server: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id) INSERT INTO applications (type, host, group_id, rank, machine_id) SELECT 'tamanu-central', 'http://reeval.invalid/', $1, 'production', m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group.id)
	.get_result(conn)
	.await
	.expect("server");
	server.id
}

/// Record a check on `server_id`. `defer` mirrors the ingest path: when true
/// the issue state is written but incident evaluation is skipped.
async fn record_check(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
	result: CheckResult,
	defer: bool,
) {
	let active = matches!(
		result,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let stamp = CheckStateStamp {
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
		message: "m".into(),
		active: Some(active),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), defer)
	.await
	.expect("record check");
}

#[derive(QueryableByName)]
struct IncFlags {
	#[diesel(sql_type = sql_types::Bool)]
	is_open: bool,
	#[diesel(sql_type = sql_types::Bool)]
	is_lingering: bool,
}

async fn latest_incident(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
) -> Option<IncFlags> {
	sql_query(
		"SELECT (i.closed_at IS NULL) AS is_open, (i.closing_at IS NOT NULL) AS is_lingering \
		 FROM incidents i JOIN applications s ON i.server_group_id = s.group_id \
		 WHERE s.id = $1 ORDER BY i.opened_at DESC LIMIT 1",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.get_result::<IncFlags>(conn)
	.await
	.optional()
	.expect("incident flags")
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

async fn queue_len(conn: &mut diesel_async::AsyncPgConnection) -> i64 {
	sql_query("SELECT count(*) AS n FROM incident_reeval_queue")
		.get_result::<Count>(conn)
		.await
		.expect("queue len")
		.n
}

#[tokio::test(flavor = "multi_thread")]
async fn draining_the_queue_opens_the_incident_and_empties_the_queue() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;

		// Ingest records a failing check but defers the incident work and
		// enqueues the server (mirrors the public-server handler).
		record_check(&mut conn, server_id, "health/db", CheckResult::Failed, true).await;
		issues::enqueue_incident_reeval(&mut conn, server_id)
			.await
			.expect("enqueue");

		assert!(
			latest_incident(&mut conn, server_id).await.is_none(),
			"no incident before the queue is drained"
		);
		assert_eq!(queue_len(&mut conn).await, 1);

		let processed = issues::process_incident_reeval_queue(&mut conn, i64::MAX)
			.await
			.expect("drain");
		assert_eq!(processed, 1);
		assert!(
			latest_incident(&mut conn, server_id)
				.await
				.is_some_and(|i| i.is_open),
			"draining the queue opens the incident"
		);
		assert_eq!(queue_len(&mut conn).await, 0, "queue emptied after drain");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn deferred_reeval_leaves_incident_when_member_recovers() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;

		// Open an incident inline with a failing check.
		record_check(
			&mut conn,
			server_id,
			"health/db",
			CheckResult::Failed,
			false,
		)
		.await;
		let inc = latest_incident(&mut conn, server_id)
			.await
			.expect("incident open");
		assert!(inc.is_open && !inc.is_lingering, "open, not yet lingering");

		// Recovery arrives on the deferred path: the issue goes inactive, but
		// its incident membership is not re-evaluated inline.
		record_check(&mut conn, server_id, "health/db", CheckResult::Passed, true).await;
		let inc = latest_incident(&mut conn, server_id)
			.await
			.expect("still open");
		assert!(
			inc.is_open && !inc.is_lingering,
			"the leave is deferred: still a member until reeval runs"
		);

		// The worker's re-evaluation walks inactive members too, so the
		// recovered check leaves and the incident starts lingering.
		issues::reevaluate_incidents_for_server(&mut conn, server_id)
			.await
			.expect("reeval");
		let inc = latest_incident(&mut conn, server_id)
			.await
			.expect("still open (lingering)");
		assert!(
			inc.is_open && inc.is_lingering,
			"recovered member left -> incident now lingering"
		);
	})
	.await
}
