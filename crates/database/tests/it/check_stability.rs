//! Stability records (CHK "Stability"): every stamped filing feeds the
//! state's fixed-size summary — lifetime counters, the healthy↔degraded
//! transition ring, and the hour-of-week duty profile — from *observed*
//! results, with skipped observations carrying no signal.

use commons_types::status::CheckResult;
use database::issues::NewEvent;
use database::stability::{
	CheckStability, DUTY_BUCKET_CAP, TRANSITION_RING_CAP, Transition, derive_stats,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_grouped_server(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let group: RowId =
		sql_query("INSERT INTO server_groups (name) VALUES ('stability-group') RETURNING id")
			.get_result(conn)
			.await
			.expect("group");
	let machine: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Uuid, _>(group.id)
			.get_result(conn)
			.await
			.expect("machine");
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, group_id, machine_id) \
		 VALUES ('tamanu-central', 'http://stability.invalid/', $1, $2) RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group.id)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("server");
	row.id
}

/// File one check observation; `observed` and `effective` differ where a
/// test wants to prove stability follows the observed side.
async fn observe(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	observed: CheckResult,
	effective: CheckResult,
) -> Uuid {
	let active = matches!(
		effective,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let stamp = database::issues::CheckStateStamp {
		check: "wobbly".into(),
		observed,
		effective,
		escalates: false,
		detail: None,
		title: None,
		instanced: None,
	};
	NewEvent {
		source: "test".into(),
		r#ref: "wobbly".into(),
		description: None,
		message: "obs".into(),
		active: Some(active),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), false)
	.await
	.expect("file observation")
	.id
}

async fn stability_row(
	conn: &mut diesel_async::AsyncPgConnection,
	issue_id: Uuid,
) -> Option<CheckStability> {
	CheckStability::for_issue_ids(conn, &[issue_id])
		.await
		.expect("load stability")
		.remove(&issue_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn observations_feed_counters_ring_and_duty_profile() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;

		// First sight, degraded: one observation, one ring entry, one
		// degraded bucket hit.
		let issue_id = observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Failed,
		)
		.await;
		let row = stability_row(&mut conn, issue_id)
			.await
			.expect("row created");
		assert_eq!(row.observations, 1);
		assert_eq!(row.degraded_observations, 1);
		assert_eq!(row.last_observed_degraded, Some(true));
		let ring = row.transition_ring();
		assert_eq!(ring.len(), 1);
		assert!(ring[0].degraded);
		let hits: i64 = row.duty_profile().iter().map(|b| b.observations).sum();
		assert_eq!(hits, 1);

		// Same result again: counters move, the ring does not.
		observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Failed,
		)
		.await;
		let row = stability_row(&mut conn, issue_id).await.expect("row");
		assert_eq!(row.observations, 2);
		assert_eq!(row.degraded_observations, 2);
		assert_eq!(row.transition_ring().len(), 1);

		// Recovery: a healthy transition lands.
		observe(
			&mut conn,
			server_id,
			CheckResult::Passed,
			CheckResult::Passed,
		)
		.await;
		let row = stability_row(&mut conn, issue_id).await.expect("row");
		assert_eq!(row.observations, 3);
		assert_eq!(row.degraded_observations, 2);
		assert_eq!(row.last_observed_degraded, Some(false));
		let ring = row.transition_ring();
		assert_eq!(ring.len(), 2);
		assert!(!ring[1].degraded);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn skipped_observations_carry_no_signal() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;
		let issue_id = observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Failed,
		)
		.await;

		observe(
			&mut conn,
			server_id,
			CheckResult::Skipped,
			CheckResult::Skipped,
		)
		.await;
		let row = stability_row(&mut conn, issue_id).await.expect("row");
		assert_eq!(row.observations, 1, "skipped is not an observation");
		assert_eq!(row.last_observed_degraded, Some(true));
		assert_eq!(row.transition_ring().len(), 1);

		// Coming back degraded after a skip is not a transition either:
		// the last signal was already degraded.
		observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Failed,
		)
		.await;
		let row = stability_row(&mut conn, issue_id).await.expect("row");
		assert_eq!(row.observations, 2);
		assert_eq!(row.transition_ring().len(), 1);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn stability_follows_observed_not_effective() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;

		// Policy grades the failure down to passed (say, a silence-like
		// transform): the stability record still sees a degraded
		// observation, because it must stay untouched by policy.
		let issue_id = observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Passed,
		)
		.await;
		let row = stability_row(&mut conn, issue_id).await.expect("row");
		assert_eq!(row.degraded_observations, 1);
		assert_eq!(row.last_observed_degraded, Some(true));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn transition_ring_is_bounded() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;
		let mut issue_id = None;
		for i in 0..(TRANSITION_RING_CAP + 8) {
			let result = if i % 2 == 0 {
				CheckResult::Failed
			} else {
				CheckResult::Passed
			};
			let id = observe(&mut conn, server_id, result, result).await;
			issue_id.get_or_insert(id);
		}
		let row = stability_row(&mut conn, issue_id.unwrap())
			.await
			.expect("row");
		let ring = row.transition_ring();
		assert_eq!(ring.len(), TRANSITION_RING_CAP, "ring keeps only the cap");
		// Newest entries survive: the last one matches the last filing.
		let parity_last = (TRANSITION_RING_CAP + 8 - 1) % 2 == 0;
		assert_eq!(ring.last().unwrap().degraded, parity_last);
		assert_eq!(row.observations as usize, TRANSITION_RING_CAP + 8);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn duty_buckets_halve_at_the_cap() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_grouped_server(&mut conn).await;
		let issue_id = observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Failed,
		)
		.await;

		// Pre-load every bucket at the cap; the next observation must
		// halve the bucket it lands in instead of growing it.
		let full: Vec<(i64, i64)> = (0..168)
			.map(|_| (DUTY_BUCKET_CAP, DUTY_BUCKET_CAP))
			.collect();
		sql_query("UPDATE check_stability SET duty_cycle = $1 WHERE issue_id = $2")
			.bind::<sql_types::Jsonb, _>(serde_json::to_value(&full).unwrap())
			.bind::<sql_types::Uuid, _>(issue_id)
			.execute(&mut conn)
			.await
			.expect("preload duty cycle");

		observe(
			&mut conn,
			server_id,
			CheckResult::Failed,
			CheckResult::Failed,
		)
		.await;
		let row = stability_row(&mut conn, issue_id).await.expect("row");
		let duty = row.duty_profile();
		let halved = duty
			.iter()
			.filter(|b| b.observations == (DUTY_BUCKET_CAP + 1) / 2)
			.count();
		assert_eq!(halved, 1, "exactly the bucket that was hit halved");
		assert_eq!(
			duty.iter()
				.filter(|b| b.observations == DUTY_BUCKET_CAP)
				.count(),
			167,
			"the other buckets are untouched"
		);
	})
	.await
}

