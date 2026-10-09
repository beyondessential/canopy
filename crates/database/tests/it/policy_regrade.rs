//! A change to a check's policy re-grades every state of it at once, and the
//! incidents they are in follow.
//!
//! spec: CHK#policy, INC#membership

use commons_tests::db::TestDb;
use commons_types::namespace::Namespace;
use commons_types::server::app_type::ApplicationType;
use commons_types::status::CheckResult;
use database::{
	check_policies::{CheckPolicy, IfLadder},
	diesel_async::AsyncPgConnection,
	issues::{CheckFiling, Incident, Issue, Scope, file_check},
	statuses::CANOPY_SOURCE,
};
use diesel::prelude::*;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection};
use uuid::Uuid;

const CHECK: &str = "backup-staleness";

struct Seeded {
	group: Uuid,
	applications: [Uuid; 2],
}

/// A group, lingering before it closes an incident, with two production
/// applications each on its own machine.
async fn seed(conn: &mut AsyncPgConnection) -> Seeded {
	let group = Uuid::new_v4();
	// In order, so the first is the first by target wherever states are
	// settled by target.
	let mut applications = [Uuid::new_v4(), Uuid::new_v4()];
	applications.sort();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name, slack_close_delay) \
		 VALUES ('{group}', 'regrade-{group}', INTERVAL '5 minutes')"
	))
	.await
	.expect("seed group");
	for application in applications {
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine}', '{group}'); \
			 INSERT INTO applications (id, host, type, group_id, rank, machine_id) \
			 VALUES ('{application}', 'https://{application}.example', 'tamanu-central', \
			         '{group}', 'production', '{machine}')"
		))
		.await
		.expect("seed application");
	}
	Seeded {
		group,
		applications,
	}
}

async fn save_rules(conn: &mut AsyncPgConnection, rules: serde_json::Value) {
	let ladder: IfLadder = serde_json::from_value(rules).expect("ladder");
	CheckPolicy::update_rules(
		conn,
		CANOPY_SOURCE,
		&Namespace::Flat,
		CHECK,
		Some(&ladder),
		"ops",
	)
	.await
	.expect("save rules");
}

/// Forget the inputs a state was graded with, as a state filed before they
/// were kept has none.
async fn forget_inputs(conn: &mut AsyncPgConnection, id: Uuid) {
	conn.batch_execute(&format!(
		"UPDATE issues SET grading_context = NULL WHERE id = '{id}'"
	))
	.await
	.expect("forget the grading inputs");
}

async fn file(conn: &mut AsyncPgConnection, application: Uuid, observed: CheckResult) -> Issue {
	file_check(
		conn,
		CheckFiling {
			source: CANOPY_SOURCE,
			scope: Scope::Application(application),
			device_id: None,
			check: CHECK,
			observed,
			title: Some("Backups are stale"),
			message: "last backup 3 days ago",
			detail: None,
			default_ceiling: CheckResult::Failed,
			default_escalates: false,
			documentation: None,
		},
	)
	.await
	.expect("file")
}

async fn reload(conn: &mut AsyncPgConnection, id: Uuid) -> Issue {
	Issue::get_by_id(conn, id).await.expect("reload state")
}

async fn incidents(conn: &mut AsyncPgConnection, group: Uuid) -> Vec<Incident> {
	use database::schema::incidents::dsl;
	dsl::incidents
		.select(Incident::as_select())
		.filter(dsl::server_group_id.eq(group))
		.load(conn)
		.await
		.expect("incidents")
}

async fn save_ceiling(conn: &mut AsyncPgConnection, ceiling: CheckResult) {
	CheckPolicy::update(
		conn,
		CANOPY_SOURCE,
		&Namespace::Flat,
		CHECK,
		ceiling,
		false,
		None,
		"ops",
	)
	.await
	.expect("save policy");
}

