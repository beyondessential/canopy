//! Maintenance windows: an operator's declaration that a target is being
//! worked on. While one suspends a target its checks grade exactly as they
//! would without it, its issues leave their incident and open none, and
//! suspension outlasts the window by the settle period so a machine that is
//! back but has not reported yet is not paged for.
//!
//! A window is over a machine, and the checks it suspends are those of the
//! applications running on it: these tests declare over the machine and assert
//! against an application's issues, which is the coverage that matters.

use commons_types::{server::rank::ServerRank, status::CheckResult};
use database::{
	issues::{CheckFiling, Issue, Scope, file_check},
	maintenance_windows::{MaintenanceWindow, SETTLE},
	statuses::CANOPY_SOURCE,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
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

async fn insert_group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('g') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert group");
	row.id
}

/// A machine and the one application on it, as `(machine, application)`.
async fn insert_server(
	conn: &mut diesel_async::AsyncPgConnection,
	group_id: Option<Uuid>,
) -> (Uuid, Uuid) {
	let machine: RowId = sql_query("INSERT INTO machines (group_id) VALUES ($1) RETURNING id")
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
		.get_result(conn)
		.await
		.expect("insert machine");
	let application: RowId = sql_query(
		"INSERT INTO applications (type, host, group_id, machine_id) \
		 VALUES ('tamanu-central', 'http://maint.invalid/', $1, $2) RETURNING id",
	)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert application");
	(machine.id, application.id)
}

/// A machine and the one ranked application on it, as `(machine,
/// application)`. The machine takes the application's rank, which is the rank
/// an environment's window has to match.
async fn insert_ranked_server(
	conn: &mut diesel_async::AsyncPgConnection,
	group_id: Uuid,
	rank: &str,
) -> (Uuid, Uuid) {
	let machine: RowId = sql_query("INSERT INTO machines (group_id) VALUES ($1) RETURNING id")
		.bind::<sql_types::Uuid, _>(group_id)
		.get_result(conn)
		.await
		.expect("insert machine");
	let application: RowId = sql_query(
		"INSERT INTO applications (type, host, group_id, rank, machine_id) VALUES ('tamanu-central', $1, $2, $3, $4) RETURNING id",
	)
	.bind::<sql_types::Text, _>(format!("http://maint-{rank}.invalid/"))
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Text, _>(rank)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert application");
	(machine.id, application.id)
}

