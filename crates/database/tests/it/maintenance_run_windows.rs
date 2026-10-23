//! The windows Canopy opens when an Ansible run starts.
//!
//! A run's window is bound to its lease and lasts the run: it covers what the
//! run acts on for the lease's holder, follows the lease's expiry, ends with
//! it, passes to whoever takes the lease over, and is freed from it when its
//! declarer holds it to an end of their own.
//!
//! spec: ANSRUN

use commons_errors::AppError;
use commons_types::server::rank::ServerRank;
use database::{
	ansible_leases::AnsibleLease,
	issues::Scope,
	maintenance_windows::{Amendment, Grain, HeldInPlace, MaintenanceWindow},
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
struct Notice {
	#[diesel(sql_type = sql_types::Text)]
	target: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	until: Option<String>,
	#[diesel(sql_type = sql_types::Text)]
	by: String,
}

async fn insert_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.get_result(conn)
		.await
		.expect("insert group");
	row.id
}

async fn insert_box(conn: &mut AsyncPgConnection, group: Uuid, rank: &str, name: &str) -> Uuid {
	let machine: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ($1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(name)
			.bind::<sql_types::Uuid, _>(group)
			.get_result(conn)
			.await
			.expect("insert machine");
	sql_query(
		"INSERT INTO applications (type, name, host, group_id, rank, machine_id) \
		 VALUES ('tamanu-central', $1, $2, $3, $4, $5)",
	)
	.bind::<sql_types::Text, _>(name)
	.bind::<sql_types::Text, _>(format!("http://{}.invalid/", Uuid::new_v4()))
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Text, _>(rank)
	.bind::<sql_types::Uuid, _>(machine.id)
	.execute(conn)
	.await
	.expect("insert application");
	machine.id
}

struct Fleet {
	group: Uuid,
	boxes: [Uuid; 3],
}

async fn fleet(conn: &mut AsyncPgConnection) -> Fleet {
	let group = insert_group(conn, "fiji").await;
	let a = insert_box(conn, group, "production", "fj-a").await;
	let b = insert_box(conn, group, "production", "fj-b").await;
	let c = insert_box(conn, group, "production", "fj-c").await;
	insert_box(conn, group, "clone", "fj-clone").await;
	Fleet {
		group,
		boxes: [a, b, c],
	}
}

/// A production lease held by `held_by`, taken as a run's first read would
/// find it.
async fn lease(conn: &mut AsyncPgConnection, group: Uuid, held_by: &str) -> AnsibleLease {
	let row: RowId = sql_query(
		"INSERT INTO ansible_leases (server_group_id, rank, intent, held_by, expires_at) \
		 VALUES ($1, 'production', 'configure', $2, NOW() + INTERVAL '30 minutes') RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Text, _>(held_by)
	.get_result(conn)
	.await
	.expect("insert lease");
	AnsibleLease::get(conn, row.id).await.expect("lease")
}

async fn notices(conn: &mut AsyncPgConnection, kind: &str) -> Vec<Notice> {
	sql_query(
		"SELECT payload->>'target' AS target, payload->>'until' AS until, payload->>'by' AS by \
		 FROM slack_outbox WHERE kind = $1 ORDER BY created_at",
	)
	.bind::<sql_types::Text, _>(kind)
	.get_results(conn)
	.await
	.expect("read outbox")
}

async fn machine_suspended(conn: &mut AsyncPgConnection, group: Uuid, machine: Uuid) -> bool {
	MaintenanceWindow::suspends(conn, None, Some(machine), Some(group))
		.await
		.expect("suspends")
}

fn in_hours(hours: i64) -> Timestamp {
	Timestamp::now() + SignedDuration::from_hours(hours)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_runs_start_declares_a_window_over_the_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let lease = lease(&mut conn, fleet.group, "op").await;

		let windows = MaintenanceWindow::declare_for_run(
			&mut conn,
			&lease,
			None,
			Some("ansible-playbook linux.yml"),
		)
		.await
		.expect("declare for the run");
		assert!(windows.serving.is_empty());
		let [window] = windows.declared.as_slice() else {
			panic!("one window: {windows:?}");
		};
		assert_eq!(
			window.grain(),
			Some(Grain::environment(fleet.group, ServerRank::Production))
		);
		assert_eq!(
			window.declared_by.as_deref(),
			Some("op"),
			"the holder declares"
		);
		assert_eq!(window.lease_id, Some(lease.id));
		assert_eq!(
			window.expected_end, lease.expires_at,
			"it lasts as long as the lease holds"
		);
		assert_eq!(window.note.as_deref(), Some("ansible-playbook linux.yml"));
		assert_eq!(
			MaintenanceWindow::bound_to_lease(&mut conn, lease.id)
				.await
				.expect("bound")
				.len(),
			1
		);

		for machine in fleet.boxes {
			assert!(machine_suspended(&mut conn, fleet.group, machine).await);
		}

		let declared = notices(&mut conn, "maintenance_declared").await;
		assert_eq!(declared.len(), 1);
		assert_eq!(declared[0].until.as_deref(), Some("lasting the run"));
		assert_eq!(declared[0].by, "op");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_limited_run_declares_over_only_what_its_limit_matches() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;
		let lease = lease(&mut conn, fleet.group, "op").await;

		let windows = MaintenanceWindow::declare_for_run(&mut conn, &lease, Some(&[b, a]), None)
			.await
			.expect("declare");
		let [window] = windows.declared.as_slice() else {
			panic!("one window: {windows:?}");
		};
		assert_eq!(
			window.machine_ids.len(),
			2,
			"one window over the two machines"
		);
		assert_eq!(window.lease_id, Some(lease.id));
		assert!(machine_suspended(&mut conn, fleet.group, a).await);
		assert!(machine_suspended(&mut conn, fleet.group, b).await);
		assert!(
			!machine_suspended(&mut conn, fleet.group, c).await,
			"the rest of the environment is watched"
		);
		let declared = notices(&mut conn, "maintenance_declared").await;
		assert_eq!(declared[0].target, "fiji 2 machines");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_limit_of_one_machine_is_that_machines_window_and_all_of_them_the_environments() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let lease_one = lease(&mut conn, fleet.group, "op").await;
		let one = MaintenanceWindow::declare_for_run(
			&mut conn,
			&lease_one,
			Some(&[fleet.boxes[1]]),
			None,
		)
		.await
		.expect("declare");
		assert_eq!(one.declared[0].machine_id, Some(fleet.boxes[1]));
		MaintenanceWindow::end_for_lease(&mut conn, lease_one.id, Some("op"))
			.await
			.expect("end");
		reset_leases_and_windows(&mut conn).await;

		let lease_all = lease(&mut conn, fleet.group, "op").await;
		let all =
			MaintenanceWindow::declare_for_run(&mut conn, &lease_all, Some(&fleet.boxes), None)
				.await
				.expect("declare");
		assert_eq!(
			all.declared[0].grain(),
			Some(Grain::environment(fleet.group, ServerRank::Production)),
			"a limit matching every member is the environment"
		);

		let stray = MaintenanceWindow::declare_for_run(
			&mut conn,
			&lease_all,
			Some(&[Uuid::new_v4()]),
			None,
		)
		.await;
		assert!(matches!(stray, Err(AppError::BadRequest(_))), "{stray:?}");
	})
	.await
}

