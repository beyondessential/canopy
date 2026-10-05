//! `issues::consolidated_checks_latest` — a server's current checks across
//! every source, graded, with the health rollup matching the headline.

use commons_types::namespace::Namespace;
use commons_types::status::{CheckResult, HealthState};
use database::check_policies::{CheckPolicy, ScopedCheckPolicy};

use crate::helpers::app_ns;
use database::issues::{
	CheckFiling, Scope, consolidated_checks_latest, consolidated_checks_latest_for_machine,
	file_check,
};
use database::statuses::{CANOPY_SOURCE, REACHABILITY_REF};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let machine: RowId = sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, machine_id) \
		 VALUES ('tamanu-central', 'http://consolidated.invalid/', $1) RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

fn filing<'a>(
	server_id: Uuid,
	source: &'a str,
	check: &'a str,
	observed: CheckResult,
) -> CheckFiling<'a> {
	CheckFiling {
		source,
		scope: Scope::Application(server_id),
		device_id: None,
		check,
		observed,
		title: None,
		message: "consolidated test",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_merges_all_sources_and_matches_rollup() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(server_id, "alertd", "db", CheckResult::Failed),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(server_id, "alertd", "disk", CheckResult::Passed),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(server_id, "tamanu", "tasks", CheckResult::Passed),
		)
		.await
		.expect("file");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		// All three checks from both sources, most urgent first.
		assert_eq!(consolidated.checks.len(), 3);
		assert_eq!(consolidated.checks[0].effective, CheckResult::Failed);
		assert_eq!(consolidated.checks[0].check, "db");
		let sources: std::collections::BTreeSet<&str> = consolidated
			.checks
			.iter()
			.map(|c| c.source.as_str())
			.collect();
		assert!(sources.contains("alertd") && sources.contains("tamanu"));

		// Rollup matches the headline: a failure makes it unhealthy.
		assert_eq!(consolidated.health_state, HealthState::Unhealthy);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_excludes_orphaned_check_states() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		// A failing check whose catalog row is then deleted, stranding its
		// check-state row (the bestool-alertd situation: an `issues` row with
		// no `check_policies` policy — invisible in settings, unmanageable).
		file_check(
			&mut conn,
			filing(
				server_id,
				"bestool-alertd",
				"sync-errors",
				CheckResult::Failed,
			),
		)
		.await
		.expect("file");
		sql_query("DELETE FROM check_policies WHERE source = 'bestool-alertd'")
			.execute(&mut conn)
			.await
			.expect("strand the check-state");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		// An orphaned check-state (no catalog row) is not a manageable check:
		// it must not surface in the detail view...
		assert!(
			consolidated.checks.is_empty(),
			"orphaned check-state is excluded from the detail view",
		);
		// ...nor drag the health rollup — a server cannot be broken by a check
		// that no longer exists in the catalog.
		assert_eq!(consolidated.health_state, HealthState::Healthy);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_excludes_decommissioned_and_flags_silenced() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(server_id, "alertd", "gone", CheckResult::Warning),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(server_id, "alertd", "hushed", CheckResult::Warning),
		)
		.await
		.expect("file");

		// Decommission one check; silence the other at server scope.
		sql_query(
			"UPDATE check_policies SET decommissioned_at = now() \
			 WHERE source = 'alertd' AND check_name = 'gone'",
		)
		.execute(&mut conn)
		.await
		.expect("decommission");
		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"hushed",
			None,
			Some("op"),
		)
		.await
		.expect("silence");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		// The decommissioned check is gone; the silenced one is present but
		// flagged.
		assert_eq!(consolidated.checks.len(), 1);
		assert_eq!(consolidated.checks[0].check, "hushed");
		assert!(consolidated.checks[0].silenced);
		// Silencing caps the effective result to skipped — matching the
		// rollup's exclusion and the snapshot path's ceiling — while the
		// observed result still records what was reported.
		assert_eq!(consolidated.checks[0].effective, CheckResult::Skipped);
		assert_eq!(consolidated.checks[0].observed, Some(CheckResult::Warning));
	})
	.await
}

