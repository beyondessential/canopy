//! Recording where an environment is going, and what that changes: the planned
//! version becomes what pre-upgrade testing holds the data against.

use commons_tests::db::TestDb;
use commons_types::{
	server::rank::ServerRank,
	version::{VersionStatus, VersionStr},
};
use database::{
	applications::Application,
	migration_tests::candidate_for,
	reported_detail::ReportedDetail,
	server_groups::ServerGroup,
	upgrade_plans::{PlannedWhen, UpgradePlan, close_met_plans, is_late, planned_target},
	versions::{NewVersion, Version},
};
use diesel::{QueryableByName, SelectableHelper, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::civil::{date, time};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

/// An application and the machine it sits on: reported detail splits by grain,
/// so recording a push names both.
#[derive(diesel::QueryableByName)]
struct AppRow {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	#[diesel(sql_type = sql_types::Uuid)]
	machine_id: Uuid,
}

async fn publish(conn: &mut AsyncPgConnection, minor: i32, patch: i32) -> Version {
	diesel::insert_into(database::schema::versions::table)
		.values(NewVersion {
			major: 2,
			minor,
			patch,
			status: VersionStatus::Published,
			changelog: String::new(),
			device_id: None,
		})
		.returning(Version::as_returning())
		.get_result(conn)
		.await
		.expect("publish")
}

/// A group with one production central reporting `running`, and the group's
/// cached effective version recomputed through the real path.
async fn group_running(conn: &mut AsyncPgConnection, running: &str) -> (Uuid, Application) {
	let group: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('kamaka') RETURNING id")
		.get_result(conn)
		.await
		.expect("group");
	let server: AppRow = sql_query(
		"WITH m AS (INSERT INTO machines (group_id) VALUES ($2) RETURNING id) INSERT INTO applications (host, type, rank, group_id, machine_id) SELECT $1, 'tamanu-central', 'production', $2, m.id FROM m RETURNING id, machine_id",
	)
	.bind::<sql_types::Text, _>("https://central.kamaka.example")
	.bind::<sql_types::Uuid, _>(group.id)
	.get_result(conn)
	.await
	.expect("server");

	report(conn, server.id, server.machine_id, running).await;
	sql_query(
		"INSERT INTO statuses (server_id, version, healthy, health) VALUES ($1, $2, true, '[]'::jsonb)",
	)
	.bind::<sql_types::Uuid, _>(server.id)
	.bind::<sql_types::Text, _>(running)
	.execute(conn)
	.await
	.expect("status");
	ServerGroup::recompute_version(conn, group.id)
		.await
		.expect("recompute");

	let server = Application::get_by_id(conn, server.id)
		.await
		.expect("get server");
	(group.id, server)
}

/// What an application says it runs.
async fn report(conn: &mut AsyncPgConnection, application: Uuid, machine: Uuid, running: &str) {
	let version: VersionStr = running.parse().expect("parse");
	ReportedDetail::record(
		conn,
		Some(application),
		machine,
		"test",
		&serde_json::json!({}),
		Some(&version),
	)
	.await
	.expect("report");
}

/// Another member of `group` at `rank`, reporting `running`.
async fn server_at(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	rank: ServerRank,
	running: &str,
) -> Application {
	let server: AppRow = sql_query(
		"WITH m AS (INSERT INTO machines (group_id) VALUES ($3) RETURNING id) INSERT INTO applications (host, type, rank, group_id, machine_id) SELECT $1, 'tamanu-central', $2, $3, m.id FROM m RETURNING id, machine_id",
	)
	.bind::<sql_types::Text, _>(format!("https://{rank}.kamaka.example"))
	.bind::<sql_types::Text, _>(rank.to_string())
	.bind::<sql_types::Uuid, _>(group)
	.get_result(conn)
	.await
	.expect("server");
	report(conn, server.id, server.machine_id, running).await;
	Application::get_by_id(conn, server.id)
		.await
		.expect("get server")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_is_what_makes_a_version_the_test_target() {
	TestDb::run(|mut conn, _url| async move {
		let (group, server) = group_running(&mut conn, "2.60.0").await;
		let intended = publish(&mut conn, 61, 2).await;
		publish(&mut conn, 63, 0).await;

		assert_eq!(
			candidate_for(&mut conn, &server)
				.await
				.expect("candidate")
				.map(|v| v.id),
			None,
			"a newer version existing is not a reason to spend a restore on it"
		);

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			intended.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				..Default::default()
			},
			Some("site can absorb 2.61 only"),
			"someone@example.com",
		)
		.await
		.expect("record plan");

		assert_eq!(
			candidate_for(&mut conn, &server)
				.await
				.expect("candidate")
				.map(|v| v.id),
			Some(intended.id),
			"testing aims where the group says it is going"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn recording_a_plan_retires_the_one_before_it() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let first = publish(&mut conn, 61, 0).await;
		let second = publish(&mut conn, 62, 0).await;

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			first.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("first plan");
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			second.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("second plan");

		let open = UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Production)
			.await
			.expect("open")
			.expect("there is one");
		assert_eq!(
			open.target_version_id, second.id,
			"a group goes one place next"
		);

		let history = UpgradePlan::history_for_group(&mut conn, group)
			.await
			.expect("history");
		assert_eq!(history.len(), 2, "the retired plan is kept");
		let retired = history
			.iter()
			.find(|plan| plan.target_version_id == first.id)
			.expect("the first plan");
		assert!(retired.superseded_at.is_some());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_cannot_aim_at_where_the_group_already_is() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.62.0").await;
		let behind = publish(&mut conn, 61, 0).await;

		let refused = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			behind.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await;
		assert!(refused.is_err(), "a plan to go backwards is not a plan");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn canopy_closes_a_plan_once_the_group_arrives() {
	TestDb::run(|mut conn, _url| async move {
		let (group, server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		assert_eq!(
			close_met_plans(&mut conn).await.expect("sweep"),
			0,
			"still on its way"
		);

		// The environment lands past the target: further than planned still
		// means the upgrade happened.
		report(&mut conn, server.id, server.machine_id, "2.62.0").await;

		// Through the periodic sweep, which is what runs in production.
		database::backup::sweep(&mut conn).await.expect("sweep");
		assert!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Production)
				.await
				.expect("open")
				.is_none(),
			"a met plan is closed, not left outstanding"
		);
		assert!(
			planned_target(&mut conn, group, ServerRank::Production)
				.await
				.expect("target")
				.is_none(),
			"and stops steering the test target"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_date_that_has_passed_reads_as_late() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;
		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 7, 1)),
				..Default::default()
			},
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		assert!(is_late(&plan, date(2026, 7, 30)));
		assert!(!is_late(&plan, date(2026, 6, 30)));

		let undated = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("undated plan");
		assert!(
			!is_late(&undated, date(2026, 7, 30)),
			"no date means nothing to be late against"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn amending_a_plan_keeps_it_the_same_plan() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 7, 1)),
				..Default::default()
			},
			Some("waiting on the site"),
			"a@example.com",
		)
		.await
		.expect("plan");

		let amended = UpgradePlan::amend(
			&mut conn,
			plan.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				..Default::default()
			},
			Some("site confirmed the window"),
			"b@example.com",
		)
		.await
		.expect("amend");

		assert_eq!(amended.id, plan.id, "the same plan, better described");
		assert_eq!(
			amended.target_version_id, target.id,
			"the target is untouched"
		);
		assert_eq!(amended.planned_for, Some(date(2026, 8, 14)));
		assert_eq!(amended.note.as_deref(), Some("site confirmed the window"));
		assert_eq!(amended.amended_by.as_deref(), Some("b@example.com"));
		assert!(amended.amended_at.is_some());
		assert_eq!(
			amended.created_by.as_deref(),
			Some("a@example.com"),
			"who recorded it is not overwritten by who amended it"
		);
		assert!(
			amended.superseded_at.is_none(),
			"an amendment is not a replacement"
		);

		let history = UpgradePlan::history_for_group(&mut conn, group)
			.await
			.expect("history");
		assert_eq!(history.len(), 1, "amending does not add to the history");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn amending_can_clear_the_date_and_note() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 7, 1)),
				..Default::default()
			},
			Some("waiting on the site"),
			"a@example.com",
		)
		.await
		.expect("plan");

		let amended = UpgradePlan::amend(
			&mut conn,
			plan.id,
			PlannedWhen::default(),
			None,
			"b@example.com",
		)
		.await
		.expect("amend");

		assert!(
			amended.planned_for.is_none(),
			"a date that is no longer expected can be taken off"
		);
		assert!(amended.note.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_replaced_plan_is_not_amendable() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let first = publish(&mut conn, 61, 0).await;
		let second = publish(&mut conn, 62, 0).await;

		let replaced = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			first.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("first plan");
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			second.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("second plan");

		let refused = UpgradePlan::amend(
			&mut conn,
			replaced.id,
			PlannedWhen {
				date: Some(date(2026, 8, 1)),
				..Default::default()
			},
			None,
			"b@example.com",
		)
		.await;
		assert!(
			refused.is_err(),
			"a replaced plan is history and stands as it was"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_met_plan_is_not_amendable() {
	TestDb::run(|mut conn, _url| async move {
		let (group, server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;
		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		report(&mut conn, server.id, server.machine_id, "2.61.0").await;
		close_met_plans(&mut conn).await.expect("sweep");

		let refused = UpgradePlan::amend(
			&mut conn,
			plan.id,
			PlannedWhen {
				date: Some(date(2026, 8, 1)),
				..Default::default()
			},
			None,
			"b@example.com",
		)
		.await;
		assert!(
			refused.is_err(),
			"the upgrade happened; the record of it stands as it was"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_plan_leaves_the_group_unplanned_but_stays_in_its_history() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;
		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		let withdrawn = UpgradePlan::withdraw(&mut conn, plan.id, "b@example.com")
			.await
			.expect("withdraw")
			.expect("the open plan");
		assert_eq!(withdrawn.withdrawn_by.as_deref(), Some("b@example.com"));
		assert!(withdrawn.withdrawn_at.is_some());
		assert!(
			withdrawn.met_at.is_none(),
			"withdrawing does not say the upgrade happened"
		);

		assert!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Production)
				.await
				.expect("open")
				.is_none(),
			"the group is no longer going anywhere"
		);
		assert!(
			planned_target(&mut conn, group, ServerRank::Production)
				.await
				.expect("target")
				.is_none(),
			"testing falls back to the newest published version"
		);

		let history = UpgradePlan::history_for_group(&mut conn, group)
			.await
			.expect("history");
		assert_eq!(history.len(), 1, "the plan is retained");
		assert_eq!(history[0].id, plan.id);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_plan_does_not_hold_the_group_s_open_slot() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let first = publish(&mut conn, 61, 0).await;
		let second = publish(&mut conn, 62, 0).await;

		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			first.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("first plan");
		UpgradePlan::withdraw(&mut conn, plan.id, "b@example.com")
			.await
			.expect("withdraw");

		let replacement = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			second.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("a group that withdrew a plan can record another");
		assert_eq!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Production)
				.await
				.expect("open")
				.map(|p| p.id),
			Some(replacement.id)
		);

		let withdrawn = UpgradePlan::history_for_group(&mut conn, group)
			.await
			.expect("history")
			.into_iter()
			.find(|p| p.id == plan.id)
			.expect("the withdrawn plan is still history");
		assert!(
			withdrawn.superseded_at.is_none(),
			"it was withdrawn, not replaced"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_plan_is_not_amendable_or_withdrawable_twice() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;
		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("plan");
		UpgradePlan::withdraw(&mut conn, plan.id, "b@example.com")
			.await
			.expect("withdraw");

		assert!(
			UpgradePlan::amend(
				&mut conn,
				plan.id,
				PlannedWhen {
					date: Some(date(2026, 8, 1)),
					..Default::default()
				},
				None,
				"c@example.com"
			)
			.await
			.is_err(),
			"a withdrawn plan is history and stands as it was"
		);
		assert!(
			UpgradePlan::withdraw(&mut conn, plan.id, "c@example.com")
				.await
				.expect("second withdraw")
				.is_none(),
			"withdrawing again changes nothing and does not restamp who withdrew it"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_can_carry_the_hour_it_starts() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 20)),
				time: Some(time(0, 0, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
				..Default::default()
			},
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		assert_eq!(plan.planned_time, Some(time(0, 0, 0, 0)));
		assert_eq!(plan.planned_zone.as_deref(), Some("Pacific/Fiji"));

		let amended = UpgradePlan::amend(
			&mut conn,
			plan.id,
			PlannedWhen {
				date: Some(date(2026, 8, 20)),
				..Default::default()
			},
			None,
			"b@example.com",
		)
		.await
		.expect("amend");

		assert!(
			amended.planned_time.is_none() && amended.planned_zone.is_none(),
			"an hour that is no longer settled can be taken off, leaving the day"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_qualifies_the_hour_it_opens() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 20)),
				time: Some(time(22, 0, 0, 0)),
				end: Some(time(2, 0, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
			},
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		assert_eq!(
			plan.planned_end_time,
			Some(time(2, 0, 0, 0)),
			"a window closing earlier in the day than it opened is the next morning"
		);

		let mut refuses = async |when| {
			UpgradePlan::record(
				&mut conn,
				group,
				ServerRank::Production,
				target.id,
				when,
				None,
				"a@example.com",
			)
			.await
		};

		assert!(
			refuses(PlannedWhen {
				date: Some(date(2026, 8, 20)),
				end: Some(time(2, 0, 0, 0)),
				..Default::default()
			})
			.await
			.is_err(),
			"a close with no open bounds nothing"
		);
		assert!(
			refuses(PlannedWhen {
				date: Some(date(2026, 8, 20)),
				time: Some(time(22, 0, 0, 0)),
				end: Some(time(22, 0, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
			})
			.await
			.is_err(),
			"closing at the hour it opens reads as either no time at all or a whole day"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_hour_nobody_can_read_is_refused() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		let mut refuses = async |when| {
			UpgradePlan::record(
				&mut conn,
				group,
				ServerRank::Production,
				target.id,
				when,
				None,
				"a@example.com",
			)
			.await
		};

		assert!(
			refuses(PlannedWhen {
				date: Some(date(2026, 8, 20)),
				time: Some(time(19, 30, 0, 0)),
				zone: None,
				..Default::default()
			})
			.await
			.is_err(),
			"a wall clock without a zone is readable only by whoever typed it"
		);
		assert!(
			refuses(PlannedWhen {
				date: None,
				time: Some(time(19, 30, 0, 0)),
				zone: Some("Pacific/Nauru".into()),
				..Default::default()
			})
			.await
			.is_err(),
			"an hour qualifies a day, so it needs one"
		);
		assert!(
			refuses(PlannedWhen {
				date: Some(date(2026, 8, 20)),
				time: Some(time(19, 30, 0, 0)),
				zone: Some("Pacific/Atlantis".into()),
				..Default::default()
			})
			.await
			.is_err(),
			"a zone nothing can resolve is not a zone"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn only_dated_plans_that_still_stand_reach_the_calendar() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _server) = group_running(&mut conn, "2.60.0").await;
		let first = publish(&mut conn, 61, 0).await;
		let second = publish(&mut conn, 62, 0).await;

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			first.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				..Default::default()
			},
			None,
			"a@example.com",
		)
		.await
		.expect("first plan");

		let open = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			second.id,
			PlannedWhen {
				date: Some(date(2026, 9, 2)),
				time: Some(time(22, 0, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
				..Default::default()
			},
			None,
			"a@example.com",
		)
		.await
		.expect("second plan");

		let dated = UpgradePlan::dated(&mut conn).await.expect("dated");
		assert_eq!(
			dated.iter().map(|plan| plan.id).collect::<Vec<_>>(),
			vec![open.id],
			"a replaced plan leaves the calendar"
		);

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			second.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("undated plan");
		assert!(
			UpgradePlan::dated(&mut conn)
				.await
				.expect("dated")
				.is_empty(),
			"a plan with no day has nowhere to sit on a calendar"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn each_environment_goes_its_own_place() {
	TestDb::run(|mut conn, _url| async move {
		let (group, production) = group_running(&mut conn, "2.60.0").await;
		let clone = server_at(&mut conn, group, ServerRank::Clone, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		// The clone rehearses the upgrade first.
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Clone,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("clone plan");

		assert_eq!(
			candidate_for(&mut conn, &clone)
				.await
				.expect("candidate")
				.map(|v| v.id),
			Some(target.id),
			"the clone is tested against its own plan"
		);
		assert_eq!(
			candidate_for(&mut conn, &production)
				.await
				.expect("candidate")
				.map(|v| v.id),
			None,
			"production has said nothing about where it is going"
		);

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("production plan");
		assert!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Clone)
				.await
				.expect("open")
				.is_some(),
			"a plan for production leaves the clone's where it was"
		);

		// The clone arrives; production has not moved.
		report(&mut conn, clone.id, clone.machine_id, "2.61.0").await;
		close_met_plans(&mut conn).await.expect("sweep");
		assert!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Clone)
				.await
				.expect("open")
				.is_none(),
			"the clone's plan is met"
		);
		assert!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Production)
				.await
				.expect("open")
				.is_some(),
			"a clone arriving says nothing about its production"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_with_no_rank_follows_the_headline_environment() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _production) = group_running(&mut conn, "2.60.0").await;
		let unranked: AppRow = sql_query(
			"WITH m AS (INSERT INTO machines (group_id) VALUES ($1) RETURNING id) INSERT INTO applications (host, type, group_id, machine_id) SELECT 'https://x.kamaka.example', 'tamanu-facility', $1, m.id FROM m RETURNING id, machine_id",
		)
		.bind::<sql_types::Uuid, _>(group)
		.get_result(&mut conn)
		.await
		.expect("application");
		let unranked = Application::get_by_id(&mut conn, unranked.id)
			.await
			.expect("get application");
		let target = publish(&mut conn, 61, 0).await;

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("plan");

		assert_eq!(
			candidate_for(&mut conn, &unranked)
				.await
				.expect("candidate")
				.map(|v| v.id),
			Some(target.id),
			"an unranked facility is tested with the production it belongs to"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_needs_an_environment_the_group_has() {
	TestDb::run(|mut conn, _url| async move {
		let (group, _production) = group_running(&mut conn, "2.60.0").await;
		let target = publish(&mut conn, 61, 0).await;

		let refused = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Demo,
			target.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await;
		assert!(refused.is_err(), "the group has no demo applications");
	})
	.await
}

/// A group with two version-tracked members at different ranks and different
/// versions. The production central carries the headline; the test facility
/// sits behind it.
async fn group_with_a_member_behind(conn: &mut AsyncPgConnection) -> Uuid {
	let group: RowId =
		sql_query("INSERT INTO server_groups (name) VALUES ('kirimati') RETURNING id")
			.get_result(conn)
			.await
			.expect("group");

	for (r#type, rank, running) in [
		("tamanu-central", "production", "2.62.0"),
		("tamanu-facility", "test", "2.58.0"),
	] {
		let host = format!("https://{}.example", Uuid::new_v4());
		let app: AppRow = sql_query(
			"WITH m AS (INSERT INTO machines (group_id) VALUES ($4) RETURNING id) INSERT INTO applications (host, type, rank, group_id, machine_id) SELECT $1, $2, $3, $4, m.id FROM m RETURNING id, machine_id",
		)
		.bind::<sql_types::Text, _>(host)
		.bind::<sql_types::Text, _>(r#type)
		.bind::<sql_types::Text, _>(rank)
		.bind::<sql_types::Uuid, _>(group.id)
		.get_result(conn)
		.await
		.expect("application");

		report(conn, app.id, app.machine_id, running).await;
		sql_query(
			"INSERT INTO statuses (server_id, version, healthy, health) VALUES ($1, $2, true, '[]'::jsonb)",
		)
		.bind::<sql_types::Uuid, _>(app.id)
		.bind::<sql_types::Text, _>(running)
		.execute(conn)
		.await
		.expect("status");
	}

	ServerGroup::recompute_version(conn, group.id)
		.await
		.expect("recompute");
	group.id
}

/// A plan is measured against the version its own environment presents, which
/// is that environment's central's. Otherwise a group whose facility lags would
/// accept a plan to go somewhere its central has already been.
// spec: APP#versions
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_measures_from_its_environments_version() {
	TestDb::run(|mut conn, _url| async move {
		let group = group_with_a_member_behind(&mut conn).await;

		use database::schema::server_groups::dsl;
		use diesel::{ExpressionMethods, QueryDsl};
		let headline: Option<String> = dsl::server_groups
			.select(dsl::effective_version)
			.filter(dsl::id.eq(group))
			.first(&mut conn)
			.await
			.expect("headline");
		assert_eq!(
			headline.as_deref(),
			Some("2.62.0"),
			"the headline is the highest-ranked central's version"
		);

		// Ahead of the lagging facility, behind the production central: not a
		// plan for production.
		let behind = publish(&mut conn, 61, 0).await;
		assert!(
			UpgradePlan::record(
				&mut conn,
				group,
				ServerRank::Production,
				behind.id,
				PlannedWhen::default(),
				None,
				"a@example.com",
			)
			.await
			.is_err(),
			"a member being behind is not a reason to plan backwards"
		);

		// Ahead of it: a plan.
		let ahead = publish(&mut conn, 63, 0).await;
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			ahead.id,
			PlannedWhen::default(),
			None,
			"a@example.com",
		)
		.await
		.expect("record plan");
		assert_eq!(
			UpgradePlan::open_for_environment(&mut conn, group, ServerRank::Production)
				.await
				.expect("open")
				.expect("there is one")
				.target_version_id,
			ahead.id,
		);
	})
	.await
}