/// Release every lease so the next one may be inserted, and clear windows.
async fn reset_leases_and_windows(conn: &mut AsyncPgConnection) {
	sql_query("UPDATE ansible_leases SET released_at = NOW() WHERE released_at IS NULL")
		.execute(conn)
		.await
		.expect("release leases");
	sql_query("DELETE FROM maintenance_windows")
		.execute(conn)
		.await
		.expect("clear windows");
}

#[tokio::test(flavor = "multi_thread")]
async fn the_holders_own_windows_serve_for_what_they_cover() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, c] = fleet.boxes;

		let own = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(a),
			None,
			in_hours(1),
			None,
			Some("op"),
		)
		.await
		.expect("the holder's own window over a");
		let lease = lease(&mut conn, fleet.group, "op").await;
		let windows = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None)
			.await
			.expect("declare");
		assert_eq!(
			windows
				.serving
				.iter()
				.map(|window| window.id)
				.collect::<Vec<_>>(),
			vec![own.id]
		);
		let [declared] = windows.declared.as_slice() else {
			panic!("one window for the rest: {windows:?}");
		};
		let mut rest = vec![b, c];
		rest.sort();
		assert_eq!(
			declared.machine_ids, rest,
			"the run's window covers the rest"
		);

		reset_leases_and_windows(&mut conn).await;
		let environment = MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(fleet.group),
			Some(ServerRank::Production),
			in_hours(1),
			None,
			Some("op"),
		)
		.await
		.expect("an environment window");
		let lease = self::lease(&mut conn, fleet.group, "op").await;
		let windows = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None)
			.await
			.expect("declare");
		assert!(
			windows.declared.is_empty(),
			"nothing new is declared where the holder's own windows cover it all"
		);
		assert_eq!(windows.serving[0].id, environment.id);

		let limited = MaintenanceWindow::declare_for_run(&mut conn, &lease, Some(&[a, b]), None)
			.await
			.expect("declare");
		assert!(
			limited.declared.is_empty(),
			"and likewise for a limit it covers"
		);
	})
	.await
}

