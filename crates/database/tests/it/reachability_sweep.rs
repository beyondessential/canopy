use commons_types::namespace::Namespace;
use commons_types::server::app_type::ApplicationType;
use commons_types::source::{IngestMode, ReachabilityMode};
use commons_types::status::CheckResult;
use database::{
	check_policies::CheckPolicy,
	issues::Issue,
	source_policies::SourcePolicy,
	statuses::{CANOPY_SOURCE, REACHABILITY_REF, Status},
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

/// Insert a monitored server with the given `alert_when_down_for` threshold
/// (seconds, stored as a PostgreSQL `INTERVAL`).
async fn insert_server(
	conn: &mut diesel_async::AsyncPgConnection,
	host: &str,
	alert_when_down_for_secs: i64,
) -> Uuid {
	insert_server_full(conn, host, alert_when_down_for_secs, true).await
}

async fn insert_server_full(
	conn: &mut diesel_async::AsyncPgConnection,
	host: &str,
	alert_when_down_for_secs: i64,
	is_monitored: bool,
) -> Uuid {
	let machine: RowId = sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	// A reporting box, so each test below varies only the application's own
	// freshness: the two grains are graded separately, and a machine left
	// silent would file its own unreachability alongside whatever the test is
	// actually about.
	insert_machine_detail_at(conn, machine.id, 0).await;
	let row: RowId = sql_query(
		r#"
			INSERT INTO applications (type, host, alert_when_down_for, is_monitored, machine_id)
			VALUES ('tamanu-central', $1, ($2 || ' seconds')::INTERVAL, $3, $4)
			RETURNING id
		"#,
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Text, _>(alert_when_down_for_secs.to_string())
	.bind::<sql_types::Bool, _>(is_monitored)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

/// Insert a server in a group, so incident membership is actually on the
/// table: the incident flow is skipped outright for ungrouped applications, and
/// a gate assertion against one would pass for the wrong reason.
async fn insert_grouped_server(
	conn: &mut diesel_async::AsyncPgConnection,
	host: &str,
	alert_when_down_for_secs: i64,
	is_monitored: bool,
) -> Uuid {
	let group: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('sweep') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert group");
	let machine: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Uuid, _>(group.id)
			.get_result(conn)
			.await
			.expect("insert machine");
	insert_machine_detail_at(conn, machine.id, 0).await;
	let row: RowId = sql_query(
		r#"
			INSERT INTO applications (type, host, alert_when_down_for, is_monitored, group_id, machine_id)
			VALUES ('tamanu-central', $1, ($2 || ' seconds')::INTERVAL, $3, $4, $5)
			RETURNING id
		"#,
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Text, _>(alert_when_down_for_secs.to_string())
	.bind::<sql_types::Bool, _>(is_monitored)
	.bind::<sql_types::Uuid, _>(group.id)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

/// Count open (`left_at IS NULL`) incident links for a server's reachability
/// check.
async fn open_incident_links(conn: &mut diesel_async::AsyncPgConnection, server_id: Uuid) -> i64 {
	#[derive(QueryableByName)]
	struct CountRow {
		#[diesel(sql_type = sql_types::BigInt)]
		n: i64,
	}
	sql_query(
		"SELECT COUNT(*) AS n FROM incident_issues ii \
		 JOIN issues i ON i.id = ii.issue_id \
		 WHERE i.application_id = $1 AND i.\"ref\" = $2 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(REACHABILITY_REF)
	.get_result::<CountRow>(conn)
	.await
	.expect("count incident links")
	.n
}

async fn insert_status_at(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	minutes_ago: i32,
) {
	sql_query(
		r#"
			INSERT INTO statuses (server_id, machine_id, created_at, extra)
			SELECT a.id, a.machine_id, NOW() - ($2 || ' minutes')::INTERVAL, '{}'::jsonb
			FROM applications a WHERE a.id = $1
		"#,
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(minutes_ago.to_string())
	.execute(conn)
	.await
	.expect("insert status");
}

/// Record a source's report against the current-state projection, dated
/// `days_ago`, without a matching status row inside the lookback window.
async fn insert_reported_detail_days_ago(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	source: &str,
	days_ago: i32,
) {
	sql_query(
		r#"
			INSERT INTO application_reported_detail (application_id, source, extra, reported_at)
			VALUES ($1, $2, '{}'::jsonb, NOW() - ($3 || ' days')::INTERVAL)
		"#,
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(source)
	.bind::<sql_types::Text, _>(days_ago.to_string())
	.execute(conn)
	.await
	.expect("insert reported detail");
}

/// A second workload on a box that already has one.
async fn insert_application_on(
	conn: &mut diesel_async::AsyncPgConnection,
	machine_id: Uuid,
	host: &str,
	alert_when_down_for_secs: i64,
) -> Uuid {
	let row: RowId = sql_query(
		r#"
			INSERT INTO applications (type, host, alert_when_down_for, machine_id)
			VALUES ('tamanu-facility', $1, ($2 || ' seconds')::INTERVAL, $3)
			RETURNING id
		"#,
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Text, _>(alert_when_down_for_secs.to_string())
	.bind::<sql_types::Uuid, _>(machine_id)
	.get_result(conn)
	.await
	.expect("insert application");
	row.id
}

/// Record the box itself as having reported `minutes_ago`, which is what a
/// push carrying machine detail leaves behind.
async fn insert_machine_detail_at(
	conn: &mut diesel_async::AsyncPgConnection,
	machine_id: Uuid,
	minutes_ago: i64,
) {
	sql_query(
		r#"
			INSERT INTO machine_reported_detail (machine_id, source, extra, reported_at)
			VALUES ($1, 'alertd', '{}'::jsonb, NOW() - ($2 || ' minutes')::INTERVAL)
			ON CONFLICT (machine_id, source) DO UPDATE SET reported_at = EXCLUDED.reported_at
		"#,
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(minutes_ago.to_string())
	.execute(conn)
	.await
	.expect("insert machine detail");
}

/// The machine an application runs on.
async fn machine_of(conn: &mut diesel_async::AsyncPgConnection, server_id: Uuid) -> Uuid {
	let row: RowId = sql_query("SELECT machine_id AS id FROM applications WHERE id = $1")
		.bind::<sql_types::Uuid, _>(server_id)
		.get_result(conn)
		.await
		.expect("read machine");
	row.id
}

async fn issue_for_machine(
	conn: &mut diesel_async::AsyncPgConnection,
	machine_id: Uuid,
) -> Option<Issue> {
	Issue::list_by_source_ref_for_machines(conn, CANOPY_SOURCE, REACHABILITY_REF, &[machine_id])
		.await
		.expect("list machine issues")
		.into_iter()
		.next()
}

async fn issue_for(conn: &mut diesel_async::AsyncPgConnection, server_id: Uuid) -> Option<Issue> {
	Issue::list_by_source_ref(conn, CANOPY_SOURCE, REACHABILITY_REF, &[server_id])
		.await
		.expect("list issues")
		.into_iter()
		.next()
}

/// Seed check state as if `source` reported `check` on the server
/// `minutes_ago` — what ingestion stamps on every report, and what the
/// per-source staleness arm reads freshness from.
async fn insert_check_state(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	source: &str,
	check: &str,
	minutes_ago: i32,
) {
	// Go through ingestion's own upsert rather than reproducing it: a
	// check-state only keeps its source "expected" for reachability if a live
	// catalog row backs it, and the row has to land in the namespace ingest
	// would have put it in. Every server here is a tamanu-central.
	CheckPolicy::upsert_default(
		conn,
		source,
		&Namespace::for_application(source, check, &ApplicationType::TamanuCentral),
		check,
	)
	.await
	.expect("catalog the seeded check");
	sql_query(
		r#"
			INSERT INTO issues
			(application_id, source, ref, check_name, observed_result, effective_result,
			 message, active, first_seen, last_seen)
			VALUES ($1, $2, 'health/' || $3, $3, 'passed', 'passed',
			 'seeded', false,
			 NOW() - ($4 || ' minutes')::INTERVAL, NOW() - ($4 || ' minutes')::INTERVAL)
		"#,
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(source)
	.bind::<sql_types::Text, _>(check)
	.bind::<sql_types::Text, _>(minutes_ago.to_string())
	.execute(conn)
	.await
	.expect("insert check state");
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_files_error_when_threshold_crossed() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// 10-min threshold, status 15 minutes old → cross.
		let id = insert_server(&mut conn, "http://down.invalid/", 600).await;
		insert_status_at(&mut conn, id, 15).await;
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("issue exists");
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert!(issue.active);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_skips_when_below_threshold() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// 10-min threshold, status 5 minutes old → still fresh.
		let id = insert_server(&mut conn, "http://fresh.invalid/", 600).await;
		insert_status_at(&mut conn, id, 5).await;
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for(&mut conn, id).await.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_files_when_no_status_ever() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// No status row at all → infinite downtime → always crosses threshold.
		let id = insert_server(&mut conn, "http://gone.invalid/", 600).await;
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("issue exists");
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert!(issue.active);
		assert!(
			issue.message.contains("has never reported"),
			"expected a never-reported message, got: {}",
			issue.message
		);
		assert!(issue.message.contains("(threshold 10m)"));
		assert!(!issue.message.contains("106751991167300d"));
		// An application nobody has named reads as its type, never its id.
		// spec: FLT#naming
		assert!(
			issue.message.starts_with("Application Tamanu central "),
			"got: {}",
			issue.message
		);
		assert!(!issue.message.contains(&id.to_string()));
	})
	.await
}

/// The backstop read status history, which is capped at the grace lookback, so
/// an application quiet for longer than the window looked identical to one that
/// had never reported: it filed "has never reported" with no elapsed time,
/// against applications that had reported for years.
#[tokio::test(flavor = "multi_thread")]
async fn sweep_distinguishes_a_long_silence_from_never_reporting() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://quiet.invalid/", 600).await;
		// Well past the lookback the windowed status read is capped at, so
		// nothing about this application is visible in status history.
		insert_reported_detail_days_ago(&mut conn, id, "alertd", 90).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("issue exists");
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert!(
			issue.message.contains("has not reported for"),
			"expected an elapsed-time message, got: {}",
			issue.message
		);
		assert!(
			!issue.message.contains("has never reported"),
			"an application that reported 90 days ago has reported: {}",
			issue.message
		);
		let detail = issue.detail.clone().expect("issue carries detail");
		assert!(
			detail["elapsed_secs"].as_i64().unwrap_or_default() > 0,
			"the event carries how long the silence has run: {detail}",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_files_for_unmonitored_server_without_alerting() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// is_monitored = false doesn't change what's true about the server:
		// its reachability is determined and recorded like everyone else's,
		// so an operator can see it's away. The monitoring gate is what
		// keeps the filing out of incidents.
		let id = insert_grouped_server(&mut conn, "http://silenced.invalid/", 600, false).await;
		insert_status_at(&mut conn, id, 120).await;
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("issue exists");
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert!(issue.active);
		assert_eq!(
			open_incident_links(&mut conn, id).await,
			0,
			"an unmonitored server's reachability must not join an incident",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_opens_an_incident_for_a_monitored_server() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// The counterpart to the unmonitored case: same filing, same group,
		// and here it does reach an incident — so that test's zero is the
		// monitoring gate rather than something else swallowing it.
		let id = insert_grouped_server(&mut conn, "http://watched.invalid/", 600, true).await;
		insert_status_at(&mut conn, id, 120).await;
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		assert_eq!(open_incident_links(&mut conn, id).await, 1);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_skips_up_server() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://up.invalid/", 600).await;
		insert_status_at(&mut conn, id, 0).await;
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for(&mut conn, id).await.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_uses_per_server_threshold() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// Flappy server: long threshold (30 min), 15-min-old status → still ok.
		let flappy = insert_server(&mut conn, "http://flappy.invalid/", 1800).await;
		insert_status_at(&mut conn, flappy, 15).await;
		// Critical server: short threshold (60 s), 15-min-old status → crosses.
		let critical = insert_server(&mut conn, "http://critical.invalid/", 60).await;
		insert_status_at(&mut conn, critical, 15).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		assert!(issue_for(&mut conn, flappy).await.is_none());
		assert!(issue_for(&mut conn, critical).await.is_some());
	})
	.await
}

#[derive(QueryableByName)]
struct RowSecs {
	#[diesel(sql_type = sql_types::Float8)]
	seconds: f64,
}

/// Freshly-inserted applications inherit the column default of 10 minutes.
#[tokio::test(flavor = "multi_thread")]
async fn new_servers_default_to_ten_minutes() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let machine: RowId = sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
			.get_result(&mut conn)
			.await
			.expect("insert machine");
		sql_query("INSERT INTO applications (type, host, machine_id) VALUES ('tamanu-central', 'http://new.invalid/', $1)")
			.bind::<sql_types::Uuid, _>(machine.id)
			.execute(&mut conn)
			.await
			.expect("insert default");

		let row: RowSecs = sql_query(
			r#"
				SELECT EXTRACT(EPOCH FROM alert_when_down_for)::float8 AS seconds
				FROM applications
				WHERE host = 'http://new.invalid/'
			"#,
		)
		.get_result(&mut conn)
		.await
		.expect("read default");
		assert_eq!(row.seconds as i64, 600);
	})
	.await
}

/// Non-positive durations are rejected at the DB level by a CHECK
/// constraint — "off" is now `is_monitored = false`, not a zero threshold.
#[tokio::test(flavor = "multi_thread")]
async fn check_constraint_forbids_non_positive_duration() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for bad in ["INTERVAL '-1 second'", "INTERVAL '0'"] {
			let machine: RowId =
				sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
					.get_result(&mut conn)
					.await
					.expect("insert machine");
			let res = sql_query(&format!(
				"INSERT INTO applications (type, host, alert_when_down_for, machine_id) \
				 VALUES ('tamanu-central', 'http://bad.invalid/', {bad}, '{machine_id}')",
				machine_id = machine.id
			))
			.execute(&mut conn)
			.await;
			assert!(res.is_err(), "expected CHECK constraint to reject {bad}",);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_warns_when_an_on_source_is_stale() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// Reachable via a fresh source, but another `on` source has gone
		// quiet (15m > 10m threshold) → reachability warns, naming it.
		let id = insert_server(&mut conn, "http://quiet-source.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 15).await;
		insert_check_state(&mut conn, id, "otheragent", "ping", 2).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("reachability issue");
		assert!(issue.active);
		assert_eq!(issue.observed_result, Some(CheckResult::Warning));
		assert!(issue.message.contains("alertd"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_passes_when_all_sources_fresh() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://fresh-sources.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 2).await;
		insert_check_state(&mut conn, id, "otheragent", "ping", 3).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for(&mut conn, id).await.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_fails_when_every_source_is_stale() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://all-stale.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 20).await;
		insert_check_state(&mut conn, id, "otheragent", "ping", 30).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("reachability issue");
		assert!(issue.active);
		assert_eq!(issue.observed_result, Some(CheckResult::Failed));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_quiet_source_stale_does_not_warn() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://quiet-mode.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 2).await; // fresh, on
		insert_check_state(&mut conn, id, "legacyagent", "beat", 30).await; // stale
		SourcePolicy::set_reachability(&mut conn, "legacyagent", ReachabilityMode::Quiet)
			.await
			.expect("set quiet");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(
			issue_for(&mut conn, id).await.is_none(),
			"a quiet source going stale raises no warning while another source is fresh"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_quiet_only_server_still_unreachable() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://quiet-only.invalid/", 600).await;
		insert_check_state(&mut conn, id, "legacyagent", "beat", 30).await; // stale
		SourcePolicy::set_reachability(&mut conn, "legacyagent", ReachabilityMode::Quiet)
			.await
			.expect("set quiet");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for(&mut conn, id).await.expect("reachability issue");
		assert_eq!(
			issue.observed_result,
			Some(CheckResult::Failed),
			"a quiet source still counts toward unreachable when it's the only one"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_off_source_is_excluded() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// An `off` source is ignored entirely; with a fresh status row and no
		// other counted source, reachability passes.
		let id = insert_server(&mut conn, "http://off-mode.invalid/", 600).await;
		insert_status_at(&mut conn, id, 0).await;
		insert_check_state(&mut conn, id, "offagent", "x", 30).await;
		SourcePolicy::set_reachability(&mut conn, "offagent", ReachabilityMode::Off)
			.await
			.expect("set off");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for(&mut conn, id).await.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_excludes_a_non_allow_source() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// A source whose ingest is `ignore` has no fresh data, so it's
		// excluded from reachability even when stale and mode `on`: only the
		// fresh `alertd` source counts → passed, no warning.
		let id = insert_server(&mut conn, "http://ignored-source.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 2).await; // fresh, on
		insert_check_state(&mut conn, id, "legacyagent", "beat", 30).await; // stale, on
		SourcePolicy::set_ingest(&mut conn, "legacyagent", IngestMode::Ignore)
			.await
			.expect("set ignore");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(
			issue_for(&mut conn, id).await.is_none(),
			"an ignored source going stale never affects reachability"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_clears_reachability_when_source_reports_again() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://returning.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 20).await;
		insert_check_state(&mut conn, id, "otheragent", "ping", 2).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("first sweep");
		assert!(issue_for(&mut conn, id).await.unwrap().active);

		// The source reports again: ingestion re-stamps its check state.
		sql_query(
			"UPDATE issues SET last_seen = NOW() WHERE application_id = $1 AND source = 'alertd'",
		)
		.bind::<sql_types::Uuid, _>(id)
		.execute(&mut conn)
		.await
		.expect("restamp state");
		Status::sweep_staleness(&mut conn)
			.await
			.expect("second sweep");
		assert!(!issue_for(&mut conn, id).await.unwrap().active);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_closes_issue_when_server_returns() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://recover.invalid/", 600).await;
		insert_status_at(&mut conn, id, 45).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("first sweep");
		assert!(issue_for(&mut conn, id).await.unwrap().active);

		// New status row at "now" → fresh again, sweep should close the issue.
		insert_status_at(&mut conn, id, 0).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("second sweep");
		assert!(!issue_for(&mut conn, id).await.unwrap().active);
	})
	.await
}

/// Going unreachable says the target's results are no longer current; it does
/// not discard them. The last observed result of each check stays exactly as it
/// was last reported, and reachability is the separate fact filed alongside.
// spec: CHK
#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_target_keeps_its_last_observed_check_results() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://keeps-results.invalid/", 600).await;
		insert_check_state(&mut conn, id, "alertd", "db", 20).await;
		insert_check_state(&mut conn, id, "otheragent", "ping", 30).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let reachability = issue_for(&mut conn, id).await.expect("reachability issue");
		assert_eq!(
			reachability.observed_result,
			Some(CheckResult::Failed),
			"nothing is reaching Canopy, so the target presents as unreachable",
		);

		for (source, check) in [("alertd", "db"), ("otheragent", "ping")] {
			let kept = Issue::list_by_source_ref(
				&mut conn,
				source,
				&format!("health/{check}"),
				std::slice::from_ref(&id),
			)
			.await
			.expect("list issues")
			.into_iter()
			.next()
			.unwrap_or_else(|| panic!("{source}'s {check} state survives the sweep"));
			assert_eq!(
				kept.observed_result,
				Some(CheckResult::Passed),
				"{source}'s last observed result is untouched by going unreachable",
			);
			assert_eq!(kept.message, "seeded");
		}
	})
	.await
}

/// Seed machine-scope check state, as ingestion stamps it when a source
/// reports a machine-subject check. The catalog row has to land in the
/// namespace ingest would have used, or the source is not counted as expected.
async fn insert_machine_check_state(
	conn: &mut diesel_async::AsyncPgConnection,
	machine_id: Uuid,
	source: &str,
	check: &str,
	minutes_ago: i32,
) {
	CheckPolicy::upsert_default(conn, source, &Namespace::for_machine(source, check), check)
		.await
		.expect("catalog the seeded check");
	sql_query(
		r#"
			INSERT INTO issues
			(machine_id, source, ref, check_name, observed_result, effective_result,
			 message, active, first_seen, last_seen)
			VALUES ($1, $2, 'health/' || $3, $3, 'passed', 'passed',
			 'seeded', false,
			 NOW() - ($4 || ' minutes')::INTERVAL, NOW() - ($4 || ' minutes')::INTERVAL)
		"#,
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(source)
	.bind::<sql_types::Text, _>(check)
	.bind::<sql_types::Text, _>(minutes_ago.to_string())
	.execute(conn)
	.await
	.expect("insert machine check state");
}

/// A box's silence is its own fact. The application on it is reporting
/// normally, so nothing about the application changes.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_box_is_unreachable_on_its_own_account() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://quiet-box.invalid/", 600).await;
		let machine = machine_of(&mut conn, id).await;
		insert_status_at(&mut conn, id, 0).await;
		// The application reported just now; the box itself last did so well
		// past its own threshold.
		insert_machine_detail_at(&mut conn, machine, 45).await;
		sql_query("UPDATE statuses SET machine_id = NULL WHERE server_id = $1")
			.bind::<sql_types::Uuid, _>(id)
			.execute(&mut conn)
			.await
			.expect("unstamp machine");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);

		let issue = issue_for_machine(&mut conn, machine)
			.await
			.expect("machine reachability issue");
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert!(issue.active);
		assert_eq!(
			issue.machine_id,
			Some(machine),
			"a machine's reachability is filed at machine scope",
		);
		assert!(issue.application_id.is_none());
		assert!(
			issue.message.contains("Machine"),
			"the operator reads which grain went quiet: {}",
			issue.message
		);

		assert!(
			issue_for(&mut conn, id).await.is_none(),
			"nothing derives an application's reachability from its machine's",
		);
	})
	.await
}

/// The other direction: an application going quiet says nothing about the box
/// it runs on, which may be running another workload perfectly well.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_application_leaves_its_machine_reachable() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://quiet-app.invalid/", 600).await;
		let machine = machine_of(&mut conn, id).await;
		insert_status_at(&mut conn, id, 45).await;
		insert_machine_detail_at(&mut conn, machine, 0).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		assert_eq!(
			issue_for(&mut conn, id)
				.await
				.expect("application reachability issue")
				.effective_result,
			Some(CheckResult::Failed)
		);
		assert!(
			issue_for_machine(&mut conn, machine).await.is_none(),
			"nothing derives a machine's reachability from an application's",
		);
	})
	.await
}

/// A machine carries its own `alert_when_down_for`, so adding a workload to a
/// box does not change how long the box may be silent.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_is_graded_on_its_own_threshold() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// The application's threshold is a minute; the box's is half an hour.
		let id = insert_server(&mut conn, "http://slow-box.invalid/", 60).await;
		let machine = machine_of(&mut conn, id).await;
		sql_query("UPDATE machines SET alert_when_down_for = INTERVAL '30 minutes' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(machine)
			.execute(&mut conn)
			.await
			.expect("widen the machine threshold");
		insert_status_at(&mut conn, id, 15).await;
		insert_machine_detail_at(&mut conn, machine, 15).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		assert!(
			issue_for(&mut conn, id).await.is_some(),
			"fifteen minutes is past the application's one-minute threshold",
		);
		assert!(
			issue_for_machine(&mut conn, machine).await.is_none(),
			"fifteen minutes is well within the box's own half-hour threshold",
		);
	})
	.await
}

