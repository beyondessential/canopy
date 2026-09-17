//! `issues::consolidated_checks_latest` — a server's current checks across
//! every source, graded, with the health rollup matching the headline.

use commons_types::namespace::Namespace;
use commons_types::status::{CheckResult, HealthState};
use database::check_policies::{CheckPolicy, ScopedCheckPolicy};

use crate::helpers::app_ns;
use database::issues::{CheckFiling, Scope, consolidated_checks_latest, file_check};
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
	let machine: RowId = sql_query("INSERT INTO machines DEFAULT VALUES RETURNING id")
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

#[tokio::test(flavor = "multi_thread")]
async fn an_application_presents_its_machines_checks_as_the_machines() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// An operator triaging an application sees the box's checks among its
		// own, marked as the box's so it's clear the fact is shared with every
		// workload on it.
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
		// The box has gone quiet in its own right. That already reaches the
		// application as its own unreachability, so the box's must not appear
		// a second time under the application.
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

		let by_check: std::collections::HashMap<&str, &commons_types::status::ConsolidatedCheck> =
			consolidated
				.checks
				.iter()
				.map(|c| (c.check.as_str(), c))
				.collect();

		let own = by_check.get("tasks").expect("the application's own check");
		assert_eq!(
			own.subject,
			commons_types::subject::CheckSubject::Application
		);
		let boxs = by_check.get("disk_free").expect("the machine's check");
		assert_eq!(boxs.subject, commons_types::subject::CheckSubject::Machine);
		assert_eq!(boxs.effective, CheckResult::Failed);

		// Exactly one reachability, the application's own, unaffected by the
		// box's.
		let reachabilities: Vec<_> = consolidated
			.checks
			.iter()
			.filter(|c| c.source == CANOPY_SOURCE && c.check == REACHABILITY_REF)
			.collect();
		assert_eq!(
			reachabilities.len(),
			1,
			"one reachability, the application's"
		);
		assert_eq!(
			reachabilities[0].subject,
			commons_types::subject::CheckSubject::Application
		);
	})
	.await
}

/// A box in trouble is the box's trouble. The application presents its
/// machine's checks so an operator reads them where they matter, and is graded
/// on its own, so a warning on the box does not read as a warning in every
/// workload on it.
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

		let health = database::issues::health_from_check_state(&mut conn, &[(server_id, None)])
			.await
			.expect("rollup");
		// Absent from the rollup is how "nothing against it" reads; callers
		// default that healthy, as the consolidated read below does.
		assert_eq!(
			health
				.get(&server_id)
				.copied()
				.unwrap_or(HealthState::Healthy),
			HealthState::Healthy,
		);

		let consolidated = consolidated_checks_latest(&mut conn, server_id, None)
			.await
			.expect("consolidated");
		assert_eq!(consolidated.health_state, HealthState::Healthy);
		// The box's check is still on the application's list, marked as the
		// box's, so nothing is hidden by not being counted.
		let boxs = consolidated
			.checks
			.iter()
			.find(|c| c.check == "disk_free")
			.expect("the machine's check on the application's list");
		assert_eq!(boxs.subject, commons_types::subject::CheckSubject::Machine);
		assert_eq!(boxs.effective, CheckResult::Failed);

		// And the box itself is graded down by it.
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
