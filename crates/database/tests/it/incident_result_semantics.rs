//! Result semantics for the incident workflow:
//!
//! - A **skipped** effective result never participates in incidents —
//!   neither joining a new one nor staying attached once a contributor
//!   grades to skipped after the fact.
//! - An **escalating failure** opens the incident (or joins it) without
//!   sitting in the per-group `slack_open_delay` holding window: the
//!   outbox row's `deliver_after` is pulled forward to NOW().

use commons_types::issue::ResolvedReason;
use commons_types::status::CheckResult;
use database::issues::{Incident, NewEvent};
use database::slack_outbox::{KIND_INCIDENT_OPEN, SlackOutbox};
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
		sql_query("WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $2) RETURNING id) INSERT INTO applications (type, host, group_id, rank, machine_id) SELECT 'tamanu-central', $1, $2, 'production', m.id FROM m RETURNING id")
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

#[derive(QueryableByName)]
struct OpenLinkCount {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}
async fn open_link_count(conn: &mut diesel_async::AsyncPgConnection, issue_ref: &str) -> i64 {
	let row: OpenLinkCount = sql_query(
		"SELECT COUNT(*) AS n FROM incident_issues ii \
		 JOIN issues i ON i.id = ii.issue_id \
		 WHERE i.ref = $1 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Text, _>(issue_ref)
	.get_result(conn)
	.await
	.expect("count");
	row.n
}

/// Pending-incident_open row summarised so we can assert on its delivery
/// timing without round-tripping the underlying timestamps (which need a
/// jiff_diesel wrapper that isn't worth pulling in for tests).
#[derive(QueryableByName, Debug)]
struct OpenInfo {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	/// Seconds until (positive) or since (negative) `deliver_after`.
	#[diesel(sql_type = sql_types::Double)]
	delay_secs: f64,
}

async fn pending_open(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) -> OpenInfo {
	sql_query(
		"SELECT id, \
				EXTRACT(EPOCH FROM (deliver_after - NOW()))::float8 AS delay_secs \
		 FROM slack_outbox WHERE incident_id = $1 AND kind = $2 LIMIT 1",
	)
	.bind::<sql_types::Uuid, _>(incident_id)
	.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
	.get_result(conn)
	.await
	.expect("pending open row")
}

// ── Debug ────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn debug_issue_does_not_join_open_incidents() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://debug-shy.invalid/").await;
		// Open an incident with an Error-severity issue.
		save_event(
			&mut conn,
			server_id,
			"real-error",
			CheckResult::Failed,
			false,
			"boom",
		)
		.await;
		assert!(
			!Incident::list_for_server(&mut conn, server_id, false, 10)
				.await
				.expect("list")
				.is_empty(),
			"incident open"
		);
		// File a Debug issue on the same server.
		save_event(
			&mut conn,
			server_id,
			"debug-noise",
			CheckResult::Skipped,
			false,
			"low signal",
		)
		.await;
		// The Debug issue's link should NOT be in incident_issues.
		assert_eq!(
			open_link_count(&mut conn, "debug-noise").await,
			0,
			"debug never joins"
		);
		// The Error contributor is still attached.
		assert_eq!(open_link_count(&mut conn, "real-error").await, 1);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn issue_downgraded_to_debug_leaves_incident_on_next_evaluation() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id =
			insert_grouped_server(&mut conn, "http://downgrade-to-debug.invalid/").await;
		// Joins at Warning while another Error issue holds the incident open.
		save_event(
			&mut conn,
			server_id,
			"main-error",
			CheckResult::Failed,
			false,
			"boom",
		)
		.await;
		save_event(
			&mut conn,
			server_id,
			"noisy",
			CheckResult::Warning,
			false,
			"noise",
		)
		.await;
		assert_eq!(open_link_count(&mut conn, "noisy").await, 1);
		// Now an operator (or rule edit) drops it to Debug. The next event push
		// runs re_evaluate_incident_membership, which should detach it.
		save_event(
			&mut conn,
			server_id,
			"noisy",
			CheckResult::Skipped,
			false,
			"demoted",
		)
		.await;
		assert_eq!(
			open_link_count(&mut conn, "noisy").await,
			0,
			"debug-downgraded issue must leave the incident"
		);
	})
	.await
}