/// A box running no application Canopy holds still reports, and its silence is
/// still a fact about it.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_bare_box_has_reachability_of_its_own() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let machine: RowId = sql_query("INSERT INTO machines (name) VALUES ('bare') RETURNING id")
			.get_result(&mut conn)
			.await
			.expect("insert machine");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for_machine(&mut conn, machine.id)
			.await
			.expect("machine reachability issue");
		assert_eq!(issue.effective_result, Some(CheckResult::Failed));
		assert!(
			issue.message.contains("Machine bare has never reported"),
			"got: {}",
			issue.message
		);
	})
	.await
}

/// A machine's expected sources are the ones reporting machine-subject checks
/// against it, graded exactly as an application's are.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_own_sources_grade_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://box-sources.invalid/", 600).await;
		let machine = machine_of(&mut conn, id).await;
		insert_status_at(&mut conn, id, 0).await;
		insert_machine_check_state(&mut conn, machine, "alertd", "disk_free", 20).await;
		insert_machine_check_state(&mut conn, machine, "otheragent", "load", 2).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 1);
		let issue = issue_for_machine(&mut conn, machine)
			.await
			.expect("machine reachability issue");
		assert_eq!(
			issue.observed_result,
			Some(CheckResult::Warning),
			"one source of two has gone quiet on the box",
		);
		assert!(issue.message.contains("alertd"));
	})
	.await
}