/// Find the reachability entry among a server's consolidated checks.
fn reachability(
	consolidated: &commons_types::status::ConsolidatedChecks,
) -> Option<&commons_types::status::ConsolidatedCheck> {
	consolidated
		.checks
		.iter()
		.find(|c| c.source == CANOPY_SOURCE && c.check == REACHABILITY_REF)
}

#[tokio::test(flavor = "multi_thread")]
async fn reachability_presents_as_passed_when_it_has_never_degraded() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// The sweep files reachability only while it's degraded, so a server
		// that has never had a reporter go quiet carries no state for it. It
		// still presents — green — so the check and its silence control are
		// there before anything is red.
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		let check = reachability(&consolidated).expect("reachability presents");
		assert_eq!(check.effective, CheckResult::Passed);
		assert_eq!(check.observed, Some(CheckResult::Passed));
		assert!(!check.silenced);
		// A passing check doesn't count against the server.
		assert_eq!(consolidated.health_state, HealthState::Healthy);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn synthesised_reachability_reflects_a_silence() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;
		// Reachability is canopy's own, so it is flat: no namespace to name.
		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			CANOPY_SOURCE,
			&Namespace::Flat,
			REACHABILITY_REF,
			None,
			Some("op"),
		)
		.await
		.expect("silence reachability");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		// Silenced reads skipped rather than green: the operator turned
		// alerting off, and the check says so.
		let check = reachability(&consolidated).expect("reachability presents");
		assert_eq!(check.effective, CheckResult::Skipped);
		assert!(check.silenced);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_filed_reachability_wins_over_the_synthesised_one() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(
				server_id,
				CANOPY_SOURCE,
				REACHABILITY_REF,
				CheckResult::Failed,
			),
		)
		.await
		.expect("file");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		// One entry, not two: the recorded state is the check.
		assert_eq!(consolidated.checks.len(), 1);
		let check = reachability(&consolidated).expect("reachability presents");
		assert_eq!(check.effective, CheckResult::Failed);
		assert_eq!(consolidated.health_state, HealthState::Unhealthy);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn decommissioning_reachability_removes_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;
		sql_query(
			"UPDATE check_policies SET decommissioned_at = now() \
			 WHERE source = $1 AND check_name = $2",
		)
		.bind::<sql_types::Text, _>(CANOPY_SOURCE)
		.bind::<sql_types::Text, _>(REACHABILITY_REF)
		.execute(&mut conn)
		.await
		.expect("decommission");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");

		// Presentation follows the catalog: a retired check stays retired
		// rather than being conjured back by the fill-in.
		assert!(reachability(&consolidated).is_none());
	})
	.await
}

/// The machine `insert_server` put the application on.
async fn machine_of(conn: &mut diesel_async::AsyncPgConnection, application: Uuid) -> Uuid {
	let row: RowId = sql_query("SELECT machine_id AS id FROM applications WHERE id = $1")
		.bind::<sql_types::Uuid, _>(application)
		.get_result(conn)
		.await
		.expect("machine of application");
	row.id
}

fn machine_filing<'a>(
	machine_id: Uuid,
	source: &'a str,
	check: &'a str,
	observed: CheckResult,
) -> CheckFiling<'a> {
	CheckFiling {
		scope: Scope::Machine(machine_id),
		..filing(machine_id, source, check, observed)
	}
}

