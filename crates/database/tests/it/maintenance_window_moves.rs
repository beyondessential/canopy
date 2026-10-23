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
	ansible_leases::{AnsibleLease, ReleaseReason, RunIntent, RunSpec},
	issues::{CheckFiling, Scope, file_check},
	maintenance_windows::{
		Amendment, Grain, HeldInPlace, MaintenanceWindow, SETTLE, line_of_descent,
	},
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
	clone_app: Uuid,
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
	let clone_app =
		insert_application(conn, Some(group), clone_box, Some("clone"), "clone central").await;
	Site {
		group,
		production_box,
		production_app,
		clone_box,
		clone_app,
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

/// Take the production lease as `by`, for a configuration run over `machine`.
async fn take_lease(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	machine: Uuid,
	by: &str,
) -> commons_errors::Result<Result<AnsibleLease, MaintenanceWindow>> {
	let assignments = std::collections::BTreeMap::new();
	AnsibleLease::take(
		conn,
		group,
		ServerRank::Production,
		&[machine],
		&RunSpec {
			intent: RunIntent::Configure,
			note: None,
			playbook: None,
			limit_pattern: None,
			limited_to: None,
			tags: &[],
			skip_tags: &[],
			assignments: &assignments,
		},
		Some(by),
		false,
	)
	.await
}

/// A machine with an application ranked production on it, and that application.
async fn production_machine(conn: &mut AsyncPgConnection, group: Uuid, name: &str) -> (Uuid, Uuid) {
	let machine = insert_machine(conn, Some(group), name).await;
	let application =
		insert_application(conn, Some(group), machine, Some("production"), name).await;
	(machine, application)
}

/// A lease held by `held_by` on the environment at `rank`, as a run's take would
/// leave it.
async fn insert_lease(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	rank: &str,
	held_by: &str,
) -> Uuid {
	let row: RowId = sql_query(
		"INSERT INTO ansible_leases (server_group_id, rank, intent, held_by, expires_at) \
		 VALUES ($1, $2, 'configure', $3, NOW() + INTERVAL '1 hour') RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Text, _>(rank)
	.bind::<sql_types::Text, _>(held_by)
	.get_result(conn)
	.await
	.expect("insert lease");
	row.id
}

/// Release a lease without ending the windows its run declared.
async fn release_leaving_windows(conn: &mut AsyncPgConnection, lease: Uuid) {
	sql_query("UPDATE ansible_leases SET released_at = NOW() WHERE id = $1")
		.bind::<sql_types::Uuid, _>(lease)
		.execute(conn)
		.await
		.expect("release");
}

fn held_by_op(held: Option<HeldInPlace>) -> bool {
	matches!(held, Some(HeldInPlace::RunLease { held_by }) if held_by.as_deref() == Some("op"))
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

/// An amendment changes only what it names. A window past its expected end
/// stays open until the sweep stamps it, and extending it then is the operator
/// saying the work ran long.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_past_its_end_but_not_yet_swept_can_be_extended() {
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
			"amend the end: the note was not named, so it stands"
		);
		assert!(
			amended.expected_end > window.expected_end,
			"amend the end: the end moved out"
		);
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
		assert_eq!(cleared.note, None, "clear the note: the note is gone");
		assert_eq!(
			cleared.expected_end, amended.expected_end,
			"clear the note: and the end was not named, so it stands"
		);

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
			move_to(Grain::environment(site.group, ServerRank::Clone)),
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
		assert_eq!(outbox_count(&mut conn).await, 1, "declaring notifies");

		let moved = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			move_to(Grain::environment(site.group, ServerRank::Clone)),
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
		assert!(
			!MaintenanceWindow::suspends(&mut conn, None, Some(site.production_box), None)
				.await
				.expect("suspends"),
			"production is watched again once the settle period has passed"
		);
		let (_, settled) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(settled, 1, "the move's settle period is claimed once");
		assert_eq!(
			open_incidents(&mut conn, site.group).await,
			1,
			"what is still failing contributes once the settle period has passed"
		);
		assert_eq!(
			outbox_count(&mut conn).await,
			1,
			"neither the move nor the end of its settle period notifies"
		);
		let (_, again) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(again, 0, "and not twice");
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
			move_to(Grain::group(site.group)),
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
			move_to(Grain::machine(site.clone_box)),
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
			move_to(Grain::group(elsewhere)),
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
			move_to(Grain::group(site.group)),
			Some("op"),
		)
		.await;
		assert!(
			matches!(occupied, Err(AppError::Conflict(_))),
			"a target holds at most one window: {occupied:?}"
		);

		let over_clone = MaintenanceWindow::declare(
			&mut conn,
			Scope::Application(site.clone_app),
			None,
			in_hours(1),
			None,
			Some("op"),
		)
		.await
		.expect("declare over the clone application");
		sql_query("UPDATE applications SET deleted_at = NOW() WHERE id = $1")
			.bind::<sql_types::Uuid, _>(site.clone_app)
			.execute(&mut conn)
			.await
			.expect("archive");
		let archived = MaintenanceWindow::amend(
			&mut conn,
			over_clone.id,
			move_to(Grain::machine(site.clone_box)),
			Some("op"),
		)
		.await;
		assert!(
			matches!(archived, Err(AppError::Conflict(_))),
			"an archived target's window still exists, so the move is a conflict, not a missing window: {archived:?}"
		);
		MaintenanceWindow::amend(
			&mut conn,
			over_clone.id,
			Amendment {
				expected_end: Some(in_hours(2)),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("it can still be amended");
	})
	.await
}

