//! A change to one of Canopy's own checks' policy re-grades every state of it
//! at once, and the incidents they are in follow.
//!
//! spec: CHK#policy, INC#membership

use commons_tests::db::TestDb;
use commons_types::namespace::Namespace;
use commons_types::status::CheckResult;
use database::{
	check_policies::CheckPolicy,
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
	let applications = [Uuid::new_v4(), Uuid::new_v4()];
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