// ── Critical ─────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn critical_open_sets_deliver_after_to_now() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://critical-now.invalid/").await;
		save_event(
			&mut conn,
			server_id,
			"crit",
			CheckResult::Failed,
			true,
			"red alert",
		)
		.await;
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list")
			.into_iter()
			.next()
			.expect("incident");
		let open = pending_open(&mut conn, incident.id).await;
		assert!(
			open.delay_secs.abs() <= 5.0,
			"Critical bypasses the holding window — deliver_after should be ~now, delay={}s",
			open.delay_secs,
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn non_critical_open_still_honours_holding_window() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://error-delayed.invalid/").await;
		save_event(
			&mut conn,
			server_id,
			"boom",
			CheckResult::Failed,
			false,
			"less urgent",
		)
		.await;
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list")
			.into_iter()
			.next()
			.expect("incident");
		let open = pending_open(&mut conn, incident.id).await;
		// Default group `slack_open_delay` is 3 minutes; should be well above 30s.
		assert!(
			open.delay_secs > 30.0,
			"Error severity sits in the holding window; got delay={}s",
			open.delay_secs,
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn critical_joining_existing_open_accelerates_pending_delivery() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id =
			insert_grouped_server(&mut conn, "http://crit-join-accelerate.invalid/").await;
		// Open the incident with a non-Critical issue → deliver_after is in the future.
		save_event(
			&mut conn,
			server_id,
			"warmup",
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
			.expect("incident");
		let initial = pending_open(&mut conn, incident.id).await;
		assert!(
			initial.delay_secs > 30.0,
			"preconditions: delay > 30s, got {}s",
			initial.delay_secs,
		);

		// A Critical-severity issue now joins the same group's open incident.
		save_event(
			&mut conn,
			server_id,
			"crit",
			CheckResult::Failed,
			true,
			"red alert",
		)
		.await;

		// The existing pending open row was pulled forward to ~now.
		let after = pending_open(&mut conn, incident.id).await;
		assert!(
			after.delay_secs.abs() <= 5.0,
			"Critical join pulls deliver_after forward; delay={}s",
			after.delay_secs,
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn critical_join_after_delivered_open_fires_escalation_open() {
	// Once the original "incident opened" Slack message has shipped, a
	// Critical contributor joining is treated as an escalation: a fresh
	// `incident_open` row is enqueued at Critical-severity timing so
	// operators hear the change. Incident.escalated_at is stamped so we
	// don't re-fire on every subsequent Critical join.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id =
			insert_grouped_server(&mut conn, "http://crit-after-delivered.invalid/").await;
		save_event(
			&mut conn,
			server_id,
			"warmup",
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
			.expect("incident");
		// Simulate the drainer having shipped the open already.
		let pending = pending_open(&mut conn, incident.id).await;
		SlackOutbox::mark_delivered(&mut conn, pending.id, "ok")
			.await
			.expect("mark delivered");
		assert!(
			incident.escalated_at.is_none(),
			"precondition: not yet escalated"
		);

		// A Critical issue joins → escalation fires.
		save_event(
			&mut conn,
			server_id,
			"crit",
			CheckResult::Failed,
			true,
			"red alert",
		)
		.await;

		// Two open rows now: the original (delivered) and the escalation
		// (pending, with delay ~0 because Critical bypasses the holding
		// window).
		#[derive(QueryableByName)]
		struct OpenRow {
			#[diesel(sql_type = sql_types::Double)]
			delay_secs: f64,
			#[diesel(sql_type = sql_types::Bool)]
			is_delivered: bool,
		}
		let rows: Vec<OpenRow> = sql_query(
			"SELECT EXTRACT(EPOCH FROM (deliver_after - NOW()))::float8 AS delay_secs, \
					delivered_at IS NOT NULL AS is_delivered \
			 FROM slack_outbox WHERE incident_id = $1 AND kind = $2 \
			 ORDER BY created_at",
		)
		.bind::<sql_types::Uuid, _>(incident.id)
		.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
		.get_results(&mut conn)
		.await
		.expect("rows");
		assert_eq!(rows.len(), 2, "two open rows: original + escalation");
		assert!(rows[0].is_delivered, "original is delivered");
		assert!(!rows[1].is_delivered, "escalation is freshly enqueued");
		assert!(
			rows[1].delay_secs.abs() <= 5.0,
			"escalation deliver_after is ~now (Critical bypasses delay); got {}s",
			rows[1].delay_secs,
		);

		// incident.escalated_at is now set.
		let refreshed = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list")
			.into_iter()
			.next()
			.expect("incident");
		assert!(
			refreshed.escalated_at.is_some(),
			"escalated_at recorded after first Critical escalation"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn repeated_critical_joins_do_not_re_fire_escalation() {
	// escalated_at is the latch: the first Critical-after-delivered-open
	// fires a fresh "incident opened" message, and every subsequent
	// Critical contributor is silent. Otherwise a flapping check could
	// repeatedly re-page operators long after the incident is well-known.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id =
			insert_grouped_server(&mut conn, "http://crit-no-double-escalate.invalid/").await;
		save_event(
			&mut conn,
			server_id,
			"warmup",
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
			.expect("incident");
		let pending = pending_open(&mut conn, incident.id).await;
		SlackOutbox::mark_delivered(&mut conn, pending.id, "ok")
			.await
			.expect("mark delivered");

		// First Critical → escalation fires.
		save_event(
			&mut conn,
			server_id,
			"crit-1",
			CheckResult::Failed,
			true,
			"red alert",
		)
		.await;
		// Second Critical → no new outbox row.
		save_event(
			&mut conn,
			server_id,
			"crit-2",
			CheckResult::Failed,
			true,
			"also red",
		)
		.await;

		#[derive(QueryableByName)]
		struct Count {
			#[diesel(sql_type = sql_types::BigInt)]
			n: i64,
		}
		let count: Count = sql_query(
			"SELECT COUNT(*) AS n FROM slack_outbox \
			 WHERE incident_id = $1 AND kind = $2",
		)
		.bind::<sql_types::Uuid, _>(incident.id)
		.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
		.get_result(&mut conn)
		.await
		.expect("count");
		assert_eq!(
			count.n, 2,
			"original + one escalation; second Critical is silent"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn debug_filing_still_records_the_issue_row() {
	// Audit-trail guarantee: Debug doesn't participate in incidents, but
	// the issue row itself is still written so the per-server / global
	// issues lists can show low-severity context.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn, "http://debug-audit.invalid/").await;
		save_event(
			&mut conn,
			server_id,
			"logspam",
			CheckResult::Skipped,
			false,
			"verbose",
		)
		.await;
		let issue: RowId =
			sql_query("SELECT id FROM issues WHERE ref = 'logspam' AND application_id = $1")
				.bind::<sql_types::Uuid, _>(server_id)
				.get_result(&mut conn)
				.await
				.expect("debug issue row exists despite no incident participation");
		// Belt-and-suspenders: the resolution path on a debug issue must not panic.
		database::issues::Issue::resolve(&mut conn, issue.id, "ops", ResolvedReason::Fixed)
			.await
			.expect("resolve a debug issue");
	})
	.await
}