/// A window a run is served against stays where it is until the lease is
/// released, whatever the window covers: an environment, a whole group, a
/// machine, an application, or a set of machines. The window of an operator
/// other than the lease's holder is not held, and a group's window is held by a
/// lease on any of its environments, even where nothing is ranked in it.
// spec: ANSRUN#work-under-way, MNT#moving-a-window
#[tokio::test(flavor = "multi_thread")]
async fn a_window_a_run_is_served_against_stays_until_the_lease_is_released() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let group = site.group;
		let (p1, a1) = (site.production_box, site.production_app);
		let (p2, a2) = production_machine(&mut conn, group, "fj-two").await;
		let (p3, _) = production_machine(&mut conn, group, "fj-three").await;
		let (s1, _) = production_machine(&mut conn, group, "fj-set-one").await;
		let (s2, _) = production_machine(&mut conn, group, "fj-set-two").await;
		let production = Grain::environment(group, ServerRank::Production);

		// A set window the run declared, while nothing of the holder's covers it.
		let first = take_lease(&mut conn, group, p1, "op")
			.await
			.expect("take")
			.expect("no window refuses the first lease");
		let set = MaintenanceWindow::declare_for_run(&mut conn, &first, Some(&[s1, s2]), None)
			.await
			.expect("declare for the run")
			.declared
			.remove(0);
		assert!(set.is_machine_set());
		assert!(
			held_by_op(set.held_in_place(&mut conn).await.expect("held")),
			"the run's set: the operator interface is told why it cannot move"
		);
		for amendment in [
			Amendment {
				machines: Some(vec![s1]),
				..Amendment::default()
			},
			Amendment {
				target: Some(Grain::group(group)),
				..Amendment::default()
			},
		] {
			let refused = MaintenanceWindow::amend(&mut conn, set.id, amendment, Some("op")).await;
			assert!(
				matches!(refused, Err(AppError::Conflict(_))),
				"the run's set: {refused:?}"
			);
		}
		let set = MaintenanceWindow::amend(
			&mut conn,
			set.id,
			Amendment {
				note: Some(Some("still going".into())),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("the run's set: its note can still change");
		assert_eq!(set.lease_id, Some(first.id));
		release_leaving_windows(&mut conn, first.id).await;
		assert!(
			set.held_in_place(&mut conn).await.expect("held").is_none(),
			"the run's set: nothing is served once the lease is released"
		);

		let environment = declare_group(&mut conn, group, Some(ServerRank::Production)).await;
		let whole = declare_group(&mut conn, group, None).await;
		let machine = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(p1),
			None,
			in_hours(1),
			None,
			Some("op"),
		)
		.await
		.expect("declare over a machine");
		let application = MaintenanceWindow::declare(
			&mut conn,
			Scope::Application(a2),
			None,
			in_hours(1),
			None,
			Some("op"),
		)
		.await
		.expect("declare over an application");

		let lease = take_lease(&mut conn, group, p1, "op")
			.await
			.expect("take")
			.expect("the declarer is served against their own windows");

		let windows = [
			(
				"an environment",
				environment,
				move_to(Grain::machine(p3)),
				Grain::machine(p3),
			),
			(
				"a machine",
				machine,
				move_to(Grain::application(a1)),
				Grain::application(a1),
			),
			(
				"an application",
				application,
				move_to(Grain::machine(p2)),
				Grain::machine(p2),
			),
			(
				"a set of machines",
				set,
				Amendment {
					machines: Some(vec![s1]),
					..Amendment::default()
				},
				Grain::machine(s1),
			),
			("a whole group", whole, move_to(production), production),
		];
		for (kind, window, amendment, _) in &windows {
			assert!(
				held_by_op(window.held_in_place(&mut conn).await.expect("held")),
				"{kind}: the dialog is told the window cannot move"
			);
			let refused =
				MaintenanceWindow::amend(&mut conn, window.id, amendment.clone(), Some("op")).await;
			assert!(
				matches!(refused, Err(AppError::Conflict(_))),
				"{kind}: the run is acting on what the window covers: {refused:?}"
			);
		}

		AnsibleLease::release(&mut conn, lease.id, Some("op"), ReleaseReason::Released)
			.await
			.expect("release");
		for (kind, window, ..) in &windows {
			assert!(
				window
					.held_in_place(&mut conn)
					.await
					.expect("held")
					.is_none(),
				"{kind}: nothing is served once the lease is released"
			);
		}

		// The group's window is held by a lease on an environment nothing in the
		// group is ranked in.
		let demo = insert_lease(&mut conn, group, "demo", "op").await;
		let (_, whole, whole_move, whole_destination) = &windows[4];
		assert!(
			held_by_op(whole.held_in_place(&mut conn).await.expect("held")),
			"a whole group: the demo lease holds it in place"
		);
		let refused =
			MaintenanceWindow::amend(&mut conn, whole.id, whole_move.clone(), Some("op")).await;
		assert!(
			matches!(refused, Err(AppError::Conflict(_))),
			"a whole group: the run on the demo environment holds the group's window: {refused:?}"
		);

		let mut moved = Vec::new();
		for (kind, window, amendment, destination) in &windows[..4] {
			let after =
				MaintenanceWindow::amend(&mut conn, window.id, amendment.clone(), Some("op"))
					.await
					.unwrap_or_else(|err| {
						panic!("{kind}: moves like any other once the lease is released: {err:?}")
					});
			assert_eq!(after.grain(), Some(*destination), "{kind}: it moved");
			moved.push(after);
		}
		release_leaving_windows(&mut conn, demo).await;
		let after = MaintenanceWindow::amend(&mut conn, whole.id, whole_move.clone(), Some("op"))
			.await
			.expect("a whole group: moves once the demo lease is released");
		assert_eq!(
			after.grain(),
			Some(*whole_destination),
			"a whole group: it moved"
		);
		moved.push(after);

		// Another operator's lease is not served against the holder's windows.
		insert_lease(&mut conn, group, "production", "someone-else").await;
		for window in &moved {
			assert!(
				window
					.held_in_place(&mut conn)
					.await
					.expect("held")
					.is_none(),
				"{:?}: another operator's lease is not served against this window",
				window.grain()
			);
		}
		let back = MaintenanceWindow::amend(
			&mut conn,
			moved[1].id,
			move_to(Grain::machine(p1)),
			Some("op"),
		)
		.await
		.expect("the lease is not served against this window, so it moves");
		assert_eq!(back.grain(), Some(Grain::machine(p1)));
	})
	.await
}

