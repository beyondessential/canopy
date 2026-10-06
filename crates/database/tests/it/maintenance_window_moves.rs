//! Retargeting a declaration and moving a window.
//!
//! A declaration can be retargeted to any grain on its starting target's line
//! of descent, and an open window can be moved along its own. A move keeps the
//! window, suspends what it newly covers at once, and leaves what it uncovers
//! settling as though the window had ended over it.
//!
//! spec: MNT

use commons_errors::AppError;
use commons_types::{server::rank::ServerRank, status::CheckResult};
use database::{
	inventory_leases::{InventoryLease, RunIntent},
	issues::{CheckFiling, Scope, file_check},
	maintenance_windows::{Amendment, Grain, MaintenanceWindow, SETTLE, line_of_descent},
	statuses::CANOPY_SOURCE,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

async fn insert_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.get_result(conn)
		.await
		.expect("insert group");
	row.id
}

async fn insert_machine(conn: &mut AsyncPgConnection, group: Option<Uuid>, name: &str) -> Uuid {
	let row: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ($1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(name)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group)
			.get_result(conn)
			.await
			.expect("insert machine");
	row.id
}

/// An application on `machine`, ranked where `rank` is given and pending
/// otherwise.
async fn insert_application(
	conn: &mut AsyncPgConnection,
	group: Option<Uuid>,
	machine: Uuid,
	rank: Option<&str>,
	name: &str,
) -> Uuid {
	let row: RowId = sql_query(
		"INSERT INTO applications (type, name, host, group_id, rank, machine_id) \
		 VALUES ('tamanu-central', $1, $2, $3, $4, $5) RETURNING id",
	)
	.bind::<sql_types::Text, _>(name)
	.bind::<sql_types::Text, _>(format!("http://{}.invalid/", Uuid::new_v4()))
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result(conn)
	.await
	.expect("insert application");
	row.id
}

/// A group with a production box and a clone box, one application on each.
struct Site {
	group: Uuid,
	production_box: Uuid,
	production_app: Uuid,
	clone_box: Uuid,
}

async fn site(conn: &mut AsyncPgConnection) -> Site {
	let group = insert_group(conn, "fiji").await;
	let production_box = insert_machine(conn, Some(group), "fj-central").await;
	let production_app = insert_application(
		conn,
		Some(group),
		production_box,
		Some("production"),
		"central",
	)
	.await;
	let clone_box = insert_machine(conn, Some(group), "fj-clone").await;
	insert_application(conn, Some(group), clone_box, Some("clone"), "clone central").await;
	Site {
		group,
		production_box,
		production_app,
		clone_box,
	}
}

fn filing(scope: Scope, check: &str) -> CheckFiling<'_> {
	CheckFiling {
		source: CANOPY_SOURCE,
		scope,
		device_id: None,
		check,
		observed: CheckResult::Failed,
		title: Some("Move test"),
		message: "move test filing",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

async fn open_incidents(conn: &mut AsyncPgConnection, group: Uuid) -> i64 {
	let row: Count = sql_query(
		"SELECT COUNT(*) AS count FROM incidents WHERE server_group_id = $1 AND closed_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(group)
	.get_result(conn)
	.await
	.expect("count open incidents");
	row.count
}

async fn outbox_count(conn: &mut AsyncPgConnection) -> i64 {
	let row: Count = sql_query(
		"SELECT COUNT(*) AS count FROM slack_outbox \
		 WHERE kind IN ('maintenance_declared', 'maintenance_ended')",
	)
	.get_result(conn)
	.await
	.expect("count outbox");
	row.count
}

async fn backdate_moves(conn: &mut AsyncPgConnection, window: Uuid, ago: SignedDuration) {
	sql_query("UPDATE maintenance_window_moves SET moved_at = $2 WHERE window_id = $1")
		.bind::<sql_types::Uuid, _>(window)
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(Timestamp::now() - ago))
		.execute(conn)
		.await
		.expect("backdate move");
}

fn in_hours(hours: i64) -> Timestamp {
	Timestamp::now() + SignedDuration::from_hours(hours)
}

async fn declare_group(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	rank: Option<ServerRank>,
) -> MaintenanceWindow {
	MaintenanceWindow::declare(
		conn,
		Scope::Group(group),
		rank,
		in_hours(1),
		Some("upgrading"),
		Some("op"),
	)
	.await
	.expect("declare")
}

fn move_to(target: Grain) -> Amendment {
	Amendment {
		target: Some(target),
		..Amendment::default()
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn an_amendment_changes_only_what_it_names() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, None).await;

		let amended = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				expected_end: Some(in_hours(3)),
				..Amendment::default()
			},
			Some("other"),
		)
		.await
		.expect("amend the end");
		assert_eq!(
			amended.note.as_deref(),
			Some("upgrading"),
			"the note was not named, so it stands"
		);
		assert!(amended.expected_end > window.expected_end);
		assert_eq!(amended.amended_by.as_deref(), Some("other"));

		let cleared = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				note: Some(None),
				..Amendment::default()
			},
			Some("other"),
		)
		.await
		.expect("clear the note");
		assert_eq!(cleared.note, None);
		assert_eq!(
			cleared.expected_end, amended.expected_end,
			"and the end was not named, so it stands"
		);
	})
	.await
}