#[test]
fn derived_stats_report_flips_and_typical_durations() {
	// Pure derivation; the ring is fabricated. Timeline (minutes ago):
	// degraded 100→70 (30m run), healthy 70→40 (30m gap), degraded
	// 40→30 (10m run), healthy since 30.
	let now = Timestamp::now();
	let at = |mins_ago: i64| now - SignedDuration::from_mins(mins_ago);
	let ring = vec![
		Transition {
			at: at(100),
			degraded: true,
		},
		Transition {
			at: at(70),
			degraded: false,
		},
		Transition {
			at: at(40),
			degraded: true,
		},
		Transition {
			at: at(30),
			degraded: false,
		},
	];
	let stats = derive_stats(&ring, now);
	assert_eq!(stats.flips_24h, 4);
	assert_eq!(stats.flips_7d, 4);
	assert_eq!(stats.ring_covers_from, Some(at(100)));
	// Completed degraded runs: 30m and 10m → median picks the upper of
	// the two-element list (30m).
	assert_eq!(stats.typical_degraded_run_secs, Some(30 * 60));
	// One completed healthy gap: 30m.
	assert_eq!(stats.typical_healthy_gap_secs, Some(30 * 60));
}

/// The startup backfill replays status history into stability rows,
/// one server per transaction, and marks itself done so it never scans
/// twice.
///
/// TODO(backfill-removal): goes with `backfill_from_statuses` once the
/// backfill has run everywhere.
#[tokio::test(flavor = "multi_thread")]
async fn backfill_replays_status_history() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		use database::stability::backfill_from_statuses;

		let server_id = insert_grouped_server(&mut conn).await;

		// Three pushes: red two hours ago, red one hour ago, green now —
		// plus a 'skipped' entry and an unrelated check to be ignored.
		for (hours_ago, result) in [(2i32, "failed"), (1, "failed"), (0, "passed")] {
			sql_query(
				"INSERT INTO statuses (server_id, source, healthy, health, extra, created_at) \
				 VALUES ($1, 'alertd', true, $2::jsonb, '{}'::jsonb, NOW() - make_interval(hours => $3))",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Jsonb, _>(serde_json::json!([
				{ "check": "db", "result": result },
				{ "check": "cron", "result": "skipped" },
			]))
			.bind::<sql_types::Integer, _>(hours_ago)
			.execute(&mut conn)
			.await
			.expect("seed status");
		}
		// The check-state row the backfill attaches to.
		let issue: RowId = sql_query(
			"INSERT INTO issues (application_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen) \
			 VALUES ($1, 'alertd', 'health/db', 'db', 'passed', 'passed', 'm', false, NOW(), NOW()) RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.get_result(&mut conn)
		.await
		.expect("seed issue");

		let backfilled = backfill_from_statuses(&mut conn)
			.await
			.expect("backfill runs");
		assert_eq!(backfilled, Some(1), "one state backfilled");

		let row = stability_row(&mut conn, issue.id).await.expect("backfilled");
		assert_eq!(row.observations, 3);
		assert_eq!(row.degraded_observations, 2);
		assert_eq!(row.last_observed_degraded, Some(false));
		let ring = row.transition_ring();
		assert_eq!(ring.len(), 2, "red at first sight, green at the end: {ring:?}");
		assert!(ring[0].degraded && !ring[1].degraded);
		let duty = row.duty_profile();
		assert_eq!(duty.len(), 168);
		assert_eq!(duty.iter().map(|b| b.observations).sum::<i64>(), 3);
		assert_eq!(duty.iter().map(|b| b.degraded).sum::<i64>(), 2);

		// The completion marker gates re-runs: a second call is a no-op
		// without rescanning anything.
		let rerun = backfill_from_statuses(&mut conn)
			.await
			.expect("backfill reruns");
		assert_eq!(rerun, None, "marker short-circuits the second run");
		let again = stability_row(&mut conn, issue.id).await.expect("row");
		assert_eq!(again.observations, 3);

		// A crash before the marker was written means a re-run over rows
		// that already exist (from the partial pass, or from live
		// recording that started meanwhile): they are left untouched.
		sql_query("DELETE FROM check_stability_backfill")
			.execute(&mut conn)
			.await
			.expect("clear marker");
		let rerun = backfill_from_statuses(&mut conn)
			.await
			.expect("backfill after partial run");
		assert_eq!(rerun, Some(0), "existing rows are not overwritten");
		let after = stability_row(&mut conn, issue.id).await.expect("row");
		assert_eq!(after.observations, 3);
	})
	.await
}
