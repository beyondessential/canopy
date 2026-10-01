//! Self-alert lifecycle: a raise files one coalescing canopy-wide issue
//! which opens a canopy-wide incident, enqueuing exactly one Slack open on
//! the not-alerting → alerting transition (with flap grace for
//! non-escalating checks, immediate for escalating ones); recovery starts
//! the incident lingering, and a re-raise within the linger continues the
//! same incident without a new notification; a flap whose open never
//! shipped is cancelled silently at linger expiry; recovery after delivery
//! enqueues the resolve at linger expiry; idle recovers write nothing.

use commons_types::status::CheckResult;
use database::self_alerts;
use database::slack_outbox::{KIND_INCIDENT_OPEN, KIND_INCIDENT_RESOLVE, SlackOutbox};
use diesel::prelude::*;
use diesel_async::RunQueryDsl;
use jiff::Timestamp;

const REF: &str = "test-self-alert";

/// Raise REF as a failure; `escalates` picks the registered policy tier
/// (escalating ≙ the old Critical, immediate notify; otherwise graced).
/// Each test runs on a fresh database, so the first raise registers the
/// catalog entry at the given tier.
async fn raise_with(
	conn: &mut diesel_async::AsyncPgConnection,
	escalates: bool,
) -> database::issues::Issue {
	self_alerts::raise(
		conn,
		REF,
		CheckResult::Failed,
		CheckResult::Failed,
		escalates,
		None,
		"title",
		"body",
	)
	.await
	.expect("raise")
}

async fn outbox_rows(conn: &mut diesel_async::AsyncPgConnection) -> Vec<SlackOutbox> {
	use database::schema::slack_outbox::dsl;
	dsl::slack_outbox
		.select(SlackOutbox::as_select())
		.order(dsl::created_at.asc())
		.load(conn)
		.await
		.expect("load outbox")
}

/// Backdate the canopy-wide incident's `closing_at` past the global linger
/// window, then run the sweep — the test-speed way to let the close-side
/// grace elapse.
async fn expire_linger(conn: &mut diesel_async::AsyncPgConnection) {
	diesel::sql_query(
		"UPDATE incidents SET closing_at = closing_at - INTERVAL '1 hour' \
		 WHERE server_group_id IS NULL",
	)
	.execute(conn)
	.await
	.expect("expire linger");
	database::issues::sweep_lingering_incidents(conn)
		.await
		.expect("linger sweep");
}

#[tokio::test(flavor = "multi_thread")]
async fn raise_enqueues_once_and_flap_recovery_is_silent() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		// Idle recover: nothing exists, nothing written.
		assert!(
			self_alerts::recover(&mut conn, REF, "nothing to do")
				.await
				.expect("recover")
				.is_none()
		);
		assert!(outbox_rows(&mut conn).await.is_empty());

		// First raise: issue + canopy-wide incident + one open row, delayed
		// by the grace (non-escalating).
		let issue = raise_with(&mut conn, false).await;
		assert_eq!(issue.application_id, None);
		assert_eq!(issue.server_group_id, None);
		assert!(issue.active);
		let rows = outbox_rows(&mut conn).await;
		let [open] = rows.as_slice() else {
			panic!("exactly one outbox row, got {rows:?}");
		};
		assert_eq!(open.kind, KIND_INCIDENT_OPEN);
		assert!(
			open.incident_id.is_some(),
			"the raise opened a canopy-wide incident"
		);
		assert_eq!(open.issue_id, Some(issue.id));
		assert!(
			open.deliver_after > Timestamp::now(),
			"non-escalating opens wait out the grace"
		);
		assert_eq!(open.payload["server"], "Canopy");
		assert_eq!(
			open.payload["source_ref"], "1 failed",
			"source_ref carries the incident's counts"
		);

		// Re-raise while alerting: no new outbox row.
		raise_with(&mut conn, false).await;
		assert_eq!(outbox_rows(&mut conn).await.len(), 1);

		// Recover: the incident lingers — the pending open is neither
		// shipped nor cancelled yet, and no resolve is enqueued.
		self_alerts::recover(&mut conn, REF, "fixed")
			.await
			.expect("recover")
			.expect("was active");
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 1, "no resolve while lingering: {rows:?}");
		assert!(
			rows[0].gave_up_at.is_none(),
			"pending open survives the linger"
		);

		// Re-raise within the linger: the same incident continues — no
		// second open row. This is the close-then-reopen noise the linger
		// exists to absorb.
		raise_with(&mut conn, false).await;
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 1, "rejoin continues the incident: {rows:?}");

		// Recover again and let the linger elapse: the open (never shipped)
		// is cancelled and no resolve follows — the whole flap was silent.
		self_alerts::recover(&mut conn, REF, "fixed again")
			.await
			.expect("recover")
			.expect("was active");
		expire_linger(&mut conn).await;
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 1, "no resolve for a flap: {rows:?}");
		assert!(rows[0].gave_up_at.is_some(), "pending open cancelled");

		// Raise again: a fresh transition, a fresh open.
		raise_with(&mut conn, false).await;
		assert_eq!(outbox_rows(&mut conn).await.len(), 2);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn escalating_ships_immediately_and_recovery_after_delivery_resolves() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		raise_with(&mut conn, true).await;
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 1);
		assert!(
			rows[0].deliver_after <= Timestamp::now(),
			"an escalating check skips the grace"
		);

		// Simulate the drainer shipping the open, then recover: the incident
		// lingers first, and the resolve is enqueued when the linger elapses.
		SlackOutbox::mark_delivered(&mut conn, rows[0].id, "ok")
			.await
			.expect("mark delivered");
		self_alerts::recover(&mut conn, REF, "identity ok")
			.await
			.expect("recover")
			.expect("was active");
		assert_eq!(
			outbox_rows(&mut conn).await.len(),
			1,
			"resolve waits out the linger"
		);
		expire_linger(&mut conn).await;
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 2);
		let resolve = &rows[1];
		assert_eq!(resolve.kind, KIND_INCIDENT_RESOLVE);
		assert_eq!(resolve.payload["server"], "Canopy");
		assert!(resolve.deliver_after <= Timestamp::now());

		// Recovering again is a no-op.
		assert!(
			self_alerts::recover(&mut conn, REF, "still ok")
				.await
				.expect("recover")
				.is_none()
		);
		assert_eq!(outbox_rows(&mut conn).await.len(), 2);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn operator_resolved_but_persisting_condition_re_notifies() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		use database::issues::Issue;

		let issue = raise_with(&mut conn, true).await;
		assert_eq!(outbox_rows(&mut conn).await.len(), 1);

		// An operator resolves it, but the condition still holds: the next
		// raise clears the resolution and notifies again — the operator's
		// claim of "fixed" was wrong and silence would hide that.
		Issue::resolve(
			&mut conn,
			issue.id,
			"op@example.com",
			commons_types::issue::ResolvedReason::Fixed,
		)
		.await
		.expect("operator resolve");
		raise_with(&mut conn, true).await;
		assert_eq!(outbox_rows(&mut conn).await.len(), 2);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn self_alert_issues_are_excluded_from_the_fleet_listing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		use database::issues::Issue;

		raise_with(&mut conn, false).await;

		let fleet = Issue::list(&mut conn, Default::default(), 100)
			.await
			.expect("list");
		assert!(
			fleet.is_empty(),
			"canopy-wide issues must not appear in the fleet listing: {fleet:?}"
		);
		let alerts = self_alerts::list(&mut conn, 50).await.expect("alerts");
		assert_eq!(alerts.len(), 1);
	})
	.await
}