/// The recovery half: the box reports again and its reachability closes.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_reporting_again_closes_its_reachability() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://box-returns.invalid/", 600).await;
		let machine = machine_of(&mut conn, id).await;
		insert_machine_detail_at(&mut conn, machine, 45).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("first sweep");
		assert!(issue_for_machine(&mut conn, machine).await.unwrap().active);

		insert_machine_detail_at(&mut conn, machine, 0).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("second sweep");
		assert!(!issue_for_machine(&mut conn, machine).await.unwrap().active);
	})
	.await
}

/// The meta row stands in for "canopy itself" and is not a box anyone can
/// reach, so the sweep passes over it exactly as it does the meta application.
#[tokio::test(flavor = "multi_thread")]
async fn the_meta_machine_is_not_swept() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for_machine(&mut conn, Uuid::nil()).await.is_none());
	})
	.await
}

/// The all-stale arm at the machine grain: no source is still reporting about
/// the box, so it is unreachable rather than merely quiet on one source.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_whose_sources_are_all_stale_is_unreachable() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://all-stale-box.invalid/", 600).await;
		let machine = machine_of(&mut conn, id).await;
		insert_status_at(&mut conn, id, 0).await;
		insert_machine_check_state(&mut conn, machine, "alertd", "disk_free", 20).await;
		insert_machine_check_state(&mut conn, machine, "otheragent", "load", 30).await;

		Status::sweep_staleness(&mut conn).await.expect("sweep");
		let issue = issue_for_machine(&mut conn, machine)
			.await
			.expect("machine reachability issue");
		assert_eq!(issue.observed_result, Some(CheckResult::Failed));
		assert!(issue.active);
	})
	.await
}