/// The dialog's coverage marks and suspension are two readings of one rule, so
/// for every grain and every target in a group they agree.
#[tokio::test(flavor = "multi_thread")]
async fn coverage_marks_agree_with_suspension() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let pending_box = insert_machine(&mut conn, Some(site.group), "fj-new").await;
		let pending_app =
			insert_application(&mut conn, Some(site.group), pending_box, None, "new").await;
		let descent = line_of_descent(&mut conn, Grain::group(site.group))
			.await
			.expect("descent");

		// Each target with the arguments filing passes for an issue on it.
		let group = Some(site.group);
		let targets = [
			(Grain::group(site.group), (None, None, group)),
			(
				Grain::machine(site.production_box),
				(None, Some(site.production_box), group),
			),
			(
				Grain::application(site.production_app),
				(Some(site.production_app), Some(site.production_box), group),
			),
			(
				Grain::machine(site.clone_box),
				(None, Some(site.clone_box), group),
			),
			(
				Grain::machine(pending_box),
				(None, Some(pending_box), group),
			),
			(
				Grain::application(pending_app),
				(Some(pending_app), Some(pending_box), group),
			),
		];

		for entry in &descent.entries {
			let (scope, rank) = (entry.grain.scope(), entry.grain.rank());
			MaintenanceWindow::declare(&mut conn, scope, rank, in_hours(1), None, Some("op"))
				.await
				.expect("declare");
			for (target, (application, machine, group)) in targets {
				let suspended = MaintenanceWindow::suspends(&mut conn, application, machine, group)
					.await
					.expect("suspends");
				assert_eq!(
					descent.covers(entry.grain, target),
					suspended,
					"a window over {:?} and a check on {target:?}",
					entry.grain,
				);
			}
			sql_query("DELETE FROM maintenance_windows")
				.execute(&mut conn)
				.await
				.expect("clear");
		}
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
			move_to(Grain::application(site.production_app)),
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

