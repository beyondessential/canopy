//! The auto-close path in `re_evaluate_incident_membership` counts only
//! effective-failure contributors when deciding whether an incident
//! still has reason to stay open. Lesser issues that joined while the
//! incident was open stay attached (audit trail / Slack context) but
//! don't hold the incident open by themselves.

use commons_types::status::CheckResult;
use database::issues::{Incident, NewEvent};
use database::slack_outbox::{KIND_INCIDENT_OPEN, KIND_INCIDENT_RESOLVE, SlackOutbox};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_grouped_server(conn: &mut diesel_async::AsyncPgConnection, host: &str) -> Uuid {
	let group: RowId =
		sql_query("INSERT INTO server_groups (name) VALUES ('test-group') RETURNING id")
			.get_result(conn)
			.await
			.expect("group");
	let row: RowId =
		sql_query("WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $2) RETURNING id) INSERT INTO applications (type, host, group_id, machine_id) SELECT 'tamanu-central', $1, $2, m.id FROM m RETURNING id")
			.bind::<sql_types::Text, _>(host)
			.bind::<sql_types::Uuid, _>(group.id)
			.get_result(conn)
			.await
			.expect("server");
	row.id
}

async fn save_event(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
	result: CheckResult,
	escalates: bool,
	message: &str,
) {
	let active = matches!(
		result,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let stamp = database::issues::CheckStateStamp {
		check: r#ref.into(),
		observed: result,
		effective: result,
		escalates,
		detail: None,
		title: None,
		instanced: None,
	};
	NewEvent {
		source: "test".into(),
		r#ref: r#ref.into(),
		description: None,
		message: message.into(),
		active: Some(active),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), false)
	.await
	.expect("save event");
}

/// Move the pending `incident_open` row past its `cancel_pending_open`
/// window so that closing the incident enqueues a `incident_resolve`
/// rather than cancelling the open. Mirrors the helper used by
/// slack_outbox_enqueue.rs.
async fn mark_open_delivered(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Uuid)]
		id: Uuid,
	}
	let row: Row =
		sql_query("SELECT id FROM slack_outbox WHERE incident_id = $1 AND kind = $2 LIMIT 1")
			.bind::<sql_types::Uuid, _>(incident_id)
			.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
			.get_result(conn)
			.await
			.expect("pending open row exists");
	SlackOutbox::mark_delivered(conn, row.id, "ok")
		.await
		.expect("mark delivered");
}

/// Backdate a lingering incident's `closing_at` past any realistic linger
/// window, then run the sweep — the test-speed way to let the close-side
/// grace elapse.
async fn expire_linger(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) {
	sql_query("UPDATE incidents SET closing_at = closing_at - INTERVAL '1 hour' WHERE id = $1")
		.bind::<sql_types::Uuid, _>(incident_id)
		.execute(conn)
		.await
		.expect("expire linger");
	database::issues::sweep_lingering_incidents(conn)
		.await
		.expect("linger sweep");
}

async fn count_resolve_rows(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) -> i64 {
	#[derive(QueryableByName)]
	struct Count {
		#[diesel(sql_type = sql_types::BigInt)]
		count: i64,
	}
	let row: Count = sql_query(
		"SELECT COUNT(*) AS count FROM slack_outbox \
		 WHERE incident_id = $1 AND kind = $2",
	)
	.bind::<sql_types::Uuid, _>(incident_id)
	.bind::<sql_types::Text, _>(KIND_INCIDENT_RESOLVE)
	.get_result(conn)
	.await
	.expect("count");
	row.count
}