/// A machine another operator's window names cannot be covered by a run, until
/// that window is past its end: a window past its expected end is over whether
/// or not the sweep has stamped it, so it neither refuses a run's declaration
/// nor names its machines.
// spec: MNT#ending
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_another_operators_window_names_cannot_be_covered_until_it_is_past_its_end() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, _] = fleet.boxes;
		let theirs = MaintenanceWindow::declare_machines(
			&mut conn,
			&[a, b],
			in_hours(1),
			None,
			Some("someone-else"),
		)
		.await
		.expect("another operator's set");
		let lease = lease(&mut conn, fleet.group, "op").await;

		let refused = MaintenanceWindow::declare_for_run(&mut conn, &lease, Some(&[b]), None).await;
		assert!(matches!(refused, Err(AppError::Conflict(_))), "{refused:?}");
		let refused = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None).await;
		assert!(matches!(refused, Err(AppError::Conflict(_))), "{refused:?}");
		assert!(
			MaintenanceWindow::bound_to_lease(&mut conn, lease.id)
				.await
				.expect("bound")
				.is_empty(),
			"nothing was declared"
		);

		sql_query(
			"UPDATE maintenance_windows SET expected_end = NOW() - interval '1 minute' WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(theirs.id)
		.execute(&mut conn)
		.await
		.expect("let it run past its end");

		let windows = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None)
			.await
			.expect("the run declares over the environment");
		assert_eq!(windows.declared.len(), 1);

		let ended = MaintenanceWindow::get(&mut conn, theirs.id)
			.await
			.expect("theirs");
		assert_eq!(
			ended.ended_at,
			Some(ended.expected_end),
			"ended as of its expected end, as the sweep would"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_lease_ends_the_window_at_its_expiry() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let lease = lease(&mut conn, fleet.group, "op").await;
		let window = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None)
			.await
			.expect("declare")
			.declared
			.remove(0);

		sql_query(
			"UPDATE maintenance_windows SET expected_end = NOW() - INTERVAL '1 minute' WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(window.id)
		.execute(&mut conn)
		.await
		.expect("the lease ran out");
		let (ended, _) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(ended, 1, "the sweep that ends any expired window ends it");

		let ended_notices = notices(&mut conn, "maintenance_ended").await;
		assert_eq!(ended_notices[0].by, "the run's lease ending");
		let ended = MaintenanceWindow::get(&mut conn, window.id)
			.await
			.expect("get");
		assert_eq!(ended.ended_by, None);
	})
	.await
}