/// A box goes quiet and takes both its workloads with it. Each of the three is
/// unreachable on its own account, by the same rule applied at its own grain:
/// there is no step that reads the machine and marks the applications.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn every_application_on_a_quiet_box_is_independently_unreachable() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let central = insert_server(&mut conn, "http://both-quiet.invalid/", 600).await;
		let machine = machine_of(&mut conn, central).await;
		let facility =
			insert_application_on(&mut conn, machine, "http://both-quiet-fac.invalid/", 600).await;
		// The box last reported 45 minutes ago, and so, by the same act, did
		// each workload on it.
		insert_machine_detail_at(&mut conn, machine, 45).await;
		insert_status_at(&mut conn, central, 45).await;
		insert_status_at(&mut conn, facility, 45).await;

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 3, "two applications and the box they run on");
		for target in [central, facility] {
			let issue = issue_for(&mut conn, target).await.expect("issue exists");
			assert_eq!(issue.effective_result, Some(CheckResult::Failed));
			assert!(issue.active);
		}
		assert_eq!(
			issue_for_machine(&mut conn, machine)
				.await
				.expect("machine issue")
				.effective_result,
			Some(CheckResult::Failed)
		);
	})
	.await
}

/// And they recover the same way. One workload comes back with the box; the
/// other is still down, and stays unreachable while its neighbours clear.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn applications_recover_independently_of_their_machine() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let central = insert_server(&mut conn, "http://partial-return.invalid/", 600).await;
		let machine = machine_of(&mut conn, central).await;
		let facility = insert_application_on(
			&mut conn,
			machine,
			"http://partial-return-fac.invalid/",
			600,
		)
		.await;
		insert_machine_detail_at(&mut conn, machine, 45).await;
		insert_status_at(&mut conn, central, 45).await;
		insert_status_at(&mut conn, facility, 45).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("first sweep");

		// The box comes back, and with it the central; the facility does not.
		insert_machine_detail_at(&mut conn, machine, 0).await;
		insert_status_at(&mut conn, central, 0).await;
		Status::sweep_staleness(&mut conn)
			.await
			.expect("second sweep");

		assert!(!issue_for_machine(&mut conn, machine).await.unwrap().active);
		assert!(!issue_for(&mut conn, central).await.unwrap().active);
		assert!(
			issue_for(&mut conn, facility).await.unwrap().active,
			"a workload that is still down stays unreachable when its box returns",
		);
	})
	.await
}