/// Grading the failure down to a warning re-grades every target's state and
/// closes the incident at once; Canopy's message, which describes what was
/// observed, is kept.
#[tokio::test(flavor = "multi_thread")]
async fn a_lowered_ceiling_regrades_every_target_and_closes() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let first = file(&mut conn, s.applications[0], CheckResult::Failed).await;
		let second = file(&mut conn, s.applications[1], CheckResult::Failed).await;
		let opened = incidents(&mut conn, s.group).await;
		assert_eq!(opened.len(), 1);
		assert!(opened[0].closed_at.is_none());

		save_ceiling(&mut conn, CheckResult::Warning).await;

		for id in [first.id, second.id] {
			let state = reload(&mut conn, id).await;
			assert_eq!(state.effective_result, Some(CheckResult::Warning));
			assert_eq!(state.observed_result, Some(CheckResult::Failed));
			assert!(state.active);
			assert_eq!(state.message, "last backup 3 days ago");
			assert_eq!(state.description.as_deref(), Some("Backups are stale"));
		}
		let after = incidents(&mut conn, s.group).await;
		assert_eq!(after.len(), 1);
		assert!(after[0].closed_at.is_some(), "closed without lingering");
		assert!(after[0].closing_at.is_none());

		// Raising it back is graded in too: the failures open a fresh incident.
		save_ceiling(&mut conn, CheckResult::Failed).await;
		assert_eq!(
			reload(&mut conn, first.id).await.effective_result,
			Some(CheckResult::Failed)
		);
		let all = incidents(&mut conn, s.group).await;
		assert_eq!(all.iter().filter(|i| i.closed_at.is_none()).count(), 1);
	})
	.await
}

/// A save that changes no grade writes nothing back: the state is as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_save_changing_no_grade_leaves_states_alone() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let filed = file(&mut conn, s.applications[0], CheckResult::Failed).await;

		save_ceiling(&mut conn, CheckResult::Failed).await;

		let state = reload(&mut conn, filed.id).await;
		assert_eq!(state.degraded_since, filed.degraded_since);
		assert_eq!(state.effective_result, Some(CheckResult::Failed));
		assert!(incidents(&mut conn, s.group).await[0].closed_at.is_none());
	})
	.await
}

/// Grading one failure away leaves an incident open while another failure is
/// still live in it.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn another_live_failure_keeps_the_incident_open() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		file(&mut conn, s.applications[0], CheckResult::Failed).await;
		file_check(
			&mut conn,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(s.applications[1]),
				device_id: None,
				check: "restore-verification",
				observed: CheckResult::Failed,
				title: Some("Restore failed"),
				message: "restore failed",
				detail: None,
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			},
		)
		.await
		.expect("file the other failure");

		save_ceiling(&mut conn, CheckResult::Warning).await;

		let incident = &incidents(&mut conn, s.group).await[0];
		assert!(incident.closed_at.is_none());
		assert!(incident.closing_at.is_none());
	})
	.await
}

/// A re-grade swapping which target fails keeps the incident: the failure
/// graded in joins before the one graded out lessens, so the incident never
/// stands without a failure.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn a_regrade_swapping_the_failure_keeps_the_incident() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let failing = file(&mut conn, s.applications[0], CheckResult::Failed).await;
		let warning = file(&mut conn, s.applications[1], CheckResult::Warning).await;
		let opened = incidents(&mut conn, s.group).await;
		assert_eq!(opened.len(), 1);

		save_rules(
			&mut conn,
			serde_json::json!({ "if": [
				{ "==": [{ "var": "check.result" }, "failed"] }, "warning",
				{ "==": [{ "var": "check.result" }, "warning"] }, "failed",
			] }),
		)
		.await;

		assert_eq!(
			reload(&mut conn, failing.id).await.effective_result,
			Some(CheckResult::Warning)
		);
		assert_eq!(
			reload(&mut conn, warning.id).await.effective_result,
			Some(CheckResult::Failed)
		);
		let after = incidents(&mut conn, s.group).await;
		assert_eq!(after.len(), 1, "neither closed nor reopened");
		assert!(after[0].closed_at.is_none());
		assert!(after[0].closing_at.is_none());
	})
	.await
}

/// A state filed before its grading inputs were kept is left as it is while a
/// rule reads the report's fields, which it has no record of: graded as if the
/// report had none, a rule that held its failure would stop matching.
#[tokio::test(flavor = "multi_thread")]
async fn a_state_without_its_inputs_is_left_while_a_rule_reads_the_report() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let filed = file(&mut conn, s.applications[0], CheckResult::Failed).await;
		save_rules(
			&mut conn,
			serde_json::json!({ "if": [
				{ "==": [{ "var": "status.region" }, "north"] }, "failed",
			] }),
		)
		.await;
		forget_inputs(&mut conn, filed.id).await;

		save_ceiling(&mut conn, CheckResult::Warning).await;

		assert_eq!(
			reload(&mut conn, filed.id).await.effective_result,
			Some(CheckResult::Failed)
		);
		assert!(incidents(&mut conn, s.group).await[0].closed_at.is_none());
	})
	.await
}

