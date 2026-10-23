//! A window over several machines of one environment.
//!
//! It covers each machine as a window over that machine alone would, is the
//! window of each machine it names, and is never its environment's own window.
//! Adding machines suspends them from the amendment; removing them leaves them
//! settling as though the window had ended over them.
//!
//! spec: MNT

use commons_errors::AppError;
use commons_types::{server::rank::ServerRank, status::CheckResult};
use database::{
	issues::{CheckFiling, Scope, file_check},
	machines::Machine,
	maintenance_windows::{
		Amendment, Cover, Grain, MaintenanceWindow, SETTLE, line_of_descent_for,
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

/// A machine in `group` with one application on it, at `rank`.
async fn insert_box(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	rank: &str,
	name: &str,
) -> (Uuid, Uuid) {
	let machine: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ($1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(name)
			.bind::<sql_types::Uuid, _>(group)
			.get_result(conn)
			.await
			.expect("insert machine");
	let application: RowId = sql_query(
		"INSERT INTO applications (type, name, host, group_id, rank, machine_id) \
		 VALUES ('tamanu-central', $1, $2, $3, $4, $5) RETURNING id",
	)
	.bind::<sql_types::Text, _>(name)
	.bind::<sql_types::Text, _>(format!("http://{}.invalid/", Uuid::new_v4()))
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Text, _>(rank)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert application");
	(machine.id, application.id)
}

/// A machine in `group` that no application has ranked, so it is in no
/// environment.
async fn insert_pending(conn: &mut AsyncPgConnection, group: Uuid, name: &str) -> Uuid {
	let machine: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ($1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(name)
			.bind::<sql_types::Uuid, _>(group)
			.get_result(conn)
			.await
			.expect("insert machine");
	machine.id
}

/// A group whose production environment is three boxes, with a clone box.
struct Fleet {
	group: Uuid,
	boxes: [Uuid; 3],
	apps: [Uuid; 3],
	clone_box: Uuid,
}

async fn fleet(conn: &mut AsyncPgConnection) -> Fleet {
	let group = insert_group(conn, "fiji").await;
	let (a, app_a) = insert_box(conn, group, "production", "fj-a").await;
	let (b, app_b) = insert_box(conn, group, "production", "fj-b").await;
	let (c, app_c) = insert_box(conn, group, "production", "fj-c").await;
	let (clone_box, _) = insert_box(conn, group, "clone", "fj-clone").await;
	Fleet {
		group,
		boxes: [a, b, c],
		apps: [app_a, app_b, app_c],
		clone_box,
	}
}

fn in_hours(hours: i64) -> Timestamp {
	Timestamp::now() + SignedDuration::from_hours(hours)
}

async fn declare_machines(
	conn: &mut AsyncPgConnection,
	machines: &[Uuid],
) -> Result<MaintenanceWindow, AppError> {
	MaintenanceWindow::declare_machines(conn, machines, in_hours(1), Some("patching"), Some("op"))
		.await
}

fn machines(machines: &[Uuid]) -> Amendment {
	Amendment {
		machines: Some(machines.to_vec()),
		..Amendment::default()
	}
}

async fn machine_suspended(conn: &mut AsyncPgConnection, group: Uuid, machine: Uuid) -> bool {
	MaintenanceWindow::suspends(conn, None, Some(machine), Some(group))
		.await
		.expect("suspends")
}

fn filing(scope: Scope) -> CheckFiling<'static> {
	CheckFiling {
		source: CANOPY_SOURCE,
		scope,
		device_id: None,
		check: "reachability",
		observed: CheckResult::Failed,
		title: Some("Set test"),
		message: "set test filing",
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

async fn backdate_moves(conn: &mut AsyncPgConnection, window: Uuid, ago: SignedDuration) {
	sql_query("UPDATE maintenance_window_moves SET moved_at = $2 WHERE window_id = $1")
		.bind::<sql_types::Uuid, _>(window)
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(Timestamp::now() - ago))
		.execute(conn)
		.await
		.expect("backdate moves");
}

/// A window over several machines covers each as one over it alone would. It
/// is not its environment's own window: a take reads the windows that name any
/// of the environment's machines, so another operator's set refuses it and one
/// over machines in another environment does not.
// spec: ANSRUN#work-under-way
#[tokio::test(flavor = "multi_thread")]
async fn a_window_over_several_machines_covers_each_and_is_not_its_environments_own() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		let set = declare_machines(&mut conn, &[b, a]).await.expect("declare");

		let mut named = vec![a, b];
		named.sort();
		assert_eq!(set.machine_ids, named, "it names its machines in order");
		assert_eq!(set.machine_id, None);
		assert_eq!(set.server_group_id, Some(fleet.group));
		assert_eq!(set.rank, Some(ServerRank::Production));
		assert_eq!(set.grain(), None, "it is not any one grain");

		for machine in [a, b] {
			assert!(machine_suspended(&mut conn, fleet.group, machine).await);
		}
		assert!(
			!machine_suspended(&mut conn, fleet.group, c).await,
			"the rest of the environment stays watched"
		);
		assert!(
			!machine_suspended(&mut conn, fleet.group, fleet.clone_box).await,
			"and so does the other environment"
		);
		assert!(
			MaintenanceWindow::suspends(&mut conn, Some(fleet.apps[0]), Some(a), Some(fleet.group))
				.await
				.expect("suspends"),
			"an application on a named machine is covered with it"
		);
		assert!(
			!MaintenanceWindow::suspends(&mut conn, None, None, Some(fleet.group))
				.await
				.expect("suspends"),
			"the group's own checks are not"
		);

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("targets");
		for machine in [a, b] {
			assert!(
				targets.suspends(machine, Some(fleet.group))
					&& !targets.settling(machine, Some(fleet.group))
					&& targets.machine_window(machine),
				"each named machine is held by a window of its own"
			);
		}
		assert!(!targets.suspends(c, Some(fleet.group)));
		assert!(
			!targets.environment_window(fleet.group, ServerRank::Production),
			"the environment itself is not marked"
		);

		let now = Timestamp::now();
		let sole = MaintenanceWindow::open_over(
			&mut conn,
			fleet.group,
			ServerRank::Production,
			&fleet.boxes,
		)
		.await
		.expect("open over");
		assert!(
			MaintenanceWindow::refusing_run(&sole, "someone-else", now).is_some(),
			"another operator's window holds the environment"
		);
		assert!(
			MaintenanceWindow::refusing_run(&sole, "op", now).is_none(),
			"the declarer is served"
		);

		assert!(
			MaintenanceWindow::open_for(
				&mut conn,
				Scope::Group(fleet.group),
				Some(ServerRank::Production)
			)
			.await
			.expect("open")
			.is_none(),
			"the environment has no window of its own"
		);
		let own = MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(fleet.group),
			Some(ServerRank::Production),
			in_hours(1),
			None,
			Some("other"),
		)
		.await
		.expect("declare over the environment");
		assert_ne!(
			own.id, set.id,
			"it opens a window rather than amending the set"
		);
		assert_eq!(own.amended_at, None);

		let over = MaintenanceWindow::open_over(
			&mut conn,
			fleet.group,
			ServerRank::Production,
			&fleet.boxes,
		)
		.await
		.expect("open over");
		assert_eq!(over.len(), 2, "lease taking reads both");

		let named = MaintenanceWindow::open_over(&mut conn, fleet.group, ServerRank::Clone, &[a])
			.await
			.expect("open over");
		assert!(
			named.iter().any(|window| window.id == set.id),
			"a set naming any of the machines is returned"
		);
		let elsewhere = MaintenanceWindow::open_over(
			&mut conn,
			fleet.group,
			ServerRank::Clone,
			&[fleet.clone_box],
		)
		.await
		.expect("open over");
		assert!(
			elsewhere.iter().all(|window| window.id != set.id),
			"a set naming none of them is not"
		);
		assert!(
			MaintenanceWindow::refusing_run(&elsewhere, "someone-else", now).is_none(),
			"a window over machines of another environment refuses nothing"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_is_named_by_at_most_one_open_window() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		let other_group = insert_group(&mut conn, "samoa").await;
		let (elsewhere, _) = insert_box(&mut conn, other_group, "production", "ws-a").await;

		for machines in [vec![a, fleet.clone_box], vec![a, elsewhere]] {
			let refused = declare_machines(&mut conn, &machines).await;
			assert!(
				matches!(refused, Err(AppError::BadRequest(_))),
				"machines of one environment only, {machines:?}: {refused:?}"
			);
		}
		let none = declare_machines(&mut conn, &[]).await;
		assert!(matches!(none, Err(AppError::BadRequest(_))), "{none:?}");
		let missing = declare_machines(&mut conn, &[a, Uuid::new_v4()]).await;
		assert!(matches!(missing, Err(AppError::NotFound(_))), "{missing:?}");

		let (gone, _) = insert_box(&mut conn, fleet.group, "production", "fj-gone").await;
		Machine::archive(&mut conn, gone).await.expect("archive");
		let archived = declare_machines(&mut conn, &[a, gone]).await;
		assert!(
			matches!(&archived, Err(AppError::BadRequest(message)) if message.contains("in no environment")),
			"an archived machine serves no environment: {archived:?}"
		);
		let pending = insert_pending(&mut conn, fleet.group, "fj-new").await;
		let unranked = declare_machines(&mut conn, &[a, pending]).await;
		assert!(
			matches!(&unranked, Err(AppError::BadRequest(message)) if message.contains("in no environment")),
			"a pending machine serves no environment: {unranked:?}"
		);
		let past = MaintenanceWindow::declare_machines(
			&mut conn,
			&[a, b],
			Timestamp::now() - SignedDuration::from_mins(1),
			None,
			Some("op"),
		)
		.await;
		assert!(
			matches!(past, Err(AppError::BadRequest(_))),
			"a window ends in the future: {past:?}"
		);

		let alone = declare_machines(&mut conn, &[a, a])
			.await
			.expect("one machine");
		assert_eq!(
			alone.machine_id,
			Some(a),
			"one machine is that machine's window"
		);
		assert!(alone.machine_ids.is_empty());

		let refused = declare_machines(&mut conn, &[a, b]).await;
		assert!(
			matches!(refused, Err(AppError::Conflict(_))),
			"a machine another window names cannot be named again: {refused:?}"
		);

		let set = declare_machines(&mut conn, &[b, c]).await.expect("declare");
		let refused = declare_machines(&mut conn, &[a, c]).await;
		assert!(matches!(refused, Err(AppError::Conflict(_))), "{refused:?}");

		let amended = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(b),
			None,
			in_hours(2),
			Some("longer"),
			Some("other"),
		)
		.await
		.expect("declare over a machine a set names");
		assert_eq!(
			amended.id, set.id,
			"the set is the machine's window, so it is amended"
		);
		assert_eq!(amended.amended_by.as_deref(), Some("other"));
		assert_eq!(
			amended.machine_ids, set.machine_ids,
			"and keeps its machines"
		);

		let again = declare_machines(&mut conn, &[c, b])
			.await
			.expect("exactly its machines");
		assert_eq!(
			again.id, set.id,
			"declaring over exactly a set's machines amends it"
		);

		assert_eq!(
			MaintenanceWindow::open_for(&mut conn, Scope::Machine(c), None)
				.await
				.expect("open")
				.map(|window| window.id),
			Some(set.id)
		);
		assert_eq!(
			MaintenanceWindow::open_for(&mut conn, Scope::Machine(a), None)
				.await
				.expect("open")
				.map(|window| window.id),
			Some(alone.id)
		);

		let moved_into =
			MaintenanceWindow::amend(&mut conn, alone.id, machines(&[a, b]), Some("op")).await;
		assert!(
			matches!(moved_into, Err(AppError::Conflict(_))),
			"nor can an amendment name a machine another window names: {moved_into:?}"
		);

		MaintenanceWindow::lift(&mut conn, set.id, Some("op"))
			.await
			.expect("lift the set");
		for amendment in [
			machines(&[b, c]),
			Amendment {
				expected_end: Some(in_hours(3)),
				..Amendment::default()
			},
		] {
			let ended = MaintenanceWindow::amend(&mut conn, set.id, amendment, Some("op")).await;
			assert!(
				matches!(ended, Err(AppError::BadRequest(_))),
				"a set that has been lifted is history: {ended:?}"
			);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_a_set_stops_covering_settles_as_though_the_window_ended() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		file_check(&mut conn, filing(Scope::Machine(c)))
			.await
			.expect("file");
		assert_eq!(open_incidents(&mut conn, fleet.group).await, 1);

		let set = declare_machines(&mut conn, &[a, b]).await.expect("declare");
		assert_eq!(
			open_incidents(&mut conn, fleet.group).await,
			1,
			"c is not covered, so its incident stands"
		);

		let widened =
			MaintenanceWindow::amend(&mut conn, set.id, machines(&[a, b, c]), Some("other"))
				.await
				.expect("add c");
		assert_eq!(widened.machine_ids.len(), 3);
		assert_eq!(widened.amended_by.as_deref(), Some("other"));
		assert!(machine_suspended(&mut conn, fleet.group, c).await);
		assert_eq!(
			open_incidents(&mut conn, fleet.group).await,
			0,
			"its open issues are re-evaluated at once"
		);
		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("targets");
		assert!(
			!targets.settling(c, Some(fleet.group)),
			"it is held, not settling"
		);
		file_check(&mut conn, filing(Scope::Machine(c)))
			.await
			.expect("file under the window");
		assert_eq!(open_incidents(&mut conn, fleet.group).await, 0);

		let narrowed = MaintenanceWindow::amend(&mut conn, set.id, machines(&[a, b]), Some("op"))
			.await
			.expect("remove c");
		assert_eq!(narrowed.machine_ids.len(), 2);

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("targets");
		assert!(
			targets.settling(c, Some(fleet.group)),
			"c serves the settle period"
		);
		assert!(
			targets.suspends(a, Some(fleet.group)) && !targets.settling(a, Some(fleet.group)),
			"the machines it still covers are held"
		);
		assert!(machine_suspended(&mut conn, fleet.group, c).await);
		assert_eq!(
			open_incidents(&mut conn, fleet.group).await,
			0,
			"c does not page the moment it is dropped"
		);

		backdate_moves(&mut conn, set.id, SETTLE + SignedDuration::from_mins(1)).await;
		assert!(!machine_suspended(&mut conn, fleet.group, c).await);
		MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(
			open_incidents(&mut conn, fleet.group).await,
			1,
			"what is still failing contributes once the settle period has passed"
		);

		let left = MaintenanceWindow::amend(&mut conn, set.id, machines(&[a]), Some("op"))
			.await
			.expect("remove a machine, leaving one");
		assert_eq!(left.id, set.id, "it is the same window");
		assert_eq!(left.machine_id, Some(a));
		assert!(left.machine_ids.is_empty());
		assert_eq!(
			left.server_group_id, None,
			"a machine's window sits at the machine"
		);
		assert_eq!(left.rank, None);
		assert_eq!(left.grain(), Some(Grain::machine(a)));
		assert_eq!(
			MaintenanceWindow::open_for(&mut conn, Scope::Machine(a), None)
				.await
				.expect("open")
				.map(|window| window.id),
			Some(set.id)
		);

		let grown = MaintenanceWindow::amend(&mut conn, set.id, machines(&[a, b]), Some("op"))
			.await
			.expect("add one back");
		assert_eq!(grown.machine_id, None);
		assert_eq!(grown.machine_ids.len(), 2, "and it is a set again");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ending_a_set_leaves_every_machine_settling() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		let set = declare_machines(&mut conn, &[a, b]).await.expect("declare");
		let lifted = MaintenanceWindow::lift(&mut conn, set.id, Some("op"))
			.await
			.expect("lift");
		assert!(lifted.ended_at.is_some());
		let on_group = MaintenanceWindow::list_for_scope(&mut conn, Scope::Group(fleet.group), 10)
			.await
			.expect("group history");
		assert!(
			on_group.iter().any(|span| span.window.id == set.id),
			"a group's history includes the windows over machines of its environments"
		);

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("targets");
		for machine in [a, b] {
			assert!(
				targets.settling(machine, Some(fleet.group)),
				"each machine serves the settle period"
			);
			assert!(machine_suspended(&mut conn, fleet.group, machine).await);
		}
		assert!(!targets.suspends(c, Some(fleet.group)));

		sql_query(
			"UPDATE maintenance_windows SET ended_at = NOW() - INTERVAL '11 minutes' WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(set.id)
		.execute(&mut conn)
		.await
		.expect("backdate end");
		for machine in [a, b] {
			assert!(!machine_suspended(&mut conn, fleet.group, machine).await);
		}
		let (_, settled) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(settled, 1, "the set is claimed once");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machines_history_includes_the_sets_that_name_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		let set = declare_machines(&mut conn, &[a, b, c])
			.await
			.expect("declare");
		MaintenanceWindow::amend(&mut conn, set.id, machines(&[a, b]), Some("op"))
			.await
			.expect("drop c");
		MaintenanceWindow::amend(&mut conn, set.id, machines(&[a]), Some("op"))
			.await
			.expect("drop b");
		let ended = MaintenanceWindow::lift(&mut conn, set.id, Some("op"))
			.await
			.expect("lift");

		let on_a = MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(a), 10)
			.await
			.expect("a's history");
		assert_eq!(on_a.len(), 1);
		assert_eq!(on_a[0].window.id, set.id);
		assert_eq!(on_a[0].moved_at, None, "a was covered to the end");
		assert_eq!(on_a[0].covered_from, set.declared_at);
		assert_eq!(on_a[0].window.ended_at, ended.ended_at);

		for dropped in [b, c] {
			let on = MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(dropped), 10)
				.await
				.expect("history");
			assert_eq!(
				on.len(),
				1,
				"{dropped} keeps the window it went quiet under"
			);
			assert_eq!(on[0].window.id, set.id);
			assert!(on[0].moved_at.is_some(), "over the span it covered it");
			assert_eq!(
				on[0].covered_from, set.declared_at,
				"the window carried on over the rest, so it covered {dropped} from its declaration"
			);
		}
		let on_c = MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(c), 10)
			.await
			.expect("c's history");
		assert_eq!(
			on_c[0].moved_to.as_deref(),
			Some("fiji fj-a"),
			"and where it went reads as what the window covers now"
		);

		let on_clone =
			MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(fleet.clone_box), 10)
				.await
				.expect("clone history");
		assert!(on_clone.is_empty());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_set_can_be_moved_only_along_its_own_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		let set = declare_machines(&mut conn, &[a, b]).await.expect("declare");

		let moved = MaintenanceWindow::amend(
			&mut conn,
			set.id,
			Amendment {
				target: Some(Grain::environment(fleet.group, ServerRank::Production)),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("widen to the environment");
		assert_eq!(moved.machine_ids.len(), 0);
		assert_eq!(
			moved.grain(),
			Some(Grain::environment(fleet.group, ServerRank::Production))
		);

		for machine in [a, b] {
			let history = MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(machine), 10)
				.await
				.expect("history");
			assert_eq!(history.len(), 1);
			assert!(history[0].moved_at.is_some());
			assert_eq!(history[0].moved_to.as_deref(), Some("fiji"));
		}

		let from_env = MaintenanceWindow::amend(&mut conn, set.id, machines(&[b, a]), Some("op"))
			.await
			.expect("narrow back to machines");
		assert_eq!(from_env.machine_ids.len(), 2);
		let both = MaintenanceWindow::amend(
			&mut conn,
			set.id,
			Amendment {
				target: Some(Grain::machine(a)),
				machines: Some(vec![a, b]),
				..Amendment::default()
			},
			Some("op"),
		)
		.await;
		assert!(
			matches!(both, Err(AppError::BadRequest(_))),
			"a grain and machines are not both a target: {both:?}"
		);

		let descent = line_of_descent_for(&mut conn, &Cover::Machines(set.machine_ids.clone()))
			.await
			.expect("descent");
		for grain in [
			Grain::group(fleet.group),
			Grain::environment(fleet.group, ServerRank::Production),
			Grain::machine(a),
			Grain::machine(b),
			Grain::machine(c),
			Grain::application(fleet.apps[0]),
		] {
			assert!(descent.includes(grain), "{grain:?}");
		}
		assert!(
			!descent.includes(Grain::application(fleet.apps[2])),
			"an application on a machine the set does not name is not contained by it"
		);
		assert!(!descent.includes(Grain::machine(fleet.clone_box)));

		let to_the_clone = MaintenanceWindow::amend(
			&mut conn,
			set.id,
			machines(&[a, fleet.clone_box]),
			Some("op"),
		)
		.await;
		assert!(
			matches!(to_the_clone, Err(AppError::BadRequest(_))),
			"{to_the_clone:?}"
		);

		let app = MaintenanceWindow::amend(
			&mut conn,
			set.id,
			Amendment {
				target: Some(Grain::application(fleet.apps[0])),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("narrow to an application on a named machine");
		assert_eq!(app.application_id, Some(fleet.apps[0]));
		assert!(app.machine_ids.is_empty());

		let machines_from_app =
			MaintenanceWindow::amend(&mut conn, set.id, machines(&[a, b]), Some("op")).await;
		assert!(
			matches!(machines_from_app, Err(AppError::BadRequest(_))),
			"several machines are chosen from an environment or its machines: {machines_from_app:?}"
		);
	})
	.await
}