/// A workload that has fallen over while its box keeps reporting has not gone
/// away: the box is still saying what it sees, and what it sees is bad. That
/// is a failing check, and unreachable would say the opposite — that nothing
/// is being heard about it.
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn a_dead_application_on_a_live_machine_is_unhealthy_not_unreachable() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let id = insert_server(&mut conn, "http://dead-app.invalid/", 600).await;
		let machine = machine_of(&mut conn, id).await;
		// Both grains reported just now, the box carrying the application's
		// report as it always does.
		insert_status_at(&mut conn, id, 0).await;
		insert_machine_detail_at(&mut conn, machine, 0).await;
		database::issues::file_check(
			&mut conn,
			database::issues::CheckFiling {
				source: "alertd",
				scope: database::issues::Scope::Application(id),
				device_id: None,
				check: "tamanu_api",
				observed: CheckResult::Failed,
				title: None,
				message: "the API is not answering",
				detail: None,
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
		)
		.await
		.expect("file the failing check");

		let filed = Status::sweep_staleness(&mut conn).await.expect("sweep");
		assert_eq!(filed, 0, "nothing has gone quiet");
		assert!(
			issue_for(&mut conn, id).await.is_none(),
			"a failing workload is not an unreachable one",
		);
		assert!(issue_for_machine(&mut conn, machine).await.is_none());

		let health = database::issues::health_from_check_state(&mut conn, &[(id, None)])
			.await
			.expect("rollup");
		assert_eq!(
			health.get(&id).copied(),
			Some(commons_types::status::HealthState::Unhealthy),
		);
	})
	.await
}