/// An application presents its own checks and no other target's. The box's
/// are read on the machine, and an application's own reachability is still
/// there.
// spec: CHK#presentation
#[tokio::test(flavor = "multi_thread")]
async fn an_application_lists_none_of_its_machines_checks() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;
		let machine_id = machine_of(&mut conn, server_id).await;

		file_check(
			&mut conn,
			filing(server_id, "tamanu", "tasks", CheckResult::Warning),
		)
		.await
		.expect("file the application's");
		file_check(
			&mut conn,
			machine_filing(machine_id, "alertd", "disk_free", CheckResult::Failed),
		)
		.await
		.expect("file the machine's");
		file_check(
			&mut conn,
			machine_filing(
				machine_id,
				CANOPY_SOURCE,
				REACHABILITY_REF,
				CheckResult::Failed,
			),
		)
		.await
		.expect("file the machine's reachability");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		let names: Vec<&str> = consolidated
			.checks
			.iter()
			.map(|c| c.check.as_str())
			.collect();
		assert!(names.contains(&"tasks"), "the application's own: {names:?}");
		assert!(!names.contains(&"disk_free"), "not the box's: {names:?}");
		let reachability: Vec<_> = consolidated
			.checks
			.iter()
			.filter(|c| c.source == CANOPY_SOURCE && c.check == REACHABILITY_REF)
			.collect();
		assert_eq!(reachability.len(), 1, "the application's own reachability");
		assert_eq!(
			reachability[0].effective,
			CheckResult::Passed,
			"the box's failing reachability is the box's"
		);

		let machine = consolidated_checks_latest_for_machine(&mut conn, machine_id, None)
			.await
			.expect("machine's consolidated");
		assert!(
			machine.checks.iter().any(|c| c.check == "disk_free"),
			"the box's check is read on the box"
		);
	})
	.await
}

/// A box in trouble is the box's trouble: the application is graded on its own
/// checks, so a failure on the box does not read as one in every workload on
/// it.
// spec: CHK#health-rollup
#[tokio::test(flavor = "multi_thread")]
async fn an_applications_rollup_leaves_out_its_machines_checks() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let machine_id = machine_of(&mut conn, server_id).await;
		file_check(
			&mut conn,
			filing(server_id, "tamanu", "tasks", CheckResult::Passed),
		)
		.await
		.expect("file the application's");
		file_check(
			&mut conn,
			machine_filing(machine_id, "alertd", "disk_free", CheckResult::Failed),
		)
		.await
		.expect("file the machine's");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		assert_eq!(consolidated.health_state, HealthState::Healthy);

		let machine_health =
			database::issues::machine_health_from_check_state(&mut conn, &[(machine_id, None)])
				.await
				.expect("machine rollup");
		assert_eq!(
			machine_health.get(&machine_id).copied(),
			Some(HealthState::Unhealthy),
		);
	})
	.await
}

/// Move a state's last stamp back, as though its source's later reports had
/// not carried it.
async fn age_state(
	conn: &mut diesel_async::AsyncPgConnection,
	application: Uuid,
	source: &str,
	check: &str,
	by: &str,
) {
	sql_query(
		"UPDATE issues SET last_seen = last_seen - $4::interval \
		 WHERE application_id = $1 AND source = $2 AND check_name = $3",
	)
	.bind::<sql_types::Uuid, _>(application)
	.bind::<sql_types::Text, _>(source)
	.bind::<sql_types::Text, _>(check)
	.bind::<sql_types::Text, _>(by)
	.execute(conn)
	.await
	.expect("age state");
}

fn names(consolidated: &commons_types::status::ConsolidatedChecks) -> Vec<&str> {
	consolidated
		.checks
		.iter()
		.filter(|c| !(c.source == CANOPY_SOURCE && c.check == REACHABILITY_REF))
		.map(|c| c.check.as_str())
		.collect()
}

/// A resolved state has been dealt with and presents nowhere, whatever its
/// last result was.
// spec: CHK#presentation
#[tokio::test(flavor = "multi_thread")]
async fn a_resolved_state_is_not_listed() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(server_id, "alertd", "db", CheckResult::Failed),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(server_id, "alertd", "disk", CheckResult::Passed),
		)
		.await
		.expect("file");
		sql_query(
			"UPDATE issues SET resolved_at = NOW(), resolved_by = 'test' \
			 WHERE application_id = $1 AND check_name = 'db'",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("resolve");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		assert_eq!(names(&consolidated), vec!["disk"]);
	})
	.await
}