/// The target's surface reads its open windows out of its history, so a window
/// still covering it is never the one a limit cuts.
#[tokio::test(flavor = "multi_thread")]
async fn history_lists_what_still_covers_the_target_first() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let own = declare_group(&mut conn, site.group, None).await;
		let clone = declare_group(&mut conn, site.group, Some(ServerRank::Clone)).await;
		MaintenanceWindow::amend(
			&mut conn,
			clone.id,
			move_to(Grain::machine(site.clone_box)),
			Some("op"),
		)
		.await
		.expect("narrow the clone's window to its box");

		let history = MaintenanceWindow::list_for_scope(&mut conn, Scope::Group(site.group), 1)
			.await
			.expect("history");
		assert_eq!(history.len(), 1);
		assert_eq!(
			history[0].window.id, own.id,
			"the group's open window comes before a span that ended more recently"
		);
		assert_eq!(history[0].ended(), None);
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

/// What a start can be retargeted to is whatever contains it and whatever it
/// contains, nested by depth, with grains the start has none of passed over.
/// The ranked box with nothing on it, the machine in no group, and the pending
/// machine each have a group of their own so they do not shift the listing
/// of the first.
// spec: GRP#environments
#[tokio::test(flavor = "multi_thread")]
async fn a_line_of_descent_nests_what_contains_and_what_is_contained() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let site = site(&mut conn).await;
		let pending_box = insert_machine(&mut conn, Some(site.group), "fj-new").await;
		let pending_app =
			insert_application(&mut conn, Some(site.group), pending_box, None, "new").await;

		let descent = line_of_descent(&mut conn, Grain::group(site.group))
			.await
			.expect("descent of a group");
		let listed = grains(&descent);
		let group = Grain::group(site.group);
		let production = Grain::environment(site.group, ServerRank::Production);
		let clone = Grain::environment(site.group, ServerRank::Clone);
		assert_eq!(listed[0], (group, 0), "group: the group heads it");
		assert_eq!(
			listed[1],
			(production, 1),
			"group: production before the clone"
		);
		assert_eq!(
			listed[2],
			(Grain::machine(site.production_box), 2),
			"group: production's machine"
		);
		assert_eq!(
			listed[3],
			(Grain::application(site.production_app), 3),
			"group: production's application"
		);
		assert_eq!(listed[4], (clone, 1), "group: the clone environment");
		assert_eq!(
			&listed[listed.len() - 2..],
			&[
				(Grain::machine(pending_box), 1),
				(Grain::application(pending_app), 2),
			],
			"group: pending machines sit under the group, apart from its environments"
		);

		let descent = line_of_descent(&mut conn, Grain::application(site.production_app))
			.await
			.expect("descent of an application");
		assert_eq!(
			grains(&descent),
			vec![
				(Grain::group(site.group), 0),
				(Grain::environment(site.group, ServerRank::Production), 1),
				(Grain::machine(site.production_box), 2),
				(Grain::application(site.production_app), 3),
			],
			"application: its machine, the machine's environment, and its group; not the clone"
		);

		let ranked_group = insert_group(&mut conn, "tonga").await;
		let fresh = insert_machine(&mut conn, Some(ranked_group), "to-new").await;
		sql_query("UPDATE machines SET rank = 'clone' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(fresh)
			.execute(&mut conn)
			.await
			.expect("rank the box");
		let descent = line_of_descent(&mut conn, Grain::machine(fresh))
			.await
			.expect("descent of a ranked box");
		assert_eq!(
			grains(&descent),
			vec![
				(Grain::group(ranked_group), 0),
				(Grain::environment(ranked_group, ServerRank::Clone), 1),
				(Grain::machine(fresh), 2),
			],
			"ranked box: already in its environment, not listed with the pending boxes"
		);

		let lone_box = insert_machine(&mut conn, None, "lone").await;
		let lone_app =
			insert_application(&mut conn, None, lone_box, Some("production"), "lone").await;
		let descent = line_of_descent(&mut conn, Grain::machine(lone_box))
			.await
			.expect("descent of a machine in no group");
		assert_eq!(
			grains(&descent),
			vec![
				(Grain::machine(lone_box), 0),
				(Grain::application(lone_app), 1),
			],
			"no group: a machine in no group offers its applications alone"
		);

		let group = insert_group(&mut conn, "samoa").await;
		let pending_box = insert_machine(&mut conn, Some(group), "pending").await;
		insert_application(&mut conn, Some(group), pending_box, None, "pending").await;
		let descent = line_of_descent(&mut conn, Grain::machine(pending_box))
			.await
			.expect("descent of a pending machine");
		assert_eq!(
			descent.entries[0].grain,
			Grain::group(group),
			"pending: a pending machine offers its group"
		);
		assert_eq!(
			descent.entries[1].depth, 1,
			"pending: with no environment between them"
		);
	})
	.await
}