fn filing(server_id: Uuid, check: &str, observed: CheckResult) -> CheckFiling<'_> {
	CheckFiling {
		source: CANOPY_SOURCE,
		scope: Scope::Application(server_id),
		device_id: None,
		check,
		observed,
		title: Some("Maintenance test"),
		message: "maintenance test filing",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

async fn state_for(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	check: &str,
) -> Issue {
	Issue::list_by_source_ref(conn, CANOPY_SOURCE, check, &[server_id])
		.await
		.expect("list")
		.into_iter()
		.next()
		.expect("state row filed")
}

/// Incidents on the group that are still open.
async fn open_incidents(conn: &mut diesel_async::AsyncPgConnection, group_id: Uuid) -> i64 {
	let row: Count = sql_query(
		"SELECT COUNT(*) AS count FROM incidents \
		 WHERE server_group_id = $1 AND closed_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.get_result(conn)
	.await
	.expect("count open incidents");
	row.count
}

/// Issues currently attached to an incident on the group.
async fn live_members(conn: &mut diesel_async::AsyncPgConnection, group_id: Uuid) -> i64 {
	let row: Count = sql_query(
		"SELECT COUNT(*) AS count FROM incident_issues ii \
		 JOIN incidents i ON i.id = ii.incident_id \
		 WHERE i.server_group_id = $1 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.get_result(conn)
	.await
	.expect("count live members");
	row.count
}

async fn backdate_expected_end(
	conn: &mut diesel_async::AsyncPgConnection,
	id: Uuid,
	ago: SignedDuration,
) {
	sql_query("UPDATE maintenance_windows SET expected_end = $2 WHERE id = $1")
		.bind::<sql_types::Uuid, _>(id)
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(Timestamp::now() - ago))
		.execute(conn)
		.await
		.expect("backdate");
}

fn in_an_hour() -> Timestamp {
	Timestamp::now() + SignedDuration::from_hours(1)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_grades_a_check_as_it_stands_and_opens_no_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			Some("upgrading"),
			Some("op"),
		)
		.await
		.expect("declare");

		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file");

		let state = state_for(&mut conn, server_id, "reachability").await;
		assert_eq!(
			state.observed_result,
			Some(CheckResult::Failed),
			"the observation is recorded through the window"
		);
		assert_eq!(
			state.effective_result,
			Some(CheckResult::Failed),
			"and graded as it stands, so an operator watches what they are fixing"
		);
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			0,
			"the window stops it opening an incident, which is all it stops"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn declaring_takes_the_target_out_of_its_open_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;

		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file");
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			1,
			"a failure with no window opens an incident"
		);

		MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		assert_eq!(
			live_members(&mut conn, group_id).await,
			0,
			"the issue leaves the incident when the window is declared"
		);
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			0,
			"nothing else holds the incident open, so it closes immediately"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_window_covers_its_servers_and_the_group_itself() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (_machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file server check");
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			0,
			"a group's window covers the checks of every server in it"
		);

		let mut group_filing = filing(server_id, "backup-staleness", CheckResult::Failed);
		group_filing.scope = Scope::Group(group_id);
		file_check(&mut conn, group_filing)
			.await
			.expect("file group check");
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			0,
			"the group's own checks are covered too, so nothing opens"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_ends_at_its_expected_end_and_suspension_outlasts_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (machine_id, _server_id) = insert_server(&mut conn, None).await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		// A minute past its expected end: the sweep ends it, stamping the
		// end at the expected end rather than at now.
		backdate_expected_end(&mut conn, window.id, SignedDuration::from_mins(1)).await;
		let (ended, settled) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!((ended, settled), (1, 0), "ended, and not yet settled");

		let ended = MaintenanceWindow::get(&mut conn, window.id)
			.await
			.expect("get");
		assert!(ended.ended_at.is_some(), "the expected end ends the window");
		assert_eq!(
			ended.ended_by, None,
			"nobody lifted it; its expected end passed"
		);
		assert!(
			MaintenanceWindow::suspends(&mut conn, Some(machine_id), None)
				.await
				.expect("suspends"),
			"suspension runs on through the settle period"
		);

		// Past the settle period now.
		backdate_expected_end(&mut conn, window.id, SETTLE + SignedDuration::from_mins(1)).await;
		sql_query("UPDATE maintenance_windows SET ended_at = expected_end WHERE id = $1")
			.bind::<sql_types::Uuid, _>(window.id)
			.execute(&mut conn)
			.await
			.expect("backdate end");
		assert!(
			!MaintenanceWindow::suspends(&mut conn, Some(machine_id), None)
				.await
				.expect("suspends"),
			"the settle period has elapsed, so the target is watched again"
		);

		let (_, settled) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(settled, 1, "the settled window is claimed exactly once");
		let (_, again) = MaintenanceWindow::sweep(&mut conn).await.expect("sweep");
		assert_eq!(again, 0, "and not claimed twice");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_after_the_settle_period_opens_an_incident_again() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file during the window");
		assert_eq!(open_incidents(&mut conn, group_id).await, 0);

		backdate_expected_end(&mut conn, window.id, SETTLE + SignedDuration::from_mins(1)).await;
		MaintenanceWindow::sweep(&mut conn).await.expect("sweep");

		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file after settling");
		assert_eq!(
			state_for(&mut conn, server_id, "reachability")
				.await
				.effective_result,
			Some(CheckResult::Failed),
			"grading is normal again once the settle period has elapsed"
		);
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			1,
			"anything still failing contributes from then on"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn declaring_over_an_open_window_amends_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (machine_id, _server_id) = insert_server(&mut conn, None).await;
		let first = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			Some("upgrading"),
			Some("op"),
		)
		.await
		.expect("declare");

		let later = Timestamp::now() + SignedDuration::from_hours(3);
		let second = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			later,
			Some("still upgrading"),
			Some("other"),
		)
		.await
		.expect("amend");

		assert_eq!(second.id, first.id, "a target has at most one open window");
		assert_eq!(second.declared_by.as_deref(), Some("op"));
		assert_eq!(
			second.amended_by.as_deref(),
			Some("other"),
			"the amendment records who made it"
		);
		assert!(second.amended_at.is_some());
		assert!(second.expected_end > first.expected_end);

		let windows = MaintenanceWindow::list_for_scope(&mut conn, Scope::Machine(machine_id), 10)
			.await
			.expect("list");
		assert_eq!(windows.len(), 1, "and not a second window");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn lifting_records_the_operator_and_is_idempotent() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (machine_id, _server_id) = insert_server(&mut conn, None).await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		let lifted = MaintenanceWindow::lift(&mut conn, window.id, Some("other"))
			.await
			.expect("lift");
		assert_eq!(lifted.ended_by.as_deref(), Some("other"));
		let ended_at = lifted.ended_at.expect("ended");

		let again = MaintenanceWindow::lift(&mut conn, window.id, Some("third"))
			.await
			.expect("lift again");
		assert_eq!(
			again.ended_at,
			Some(ended_at),
			"a window already ended is left as it was"
		);
		assert_eq!(again.ended_by.as_deref(), Some("other"));
	})
	.await
}