/// Giving a run's window an end by amending it is holding it: the lease no
/// longer overwrites the end when it is extended.
// spec: MNT#ending
#[tokio::test(flavor = "multi_thread")]
async fn amending_a_run_windows_end_frees_it_from_the_lease() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let lease = lease(&mut conn, fleet.group, "op").await;
		let window = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None)
			.await
			.expect("declared")
			.declared
			.remove(0);

		let end = in_hours(6);
		let refused = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				expected_end: Some(end),
				..Amendment::default()
			},
			Some("bystander"),
		)
		.await;
		assert!(
			matches!(refused, Err(AppError::Conflict(_))),
			"only the run's operator gives its window an end: {refused:?}"
		);

		let amended = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				expected_end: Some(end),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("amended");
		assert!(amended.lease_id.is_none(), "no longer follows the lease");

		AnsibleLease::extend(&mut conn, lease.id)
			.await
			.expect("extended");
		let after = MaintenanceWindow::get(&mut conn, window.id)
			.await
			.expect("window");
		assert_eq!(
			after.expected_end.as_microsecond(),
			end.as_microsecond(),
			"the lease does not overwrite it"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn holding_a_window_frees_it_from_the_lease() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let lease = lease(&mut conn, fleet.group, "op").await;
		let window = MaintenanceWindow::declare_for_run(&mut conn, &lease, None, None)
			.await
			.expect("declare")
			.declared
			.remove(0);

		let refused =
			MaintenanceWindow::hold(&mut conn, window.id, in_hours(3), Some("bystander")).await;
		assert!(matches!(refused, Err(AppError::Conflict(_))), "{refused:?}");
		let past = MaintenanceWindow::hold(
			&mut conn,
			window.id,
			Timestamp::now() - SignedDuration::from_mins(1),
			Some("op"),
		)
		.await;
		assert!(matches!(past, Err(AppError::BadRequest(_))), "{past:?}");

		let end = in_hours(3);
		let held = MaintenanceWindow::hold(&mut conn, window.id, end, Some("op"))
			.await
			.expect("hold");
		assert_eq!(held.id, window.id);
		assert_eq!(held.lease_id, None);
		assert_eq!(held.expected_end.as_microsecond(), end.as_microsecond());
		assert_eq!(
			held.amended_by.as_deref(),
			Some("op"),
			"recorded as an amendment"
		);
		assert!(held.amended_at.is_some());

		assert!(
			MaintenanceWindow::end_for_lease(&mut conn, lease.id, Some("op"))
				.await
				.expect("end")
				.is_empty(),
			"the lease ending no longer ends it"
		);
		MaintenanceWindow::track_lease(&mut conn, lease.id, in_hours(1))
			.await
			.expect("track");
		assert_eq!(
			MaintenanceWindow::get(&mut conn, window.id)
				.await
				.expect("get")
				.expected_end
				.as_microsecond(),
			end.as_microsecond(),
			"nor does the lease's expiry move its end"
		);

		let again = MaintenanceWindow::hold(&mut conn, window.id, in_hours(4), Some("op")).await;
		assert!(
			matches!(again, Err(AppError::BadRequest(_))),
			"only a window that lasts a run is held: {again:?}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_stays_where_it_is_while_its_lease_is_held() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let fleet = fleet(&mut conn).await;
		let [a, b, _] = fleet.boxes;
		let lease = lease(&mut conn, fleet.group, "op").await;
		let window = MaintenanceWindow::declare_for_run(&mut conn, &lease, Some(&[a, b]), None)
			.await
			.expect("declare")
			.declared
			.remove(0);

		assert!(
			matches!(
				window.held_in_place(&mut conn).await.expect("held"),
				Some(HeldInPlace::RunLease { .. })
			),
			"the operator interface is told why it cannot move"
		);
		for amendment in [
			Amendment {
				machines: Some(vec![a]),
				..Amendment::default()
			},
			Amendment {
				target: Some(Grain::group(fleet.group)),
				..Amendment::default()
			},
		] {
			let refused =
				MaintenanceWindow::amend(&mut conn, window.id, amendment, Some("op")).await;
			assert!(matches!(refused, Err(AppError::Conflict(_))), "{refused:?}");
		}

		let extended = MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				note: Some(Some("still going".into())),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("its note can still change");
		assert_eq!(extended.lease_id, Some(lease.id));

		sql_query("UPDATE ansible_leases SET released_at = NOW() WHERE id = $1")
			.bind::<sql_types::Uuid, _>(lease.id)
			.execute(&mut conn)
			.await
			.expect("release");
		MaintenanceWindow::amend(
			&mut conn,
			window.id,
			Amendment {
				machines: Some(vec![a]),
				..Amendment::default()
			},
			Some("op"),
		)
		.await
		.expect("it moves like any other once the lease is released");
	})
	.await
}