/// Scenario: error issue opens an incident, warning issue joins (because
/// the group already has an open incident), error issue resolves. Under
/// the old logic the warning would have held the incident open
/// indefinitely; under the new logic the incident closes because no
/// remaining contributor is at severity ≥ error.
#[tokio::test(flavor = "multi_thread")]
async fn warning_does_not_hold_incident_open_after_error_resolves() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://stranded-warning.invalid/").await;

		// Error issue opens the incident.
		save_event(
			&mut conn,
			server_id,
			"error-ref",
			CheckResult::Failed,
			false,
			"boom",
		)
		.await;
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("incident opened by error issue");

		// Pretend Slack has heard about the open so the resolve enqueue
		// isn't short-circuited by the flap-suppression cancel-pending path.
		mark_open_delivered(&mut conn, incident.id).await;

		// Warning issue arrives — joins because the group has an open incident.
		save_event(
			&mut conn,
			server_id,
			"warning-ref",
			CheckResult::Warning,
			false,
			"noise",
		)
		.await;

		// Sanity: incident still open with both attached.
		assert!(
			!Incident::list_for_server(&mut conn, server_id, false, 10)
				.await
				.expect("list")
				.is_empty(),
			"incident is still open while error contributor is alive"
		);

		// Resolve the error issue (active=false).
		save_event(
			&mut conn,
			server_id,
			"error-ref",
			CheckResult::Passed,
			false,
			"recovered",
		)
		.await;

		// The incident lingers rather than closing on the spot — the still-
		// active warning must not hold it past the window, so once the
		// linger elapses the sweep closes it despite the warning.
		let lingering = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list");
		assert_eq!(
			lingering.len(),
			1,
			"incident lingers after the failure recovers"
		);
		expire_linger(&mut conn, incident.id).await;
		let still_open = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list");
		assert!(
			still_open.is_empty(),
			"incident must close once no failure contributor is alive, got: {still_open:?}",
		);

		// One Slack resolve enqueued (not two — the guard against re-close fires).
		assert_eq!(
			count_resolve_rows(&mut conn, incident.id).await,
			1,
			"exactly one slack_resolve row at this point"
		);
	})
	.await
}

/// Sibling test: two error contributors, resolving only one keeps the
/// incident open. Guards against an overzealous filter that would close
/// when any single error issue leaves.
#[tokio::test(flavor = "multi_thread")]
async fn incident_stays_open_while_a_second_error_contributor_is_alive() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://two-errors.invalid/").await;

		save_event(
			&mut conn,
			server_id,
			"error-a",
			CheckResult::Failed,
			false,
			"a",
		)
		.await;
		save_event(
			&mut conn,
			server_id,
			"error-b",
			CheckResult::Failed,
			false,
			"b",
		)
		.await;

		// Resolve only one.
		save_event(
			&mut conn,
			server_id,
			"error-a",
			CheckResult::Passed,
			false,
			"recovered",
		)
		.await;

		let still_open = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list");
		assert_eq!(
			still_open.len(),
			1,
			"incident stays open with second error contributor alive"
		);
	})
	.await
}

/// Edge case: warning leaves an already-closed incident. The (true, _, true)
/// arm fires for the warning issue's `active=false` event, but the
/// closed_at-guard on the close UPDATE prevents a second Slack resolve.
#[tokio::test(flavor = "multi_thread")]
async fn stranded_warning_resolve_does_not_re_enqueue_slack() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://double-close.invalid/").await;

		save_event(
			&mut conn,
			server_id,
			"error-ref",
			CheckResult::Failed,
			false,
			"boom",
		)
		.await;
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list")
			.into_iter()
			.next()
			.expect("opened");
		mark_open_delivered(&mut conn, incident.id).await;
		save_event(
			&mut conn,
			server_id,
			"warning-ref",
			CheckResult::Warning,
			false,
			"noise",
		)
		.await;

		// Error resolves → incident lingers, then the sweep closes it
		// (one slack resolve).
		save_event(
			&mut conn,
			server_id,
			"error-ref",
			CheckResult::Passed,
			false,
			"recovered",
		)
		.await;
		expire_linger(&mut conn, incident.id).await;
		assert_eq!(count_resolve_rows(&mut conn, incident.id).await, 1);

		// Warning eventually resolves too. Incident is already closed; this
		// must NOT enqueue another slack_resolve.
		save_event(
			&mut conn,
			server_id,
			"warning-ref",
			CheckResult::Passed,
			false,
			"settled",
		)
		.await;
		assert_eq!(
			count_resolve_rows(&mut conn, incident.id).await,
			1,
			"warning's later resolve must not enqueue a second slack_resolve"
		);
	})
	.await
}