#[derive(QueryableByName)]
struct OutboxVars {
	#[diesel(sql_type = sql_types::Text)]
	target: String,
	#[diesel(sql_type = sql_types::Text)]
	by: String,
}

async fn outbox_vars(conn: &mut diesel_async::AsyncPgConnection, kind: &str) -> Vec<OutboxVars> {
	sql_query(
		"SELECT payload->>'target' AS target, payload->>'by' AS by \
		 FROM slack_outbox WHERE kind = $1 ORDER BY created_at",
	)
	.bind::<sql_types::Text, _>(kind)
	.get_results(conn)
	.await
	.expect("read outbox")
}

#[tokio::test(flavor = "multi_thread")]
async fn declaring_and_ending_notify_operators() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (machine_id, _server_id) = insert_server(&mut conn, None).await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			Some("swapping the disk"),
			Some("op"),
		)
		.await
		.expect("declare");

		let declared = outbox_vars(&mut conn, "maintenance_declared").await;
		assert_eq!(declared.len(), 1, "declaring notifies once");
		assert_eq!(declared[0].by, "op");

		MaintenanceWindow::lift(&mut conn, window.id, Some("other"))
			.await
			.expect("lift");
		let ended = outbox_vars(&mut conn, "maintenance_ended").await;
		assert_eq!(ended.len(), 1, "ending notifies once");
		assert_eq!(ended[0].by, "other", "the ending names who lifted it");
		assert_eq!(ended[0].target, declared[0].target);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_expiring_notifies_as_the_expected_end_passing() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (machine_id, _server_id) = insert_server(&mut conn, None).await;
		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		backdate_expected_end(&mut conn, window.id, SignedDuration::from_mins(1)).await;
		MaintenanceWindow::sweep(&mut conn).await.expect("sweep");

		let ended = outbox_vars(&mut conn, "maintenance_ended").await;
		assert_eq!(ended.len(), 1);
		assert_eq!(
			ended[0].by, "its expected end passing",
			"nobody lifted it, so the notice names the expiry, not an operator"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incident_closed_by_a_declaration_says_so() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;

		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file");
		assert_eq!(open_incidents(&mut conn, group_id).await, 1);

		// A resolve notice is suppressed while the open is still pending
		// delivery; the drainer has shipped it in any real timeline.
		sql_query("UPDATE slack_outbox SET delivered_at = now() WHERE kind = 'incident_open'")
			.execute(&mut conn)
			.await
			.expect("mark open delivered");

		MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		#[derive(QueryableByName)]
		struct ResolveBy {
			#[diesel(sql_type = sql_types::Text)]
			by: String,
		}
		let resolves: Vec<ResolveBy> = sql_query(
			"SELECT payload->>'by' AS by FROM slack_outbox WHERE kind = 'incident_resolve'",
		)
		.get_results(&mut conn)
		.await
		.expect("read resolves");
		assert_eq!(resolves.len(), 1, "the close is notified");
		assert_eq!(
			resolves[0].by, "maintenance declared by op",
			"the notice says maintenance was declared, not that the problem went away"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_server_joining_a_group_under_a_window_is_covered() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			None,
			in_an_hour(),
			None,
			None,
		)
		.await
		.expect("declare");

		// Joins while the window holds.
		let (_machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;
		file_check(
			&mut conn,
			filing(server_id, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file");

		assert_eq!(
			state_for(&mut conn, server_id, "reachability")
				.await
				.effective_result,
			Some(CheckResult::Failed),
			"the failure grades as it stands"
		);
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			0,
			"and a group's window covers servers that join while it holds"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn canopy_wide_checks_are_never_suspended() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, server_id) = insert_server(&mut conn, Some(group_id)).await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			None,
			in_an_hour(),
			None,
			None,
		)
		.await
		.expect("declare group window");
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			None,
		)
		.await
		.expect("declare server window");

		let mut global = filing(server_id, "self-heartbeat", CheckResult::Failed);
		global.scope = Scope::Global;
		file_check(&mut conn, global).await.expect("file global");

		#[derive(QueryableByName)]
		struct Effective {
			#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
			effective_result: Option<String>,
		}
		let row: Effective = sql_query(
			"SELECT effective_result FROM issues \
			 WHERE check_name = 'self-heartbeat' AND application_id IS NULL \
			   AND machine_id IS NULL AND server_group_id IS NULL",
		)
		.get_result(&mut conn)
		.await
		.expect("global state row");
		assert_eq!(
			row.effective_result.as_deref(),
			Some("failed"),
			"a window over a machine or a group never suspends canopy's own checks"
		);
	})
	.await
}