/// A state filed before its grading inputs were kept is graded by a rule
/// reading tags from its target's tags as they stand.
#[tokio::test(flavor = "multi_thread")]
async fn a_state_without_its_inputs_reads_its_targets_tags() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let filed = file(&mut conn, s.applications[0], CheckResult::Failed).await;
		forget_inputs(&mut conn, filed.id).await;
		conn.batch_execute(&format!(
			"UPDATE applications SET tags = '{{\"tier\": \"low\"}}' WHERE id = '{}'",
			s.applications[0]
		))
		.await
		.expect("tag the application");

		save_rules(
			&mut conn,
			serde_json::json!({ "if": [
				{ "==": [{ "var": "tag.tier" }, "low"] }, "passed",
			] }),
		)
		.await;

		assert_eq!(
			reload(&mut conn, filed.id).await.effective_result,
			Some(CheckResult::Passed)
		);
		assert!(incidents(&mut conn, s.group).await[0].closed_at.is_some());
	})
	.await
}

/// A policy change to one application type's check re-grades that type's
/// states and leaves another type's same-named check alone.
// spec: CHK#policy
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_change_leaves_another_types_check_alone() {
	TestDb::run(async |mut conn, _| {
		let central = Uuid::new_v4();
		let facility = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id) VALUES ('central', '{central}'), ('facility', '{facility}'); \
			 INSERT INTO applications (id, host, type, machine_id) VALUES \
			   ('{central}', 'https://central.example', 'tamanu-central', '{central}'), \
			   ('{facility}', 'https://facility.example', 'tamanu-facility', '{facility}'); \
			 INSERT INTO issues (application_id, source, ref, check_name, observed_result, \
			                     effective_result, message, active) VALUES \
			   ('{central}', 'alertd', 'health/disk_space', 'disk_space', 'failed', 'failed', 'low', true), \
			   ('{facility}', 'alertd', 'health/disk_space', 'disk_space', 'failed', 'failed', 'low', true)"
		))
		.await
		.expect("seed both types' states");
		for ty in [
			ApplicationType::TamanuCentral,
			ApplicationType::TamanuFacility,
		] {
			CheckPolicy::upsert_default(
				&mut conn,
				"alertd",
				&Namespace::of("alertd", Some(&ty)),
				"disk_space",
			)
			.await
			.expect("catalog row");
		}

		CheckPolicy::update(
			&mut conn,
			"alertd",
			&Namespace::of("alertd", Some(&ApplicationType::TamanuCentral)),
			"disk_space",
			CheckResult::Warning,
			false,
			None,
			"ops",
		)
		.await
		.expect("save policy");

		let effective = async |conn: &mut AsyncPgConnection, application: Uuid| {
			use database::schema::issues::dsl;
			dsl::issues
				.select(dsl::effective_result)
				.filter(dsl::application_id.eq(application))
				.first::<Option<String>>(conn)
				.await
				.expect("state")
		};
		assert_eq!(
			effective(&mut conn, central).await.as_deref(),
			Some("warning")
		);
		assert_eq!(
			effective(&mut conn, facility).await.as_deref(),
			Some("failed")
		);
	})
	.await
}

/// A policy change that closes an incident is credited to the operator who
/// made it, and one of Canopy's own checks graded out of trouble no longer
/// reads as the failure it observed.
// spec: CHK#policy, INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn a_policy_change_closing_an_incident_is_credited_to_its_operator() {
	TestDb::run(async |mut conn, _| {
		let s = seed(&mut conn).await;
		let filed = file(&mut conn, s.applications[0], CheckResult::Failed).await;
		let incident = &incidents(&mut conn, s.group).await[0];
		// Ship the open, so the close posts a resolve rather than cancelling it.
		conn.batch_execute(&format!(
			"UPDATE slack_outbox SET delivered_at = NOW() \
			 WHERE incident_id = '{}' AND kind = 'incident_open'",
			incident.id
		))
		.await
		.expect("deliver the open");

		save_ceiling(&mut conn, CheckResult::Passed).await;

		let state = reload(&mut conn, filed.id).await;
		assert_eq!(state.effective_result, Some(CheckResult::Passed));
		assert!(!state.active);
		assert_eq!(state.message, format!("Health check '{CHECK}' recovered"));
		assert!(state.description.is_none());
		assert_eq!(state.title.as_deref(), Some("Backups are stale"));

		use database::schema::slack_outbox::dsl;
		let by: Vec<serde_json::Value> = dsl::slack_outbox
			.select(dsl::payload)
			.filter(dsl::incident_id.eq(incident.id))
			.filter(dsl::kind.eq("incident_resolve"))
			.load(&mut conn)
			.await
			.expect("resolve rows");
		assert_eq!(by.len(), 1);
		assert_eq!(by[0]["by"].as_str(), Some("ops"));
	})
	.await
}