/// A check its source no longer carries drops off the list, while the source's
/// other checks stay.
// spec: CHK#presentation
#[tokio::test(flavor = "multi_thread")]
async fn a_check_its_source_stopped_reporting_is_not_listed() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		for check in ["db", "disk"] {
			file_check(
				&mut conn,
				filing(server_id, "alertd", check, CheckResult::Passed),
			)
			.await
			.expect("file");
		}
		age_state(&mut conn, server_id, "alertd", "db", "1 hour").await;

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		assert_eq!(names(&consolidated), vec!["disk"]);
	})
	.await
}

/// A retired thread sharing a check's name beside its live state presents the
/// check once: the live state, with its detail.
// spec: CHK#state
#[tokio::test(flavor = "multi_thread")]
async fn a_retired_thread_does_not_present_a_check_twice() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(server_id, "alertd", "caddy_version", CheckResult::Passed),
		)
		.await
		.expect("file");
		sql_query(
			"INSERT INTO issues (application_id, source, ref, message, active, first_seen, last_seen, \
			 check_name, observed_result, effective_result) \
			 VALUES ($1, 'alertd', 'health-broken/caddy_version', 'retired', false, \
			 NOW() - interval '90 days', NOW() - interval '90 days', 'caddy_version', 'passed', 'passed')",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("insert the retired thread");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		assert_eq!(names(&consolidated), vec!["caddy_version"]);
	})
	.await
}

/// Canopy's own determinations aren't reports, so how long ago one was filed
/// says nothing about whether it is current: unresolved is current.
// spec: CHK#presentation
#[tokio::test(flavor = "multi_thread")]
async fn a_reserved_sources_unresolved_state_stays_listed() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(
				server_id,
				CANOPY_SOURCE,
				"certificate-expiry",
				CheckResult::Warning,
			),
		)
		.await
		.expect("file canopy's");
		file_check(
			&mut conn,
			filing(server_id, CANOPY_SOURCE, "dns-records", CheckResult::Passed),
		)
		.await
		.expect("file canopy's");
		age_state(
			&mut conn,
			server_id,
			CANOPY_SOURCE,
			"certificate-expiry",
			"30 days",
		)
		.await;

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		let listed = names(&consolidated);
		assert!(listed.contains(&"certificate-expiry"), "{listed:?}");
		assert!(listed.contains(&"dns-records"), "{listed:?}");
		assert!(consolidated.checks.iter().all(|c| !c.quiet));
	})
	.await
}

/// A source that hasn't reported within the application's down threshold has
/// its checks presented at their last result, marked quiet, and its last
/// failure still counts against the application: last known bad stays bad.
// spec: CHK#presentation
// spec: CHK#health-rollup
#[tokio::test(flavor = "multi_thread")]
async fn a_quiet_sources_checks_are_marked_and_still_count() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(server_id, "tamanu", "tasks", CheckResult::Failed),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(server_id, "alertd", "db", CheckResult::Passed),
		)
		.await
		.expect("file");
		age_state(&mut conn, server_id, "tamanu", "tasks", "46 days").await;

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		let by_check: std::collections::HashMap<&str, &commons_types::status::ConsolidatedCheck> =
			consolidated
				.checks
				.iter()
				.map(|c| (c.check.as_str(), c))
				.collect();
		let tasks = by_check.get("tasks").expect("the quiet source's check");
		assert!(tasks.quiet);
		assert_eq!(tasks.effective, CheckResult::Failed);
		let reported = tasks.last_reported_at.expect("when it was last reported");
		assert!(
			jiff::Timestamp::now().duration_since(reported)
				> jiff::SignedDuration::from_hours(24 * 45)
		);
		assert!(!by_check.get("db").expect("the live source's check").quiet);
		assert_eq!(consolidated.health_state, HealthState::Unhealthy);
	})
	.await
}