/// The point of declaring over the machine: one window, every workload on the
/// box. Naming an application would have left its neighbours alerting through
/// work that was always going to stop them too.
// spec: MNT#declaring
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_window_covers_every_application_on_the_box() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, first) = insert_server(&mut conn, Some(group_id)).await;

		// A second workload on the same box.
		let second: RowId = sql_query(
			"INSERT INTO applications (type, host, group_id, machine_id) \
			 VALUES ('tamanu-central', 'http://maint2.invalid/', $1, $2) RETURNING id",
		)
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(group_id))
		.bind::<sql_types::Uuid, _>(machine_id)
		.get_result(&mut conn)
		.await
		.expect("second application");

		MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			Some("patching the host"),
			None,
		)
		.await
		.expect("declare machine window");

		for (label, application) in [("first", first), ("second", second.id)] {
			file_check(
				&mut conn,
				filing(application, "reachability", CheckResult::Failed),
			)
			.await
			.expect("file");
			assert_eq!(
				open_incidents(&mut conn, group_id).await,
				0,
				"the {label} application on the box is covered by its machine's window"
			);
		}

		// An application on another box is untouched by it.
		let (_elsewhere, other) = insert_server(&mut conn, Some(group_id)).await;
		file_check(
			&mut conn,
			filing(other, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file");
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			1,
			"a window covers the box it names and no other"
		);
		assert_eq!(
			live_members(&mut conn, group_id).await,
			1,
			"and only the uncovered box's failure is in the incident"
		);
	})
	.await;
}