/// A window past its expected end stays open until the sweep stamps it, and
/// extending it then is the operator saying the work ran long.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_past_its_end_but_not_yet_swept_can_be_extended() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, None).await;
		sql_query(
			"UPDATE maintenance_windows SET expected_end = NOW() - INTERVAL '1 minute' WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(window.id)
		.execute(&mut conn)
		.await
		.expect("run past its end");

		let refused = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Environment {
				group_id: site.group,
				rank: ServerRank::Clone,
			}),
			Some("op"),
		)
		.await;
		assert!(
			matches!(refused, Err(AppError::BadRequest(_))),
			"moving a window that is over without extending it is refused: {refused:?}"
		);

		let extended = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				expected_end: Some(in_hours(2)),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("extend the window that ran long");
		assert!(extended.holds_at(Timestamp::now()), "it holds again");

		MaintenanceWindow::lift(&mut conn, window.id, Some("op"))
			.await
			.expect("lift");
		let ended = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				expected_end: Some(in_hours(2)),
				..Amendment::default()
			},
			Some("op"),
		)
		.await;
		assert!(
			matches!(ended, Err(AppError::BadRequest(_))),
			"a window that has ended is history: {ended:?}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_suspends_the_new_target_now_and_settles_the_one_it_left() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, None).await;

		let moved = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Environment {
				group_id: site.group,
				rank: ServerRank::Clone,
			}),
			Some("mover"),
		)
		.await
		.expect("narrow to the clone");
		assert_eq!(moved.id, window.id, "it stays the same window");
		assert_eq!(moved.declared_by.as_deref(), Some("op"));
		assert_eq!(moved.amended_by.as_deref(), Some("mover"));
		assert_eq!(moved.rank, Some(ServerRank::Clone));

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("suspended");
		assert!(
			targets.suspends(site.clone_box, Some(site.group))
				&& !targets.settling(site.clone_box, Some(site.group)),
			"the clone is held by the window now"
		);
		assert!(
			targets.settling(site.production_box, Some(site.group)),
			"production settles as though the window had ended over it"
		);
		assert!(
			targets.group_window_settling(site.group),
			"and the settling mark is drawn at the grain the window left"
		);
		assert!(
			MaintenanceWindow::suspends(&mut conn, None, None, Some(site.group))
				.await
				.expect("suspends"),
			"the group's own checks are settling too"
		);

		backdate_moves(&mut conn, window.id, SETTLE + SignedDuration::from_mins(1)).await;
		assert!(
			!MaintenanceWindow::suspends(&mut conn, None, Some(site.production_box), None)
				.await
				.expect("suspends"),
			"production is watched again once the settle period has passed"
		);
		let (_, settled) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(settled, 1, "the move's settle period is claimed once");
		let (_, again) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(again, 0, "and not twice");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_on_what_a_move_left_alerts_once_it_has_settled() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, None).await;
		MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Environment {
				group_id: site.group,
				rank: ServerRank::Clone,
			}),
			Some("op"),
		)
		.await
		.expect("move");

		file_check(
			&mut conn,
			filing(Scope::Application(site.production_app), "reachability"),
		)
		.await
		.expect("file while settling");
		assert_eq!(
			open_incidents(&mut conn, site.group).await,
			0,
			"narrowing does not page for the rest of the group the moment it moves"
		);

		backdate_moves(&mut conn, window.id, SETTLE + SignedDuration::from_mins(1)).await;
		MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(
			open_incidents(&mut conn, site.group).await,
			1,
			"what is still failing contributes once the settle period has passed"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn moving_over_a_target_takes_its_issues_out_of_their_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		file_check(
			&mut conn,
			filing(Scope::Group(site.group), "backup-staleness"),
		)
		.await
		.expect("file group check");
		assert_eq!(open_incidents(&mut conn, site.group).await, 1);

		let window = declare_group(&mut conn, site.group, Some(ServerRank::Production)).await;
		assert_eq!(
			open_incidents(&mut conn, site.group).await,
			1,
			"production's window leaves the group's own check watched"
		);

		MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Group {
				group_id: site.group,
			}),
			Some("op"),
		)
		.await
		.expect("widen to the group");
		assert_eq!(
			open_incidents(&mut conn, site.group).await,
			0,
			"widening to the group quiets it"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_is_refused_where_it_cannot_go() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let elsewhere = insert_group(&mut conn, "tonga").await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(site.production_box),
			None,
			in_hours(1),
			None,
			Some("op"),
		)
		.await
		.expect("declare over the box");

		let sideways = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Machine {
				machine_id: site.clone_box,
			}),
			Some("op"),
		)
		.await;
		assert!(
			matches!(sideways, Err(AppError::BadRequest(_))),
			"a sibling box is not on the line of descent: {sideways:?}"
		);
		let away = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Group {
				group_id: elsewhere,
			}),
			Some("op"),
		)
		.await;
		assert!(
			matches!(away, Err(AppError::BadRequest(_))),
			"nor is another group: {away:?}"
		);

		declare_group(&mut conn, site.group, None).await;
		let occupied = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Group {
				group_id: site.group,
			}),
			Some("op"),
		)
		.await;
		assert!(
			matches!(occupied, Err(AppError::Conflict(_))),
			"a target holds at most one window: {occupied:?}"
		);

		MaintenanceWindow::lift(&mut conn, window.id, Some("op"))
			.await
			.expect("lift");
		let ended = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Environment {
				group_id: site.group,
				rank: ServerRank::Production,
			}),
			Some("op"),
		)
		.await;
		assert!(
			matches!(ended, Err(AppError::BadRequest(_))),
			"an ended window is history: {ended:?}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_a_run_is_served_against_stays_until_the_lease_is_released() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, Some(ServerRank::Production)).await;
		let lease = InventoryLease::take(
			&mut conn,
			site.group,
			ServerRank::Production,
			RunIntent::Configure,
			Some("op"),
			None,
			false,
		)
		.await
		.expect("take");

		assert!(
			window
				.fixed_because(&mut conn)
				.await
				.expect("fixed")
				.is_some(),
			"the dialog is told the window cannot move"
		);
		let refused = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Machine {
				machine_id: site.production_box,
			}),
			Some("op"),
		)
		.await;
		assert!(
			matches!(refused, Err(AppError::Conflict(_))),
			"the run is acting on the environment the window covers: {refused:?}"
		);

		InventoryLease::release(&mut conn, lease.id, Some("op"))
			.await
			.expect("release");
		MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Machine {
				machine_id: site.production_box,
			}),
			Some("op"),
		)
		.await
		.expect("moves like any other once the lease is released");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn another_operators_lease_does_not_hold_a_window_in_place() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, Some(ServerRank::Production)).await;
		InventoryLease::take(
			&mut conn,
			site.group,
			ServerRank::Production,
			RunIntent::Configure,
			Some("someone-else"),
			None,
			false,
		)
		.await
		.expect("take");

		MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Machine {
				machine_id: site.production_box,
			}),
			Some("op"),
		)
		.await
		.expect("the lease is not served against this window");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_moved_window_is_in_both_targets_histories() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(site.production_box),
			None,
			in_hours(1),
			Some("patching"),
			Some("op"),
		)
		.await
		.expect("declare");
		let moved = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Application {
				application_id: site.production_app,
			}),
			Some("op"),
		)
		.await
		.expect("narrow to the application");

		let on_box =
			MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(site.production_box), 10)
				.await
				.expect("box history");
		assert_eq!(
			on_box.len(),
			1,
			"the box keeps the window it went quiet under"
		);
		assert_eq!(on_box[0].window.id, window.id);
		assert_eq!(on_box[0].covered_from, window.declared_at);
		assert!(
			on_box[0].moved_at.is_some(),
			"over the span it covered the box"
		);
		assert_eq!(on_box[0].moved_to.as_deref(), Some("fiji central"));

		let on_app = MaintenanceWindow::list_for_scope(
			&mut conn,
			Scope::Application(site.production_app),
			10,
		)
		.await
		.expect("application history");
		assert_eq!(on_app.len(), 1);
		assert_eq!(
			on_app[0].moved_at, None,
			"it is the application's window now"
		);
		assert_eq!(Some(on_app[0].covered_from), on_box[0].moved_at);
		assert_eq!(on_app[0].window.amended_at, moved.amended_at);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_notifies_no_one() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let window = declare_group(&mut conn, site.group, None).await;
		assert_eq!(outbox_count(&mut conn).await, 1, "declaring notifies");

		MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::Environment {
				group_id: site.group,
				rank: ServerRank::Clone,
			}),
			Some("op"),
		)
		.await
		.expect("move");
		backdate_moves(&mut conn, window.id, SETTLE + SignedDuration::from_mins(1)).await;
		MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(
			outbox_count(&mut conn).await,
			1,
			"neither the move nor the end of its settle period notifies"
		);
	})
	.await
}