/// A box's own checks are the box's business, not canopy's. They file with
/// no application and no group, which is the same shape a canopy-wide alert
/// has, so the self-alert listing reads the machine too. Otherwise every
/// box-subject check turns up in the operator's self-alert banner as one of
/// canopy's own problems.
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_issues_are_not_self_alerts() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		#[derive(diesel::QueryableByName)]
		struct RowId {
			#[diesel(sql_type = diesel::sql_types::Uuid)]
			id: uuid::Uuid,
		}
		let machine: RowId = diesel::sql_query("INSERT INTO machines DEFAULT VALUES RETURNING id")
			.get_result(&mut conn)
			.await
			.expect("insert machine");

		database::issues::file_check(
			&mut conn,
			database::issues::CheckFiling {
				source: "alertd",
				scope: database::issues::Scope::Machine(machine.id),
				device_id: None,
				check: "disk_free",
				observed: CheckResult::Failed,
				title: None,
				message: "disk nearly full",
				detail: None,
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
		)
		.await
		.expect("file machine check");

		let alerts = self_alerts::list(&mut conn, 50).await.expect("alerts");
		assert!(
			alerts.is_empty(),
			"a box's own check is not one of canopy's self-alerts: {alerts:?}"
		);
	})
	.await
}

// --- Fleet-wide check-liveness self-alert (STALE_CHECKS_REF) ---

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection) -> uuid::Uuid {
	#[derive(diesel::QueryableByName)]
	struct RowId {
		#[diesel(sql_type = diesel::sql_types::Uuid)]
		id: uuid::Uuid,
	}
	let row: RowId = diesel::sql_query(
		"WITH m AS (INSERT INTO machines DEFAULT VALUES RETURNING id) INSERT INTO applications (type, host, machine_id) SELECT 'tamanu-central', 'http://sc.invalid/', m.id FROM m RETURNING id",
	)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

/// File a check (registering its catalog row) and backdate its catalog
/// `last_seen` to `hours_ago`, simulating a check that went quiet.
async fn quiet_check(conn: &mut diesel_async::AsyncPgConnection, check: &str, hours_ago: i64) {
	let server_id = insert_server(conn).await;
	database::issues::file_check(
		conn,
		database::issues::CheckFiling {
			source: "alertd",
			scope: database::issues::Scope::Application(server_id),
			device_id: None,
			check,
			observed: CheckResult::Passed,
			title: None,
			message: "sc filing",
			detail: None,
			default_ceiling: CheckResult::Warning,
			default_escalates: false,
			documentation: None,
		},
	)
	.await
	.expect("file");
	diesel::sql_query(format!(
		"UPDATE check_policies SET last_seen = now() - interval '{hours_ago} hours' \
		 WHERE source = 'alertd' AND check_name = $1"
	))
	.bind::<diesel::sql_types::Text, _>(check)
	.execute(conn)
	.await
	.expect("backdate last_seen");
}

async fn stale_alert_active(conn: &mut diesel_async::AsyncPgConnection) -> bool {
	self_alerts::current(conn, self_alerts::STALE_CHECKS_REF)
		.await
		.expect("current")
		.map(|i| i.active)
		.unwrap_or(false)
}

#[tokio::test(flavor = "multi_thread")]
async fn stale_check_raises_the_liveness_alert() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		quiet_check(&mut conn, "gone", 24 * 31).await;
		self_alerts::sweep_stale_healthchecks(&mut conn)
			.await
			.expect("sweep");
		assert!(
			stale_alert_active(&mut conn).await,
			"a check unreported for 31 days raises the liveness alert",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn recently_seen_check_raises_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		quiet_check(&mut conn, "fresh", 24).await;
		self_alerts::sweep_stale_healthchecks(&mut conn)
			.await
			.expect("sweep");
		assert!(
			!stale_alert_active(&mut conn).await,
			"a check seen a day ago does not raise the liveness alert",
		);
	})
	.await
}