/// An upgrade rehearsed on a site's clone leaves its production, and the
/// group's own checks, watched.
// spec: MNT#declaring
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_window_covers_only_its_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (production_box, production) =
			insert_ranked_server(&mut conn, group_id, "production").await;
		let (clone_box, clone) = insert_ranked_server(&mut conn, group_id, "clone").await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Clone),
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		file_check(
			&mut conn,
			filing(clone, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file clone check");
		assert_eq!(
			live_members(&mut conn, group_id).await,
			0,
			"the clone is under the window"
		);

		file_check(
			&mut conn,
			filing(production, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file production check");
		assert_eq!(
			live_members(&mut conn, group_id).await,
			1,
			"production is not"
		);

		let mut group_filing = filing(production, "backup-staleness", CheckResult::Failed);
		group_filing.scope = Scope::Group(group_id);
		file_check(&mut conn, group_filing)
			.await
			.expect("file group check");
		assert_eq!(
			live_members(&mut conn, group_id).await,
			2,
			"the group's own checks are watched through a clone's window"
		);

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("suspended");
		assert!(
			targets.machines.contains(&clone_box) && !targets.machines.contains(&production_box)
		);
		assert!(
			!targets.groups.contains(&group_id),
			"an environment's window is not the group's"
		);

		// The group's own window is a distinct target beside the clone's.
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare group-wide");
		assert!(
			MaintenanceWindow::open_for(&mut conn, Scope::Group(group_id), Some(ServerRank::Clone))
				.await
				.expect("open")
				.is_some(),
			"declaring over the group does not amend the clone's window"
		);
	})
	.await
}

/// Ranks were spelled `live` and `prod` before the canonical set, and such an
/// application is production: a production window must still cover it.
// spec: MNT#declaring
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_rank_spelling_falls_under_its_environment_window() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (_box, legacy) = insert_ranked_server(&mut conn, group_id, "live").await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Production),
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		file_check(
			&mut conn,
			filing(legacy, "reachability", CheckResult::Failed),
		)
		.await
		.expect("file check");
		assert_eq!(
			live_members(&mut conn, group_id).await,
			0,
			"an application stored as live is under its group's production window"
		);
	})
	.await
}

/// An application with no rank serves no environment, so an environment's
/// window says nothing about it: only the group's own window covers it.
// spec: MNT#declaring
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_window_does_not_suspend_an_unranked_member() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (unranked_box, unranked) = insert_server(&mut conn, Some(group_id)).await;
		let (production_box, production) =
			insert_ranked_server(&mut conn, group_id, "production").await;
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Production),
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");

		for application in [production, unranked] {
			file_check(
				&mut conn,
				filing(application, "reachability", CheckResult::Failed),
			)
			.await
			.expect("file check");
		}
		assert_eq!(
			open_incidents(&mut conn, group_id).await,
			1,
			"the unranked application's failure opens an incident"
		);
		assert_eq!(
			live_members(&mut conn, group_id).await,
			1,
			"and is the only issue in it: production is under the window"
		);

		assert!(
			!MaintenanceWindow::suspends(&mut conn, Some(unranked_box), Some(group_id))
				.await
				.expect("suspends"),
			"an unranked application is in no environment's window"
		);
		assert!(
			MaintenanceWindow::suspends(&mut conn, Some(production_box), Some(group_id))
				.await
				.expect("suspends"),
			"the environment's own members are"
		);

		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare group-wide");
		assert!(
			MaintenanceWindow::suspends(&mut conn, Some(unranked_box), Some(group_id))
				.await
				.expect("suspends"),
			"the group's own window covers every member, ranked or not"
		);
	})
	.await
}

/// The Slack notice for an environment's window names the environment, so a
/// reader can tell that a site's clone went quiet from the site itself going
/// quiet. Only the group's and the machine's forms were covered.
// spec: MNT#notification
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_windows_notice_names_the_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		insert_ranked_server(&mut conn, group_id, "clone").await;
		insert_ranked_server(&mut conn, group_id, "production").await;

		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Clone),
			in_an_hour(),
			Some("refreshing from production"),
			Some("op"),
		)
		.await
		.expect("declare over the clone");

		let declared = outbox_vars(&mut conn, "maintenance_declared").await;
		assert_eq!(declared.len(), 1, "declaring notifies once");
		assert_eq!(
			declared[0].target, "g clone",
			"the notice names the environment, not the bare group"
		);
	})
	.await
}