fn grains(descent: &database::maintenance_windows::Descent) -> Vec<(Grain, u8)> {
	descent
		.entries
		.iter()
		.map(|entry| (entry.grain, entry.depth))
		.collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_groups_line_of_descent_nests_everything_in_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let pending_box = insert_machine(&mut conn, Some(site.group), "fj-new").await;
		let pending_app =
			insert_application(&mut conn, Some(site.group), pending_box, None, "new").await;

		let descent = line_of_descent(
			&mut conn,
			Grain::Group {
				group_id: site.group,
			},
		)
		.await
		.expect("descent");
		let listed = grains(&descent);
		let group = Grain::Group {
			group_id: site.group,
		};
		let production = Grain::Environment {
			group_id: site.group,
			rank: ServerRank::Production,
		};
		let clone = Grain::Environment {
			group_id: site.group,
			rank: ServerRank::Clone,
		};
		assert_eq!(listed[0], (group, 0), "the group heads it");
		assert_eq!(listed[1], (production, 1), "production before the clone");
		assert_eq!(
			listed[2],
			(
				Grain::Machine {
					machine_id: site.production_box
				},
				2
			)
		);
		assert_eq!(
			listed[3],
			(
				Grain::Application {
					application_id: site.production_app
				},
				3
			)
		);
		assert_eq!(listed[4], (clone, 1));
		assert_eq!(
			&listed[listed.len() - 2..],
			&[
				(
					Grain::Machine {
						machine_id: pending_box
					},
					1
				),
				(
					Grain::Application {
						application_id: pending_app
					},
					2
				),
			],
			"pending machines sit under the group, apart from its environments"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_applications_line_of_descent_is_what_contains_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let descent = line_of_descent(
			&mut conn,
			Grain::Application {
				application_id: site.production_app,
			},
		)
		.await
		.expect("descent");
		assert_eq!(
			grains(&descent),
			vec![
				(
					Grain::Group {
						group_id: site.group
					},
					0
				),
				(
					Grain::Environment {
						group_id: site.group,
						rank: ServerRank::Production
					},
					1
				),
				(
					Grain::Machine {
						machine_id: site.production_box
					},
					2
				),
				(
					Grain::Application {
						application_id: site.production_app
					},
					3
				),
			],
			"its machine, the machine's environment, and its group; not the clone"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn grains_the_start_has_none_of_are_passed_over() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let lone_box = insert_machine(&mut conn, None, "lone").await;
		let lone_app =
			insert_application(&mut conn, None, lone_box, Some("production"), "lone").await;
		let descent = line_of_descent(
			&mut conn,
			Grain::Machine {
				machine_id: lone_box,
			},
		)
		.await
		.expect("descent");
		assert_eq!(
			grains(&descent),
			vec![
				(
					Grain::Machine {
						machine_id: lone_box
					},
					0
				),
				(
					Grain::Application {
						application_id: lone_app
					},
					1
				),
			],
			"a machine in no group offers its applications alone"
		);

		let group = insert_group(&mut conn, "samoa").await;
		let pending_box = insert_machine(&mut conn, Some(group), "pending").await;
		insert_application(&mut conn, Some(group), pending_box, None, "pending").await;
		let descent = line_of_descent(
			&mut conn,
			Grain::Machine {
				machine_id: pending_box,
			},
		)
		.await
		.expect("descent");
		assert_eq!(
			descent.entries[0].grain,
			Grain::Group { group_id: group },
			"a pending machine offers its group"
		);
		assert_eq!(
			descent.entries[1].depth, 1,
			"with no environment between them"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_group_covers_the_groups_own_checks() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let production = Grain::Environment {
			group_id: site.group,
			rank: ServerRank::Production,
		};
		let descent = line_of_descent(&mut conn, production)
			.await
			.expect("descent");
		let group = Grain::Group {
			group_id: site.group,
		};
		let on_app = Grain::Application {
			application_id: site.production_app,
		};

		assert!(descent.covers(group, group));
		assert!(
			!descent.covers(production, group),
			"production's window leaves the group's own checks watched"
		);
		assert!(descent.covers(production, on_app));
		assert!(descent.covers(
			Grain::Machine {
				machine_id: site.production_box
			},
			on_app
		));
		assert!(!descent.covers(
			on_app,
			Grain::Machine {
				machine_id: site.production_box
			}
		));
	})
	.await
}