/// An application-subject entry whose type has left the fleet cannot be
/// reported again by anything, so it is not a reporter falling silent — it is
/// an entry whose population went away. Surfacing it asks the operator to tidy
/// up after a fleet change, every day, forever.
// spec: CHK#liveness-and-decommissioning
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_check_no_live_application_could_report_is_ignored() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		quiet_check(&mut conn, "orphaned", 24 * 40).await;
		// The only application of the type that reported it goes, taking the
		// population its namespace names with it.
		diesel::sql_query("UPDATE applications SET deleted_at = now()")
			.execute(&mut conn)
			.await
			.expect("archive the fleet");

		self_alerts::sweep_stale_healthchecks(&mut conn)
			.await
			.expect("sweep");

		assert!(
			!stale_alert_active(&mut conn).await,
			"an entry no live application could report into is not gone quiet",
		);
	})
	.await
}

/// The machine and curated namespaces have populations that do not empty —
/// every box, and Canopy itself — so they are never filtered out on this
/// reasoning.
// spec: CHK#liveness-and-decommissioning
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_machine_check_still_raises_with_no_applications_left() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		// `disk_free` is a machine-subject name, so its entry is the machine
		// namespace's and names no type.
		quiet_check(&mut conn, "disk_free", 24 * 40).await;
		diesel::sql_query("UPDATE applications SET deleted_at = now()")
			.execute(&mut conn)
			.await
			.expect("archive the fleet");

		self_alerts::sweep_stale_healthchecks(&mut conn)
			.await
			.expect("sweep");

		assert!(
			stale_alert_active(&mut conn).await,
			"a machine check's population is every box, which does not empty",
		);
	})
	.await
}

/// The alert has to say which entry to retire, and hand the surface enough to
/// link it. A bare `source/name` names as many entries as there are types
/// reporting that name.
// spec: SELF#presentation
#[tokio::test(flavor = "multi_thread")]
async fn the_alert_names_each_check_by_its_whole_identity() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		quiet_check(&mut conn, "db_version", 24 * 40).await;

		let issue = self_alerts::sweep_stale_healthchecks(&mut conn)
			.await
			.expect("sweep")
			.expect("an alert");

		assert!(
			issue.message.contains("alertd/tamanu-central.db_version"),
			"the message qualifies the name by the type whose check it is: {}",
			issue.message,
		);

		let detail = issue.detail.expect("structured detail");
		let checks = detail
			.get("checks")
			.and_then(|c| c.as_array())
			.expect("a checks array");
		assert_eq!(checks.len(), 1);
		let entry = &checks[0];
		assert_eq!(entry.get("source").and_then(|v| v.as_str()), Some("alertd"));
		assert_eq!(
			entry.get("check").and_then(|v| v.as_str()),
			Some("db_version")
		);
		assert_eq!(
			entry.get("subject").and_then(|v| v.as_str()),
			Some("application")
		);
		assert_eq!(
			entry.get("application_type").and_then(|v| v.as_str()),
			Some("tamanu-central"),
			"the surface needs the namespace to build the policy-page link"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn decommissioned_stale_check_is_ignored() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		quiet_check(&mut conn, "retired", 24 * 40).await;
		diesel::sql_query(
			"UPDATE check_policies SET decommissioned_at = now() \
			 WHERE source = 'alertd' AND check_name = 'retired'",
		)
		.execute(&mut conn)
		.await
		.expect("decommission");
		self_alerts::sweep_stale_healthchecks(&mut conn)
			.await
			.expect("sweep");
		assert!(
			!stale_alert_active(&mut conn).await,
			"a decommissioned check never raises the liveness alert",
		);
	})
	.await
}