/// A production environment's window reads as the group's name alone, the way
/// production trouble does everywhere else, so the two forms are worth pinning
/// together.
// spec: MNT#notification
#[tokio::test(flavor = "multi_thread")]
async fn a_production_windows_notice_names_the_group_alone() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		insert_ranked_server(&mut conn, group_id, "production").await;

		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Production),
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare over production");

		let declared = outbox_vars(&mut conn, "maintenance_declared").await;
		assert_eq!(declared[0].target, "g", "production reads as the site");
	})
	.await
}

/// Suspension outlasts a window by the settle period, and the status surfaces
/// mark those two states apart. An environment's window has to resolve to its
/// machines for either mark to land on a box, which is the branch nothing
/// exercised.
// spec: MNT#settling
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_window_settles_over_the_machines_it_covered() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (clone_box, _) = insert_ranked_server(&mut conn, group_id, "clone").await;
		let (production_box, _) = insert_ranked_server(&mut conn, group_id, "production").await;

		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Clone),
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare over the clone");

		let holding = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("suspended");
		assert!(
			holding.suspends(clone_box, Some(group_id))
				&& !holding.settling(clone_box, Some(group_id)),
			"while it holds, the clone's box reads as being worked on"
		);
		assert!(
			!holding.suspends(production_box, Some(group_id)),
			"and production's box is not covered at all"
		);

		MaintenanceWindow::lift(&mut conn, window.id, Some("op"))
			.await
			.expect("lift");

		let settling = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("suspended");
		assert!(
			settling.suspends(clone_box, Some(group_id)),
			"lifting does not end suspension, the settle period does"
		);
		assert!(
			settling.settling(clone_box, Some(group_id)),
			"and the box reads as settling rather than as still being worked on"
		);
	})
	.await
}

/// A box under two windows is being worked on for as long as either holds, so
/// its own window ending does not make it settling while its group's still
/// stands. Getting this wrong marks a box as handed back while an operator is
/// still in it.
// spec: MNT#settling
#[tokio::test(flavor = "multi_thread")]
async fn a_box_whose_group_window_still_holds_is_not_settling() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (machine_id, _) = insert_server(&mut conn, Some(group_id)).await;

		let own = MaintenanceWindow::declare(
			&mut conn,
			Scope::Machine(machine_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare over the box");
		MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			None,
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare over the group");

		MaintenanceWindow::lift(&mut conn, own.id, Some("op"))
			.await
			.expect("lift the box's own");

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("suspended");
		assert!(
			!targets.settling(machine_id, Some(group_id)),
			"the group's window still holds over it"
		);
	})
	.await
}

/// Once the settle period elapses the box is no longer suspended at all, which
/// is what takes the mark off it rather than leaving it marked for good.
// spec: MNT#settling
#[tokio::test(flavor = "multi_thread")]
async fn a_box_past_the_settle_period_is_no_longer_suspended() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let (clone_box, _) = insert_ranked_server(&mut conn, group_id, "clone").await;

		let window = MaintenanceWindow::declare(
			&mut conn,
			Scope::Group(group_id),
			Some(ServerRank::Clone),
			in_an_hour(),
			None,
			Some("op"),
		)
		.await
		.expect("declare");
		MaintenanceWindow::lift(&mut conn, window.id, Some("op"))
			.await
			.expect("lift");
		sql_query("UPDATE maintenance_windows SET ended_at = $2 WHERE id = $1")
			.bind::<sql_types::Uuid, _>(window.id)
			.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(
				Timestamp::now() - SETTLE - SignedDuration::from_mins(1),
			))
			.execute(&mut conn)
			.await
			.expect("age the end past the settle period");

		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.expect("suspended");
		assert!(
			!targets.suspends(clone_box, Some(group_id)),
			"nothing is held back once the settle period is out"
		);
	})
	.await
}