/// Most urgent first, then by presented name: a bare name and an application
/// type's interleave alphabetically on what an operator reads.
// spec: CHK#presentation
#[tokio::test(flavor = "multi_thread")]
async fn checks_order_by_result_then_presented_name() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;
		for (check, result) in [
			("caddy_certs", CheckResult::Passed),
			("sync_facility_stale", CheckResult::Failed),
			("memory", CheckResult::Passed),
		] {
			file_check(&mut conn, filing(server_id, "alertd", check, result))
				.await
				.expect("file");
		}
		file_check(
			&mut conn,
			filing(
				server_id,
				CANOPY_SOURCE,
				"certificate-expiry",
				CheckResult::Warning,
			),
		)
		.await
		.expect("file canopy's");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		let order: Vec<&str> = consolidated
			.checks
			.iter()
			.map(|c| c.qualified_name.as_str())
			.collect();
		assert_eq!(
			order,
			vec![
				"tamanu-central:sync_facility_stale",
				"certificate-expiry",
				"reachability",
				"tamanu-central:caddy_certs",
				"tamanu-central:memory",
			]
		);
	})
	.await
}

/// A machine's own list follows the same rules as an application's: a check
/// its source no longer reports, and a resolved one, are not presented.
// spec: CHK#presentation
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_list_presents_only_current_checks() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let machine_id = machine_of(&mut conn, server_id).await;
		for check in ["disk_free", "memory", "load"] {
			file_check(
				&mut conn,
				machine_filing(machine_id, "alertd", check, CheckResult::Passed),
			)
			.await
			.expect("file the machine's");
		}
		sql_query(
			"UPDATE issues SET last_seen = last_seen - INTERVAL '1 hour' \
			 WHERE machine_id = $1 AND check_name = 'memory'",
		)
		.bind::<sql_types::Uuid, _>(machine_id)
		.execute(&mut conn)
		.await
		.expect("age");
		sql_query(
			"UPDATE issues SET resolved_at = NOW(), resolved_by = 'test' \
			 WHERE machine_id = $1 AND check_name = 'load'",
		)
		.bind::<sql_types::Uuid, _>(machine_id)
		.execute(&mut conn)
		.await
		.expect("resolve");

		let machine = consolidated_checks_latest_for_machine(&mut conn, machine_id, None)
			.await
			.expect("machine's consolidated");
		assert_eq!(names(&machine), vec!["disk_free"]);
	})
	.await
}

/// The muted checks and the reachability check read one clock: every source
/// reachability names as stale has its checks muted.
// spec: CHK#presentation
// spec: CHK#reachability
#[tokio::test(flavor = "multi_thread")]
async fn every_source_reachability_names_stale_is_muted() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::seed_own_checks(&mut conn)
			.await
			.expect("seed canopy's own checks");
		let server_id = insert_server(&mut conn).await;
		file_check(
			&mut conn,
			filing(server_id, "otheragent", "ping", CheckResult::Passed),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(server_id, "alertd", "db", CheckResult::Passed),
		)
		.await
		.expect("file");
		age_state(&mut conn, server_id, "otheragent", "ping", "46 days").await;

		database::statuses::Status::sweep_staleness(&mut conn)
			.await
			.expect("sweep");

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		let reachability = consolidated
			.checks
			.iter()
			.find(|c| c.source == CANOPY_SOURCE && c.check == REACHABILITY_REF)
			.expect("reachability");
		let stale: Vec<&str> = reachability.detail["stale_sources"]
			.as_array()
			.expect("stale sources named")
			.iter()
			.map(|s| s["source"].as_str().unwrap())
			.collect();
		assert_eq!(stale, vec!["otheragent"]);
		for check in &consolidated.checks {
			if stale.contains(&check.source.as_str()) {
				assert!(check.quiet, "{} is from a stale source", check.check);
			} else if check.source != CANOPY_SOURCE {
				assert!(!check.quiet, "{} is from a live source", check.check);
			}
		}
	})
	.await
}
