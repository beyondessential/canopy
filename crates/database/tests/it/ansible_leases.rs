//! The lease a run holds: what it records of the run, how it starts, and how it
//! is released, taken over, swept and kept with the windows its run declares.
//!
//! spec: ANSRUN#run-leases

use std::collections::BTreeMap;

use commons_tests::diesel_async::SimpleAsyncConnection;
use commons_types::server::rank::ServerRank;
use database::{
	ansible_leases::{AnsibleLease, PENDING_FOR, ReleaseReason, RunIntent, RunSpec},
	maintenance_windows::MaintenanceWindow,
};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

async fn group_with_machines(
	conn: &mut commons_tests::diesel_async::AsyncPgConnection,
	machines: usize,
) -> (Uuid, Vec<Uuid>) {
	let group = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka')"
	))
	.await
	.unwrap();
	let mut ids = Vec::new();
	for index in 0..machines {
		let id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id, rank)
			 VALUES ('{id}', 'kamaka-{index}', '{group}', 'production')"
		))
		.await
		.unwrap();
		ids.push(id);
	}
	(group, ids)
}

fn spec<'a>(assignments: &'a BTreeMap<Uuid, String>) -> RunSpec<'a> {
	RunSpec {
		intent: RunIntent::Configure,
		note: None,
		playbook: None,
		limit_pattern: None,
		limited_to: None,
		tags: &[],
		skip_tags: &[],
		assignments,
	}
}

async fn take(
	conn: &mut commons_tests::diesel_async::AsyncPgConnection,
	group: Uuid,
	machines: &[Uuid],
	run: &RunSpec<'_>,
	by: &str,
	take_over: bool,
) -> AnsibleLease {
	AnsibleLease::take(
		conn,
		group,
		ServerRank::Production,
		machines,
		run,
		Some(by),
		take_over,
	)
	.await
	.unwrap()
	.unwrap_or_else(|window| panic!("refused by a window: {window:?}"))
}

/// A lease keeps what the run was taken to do.
// spec: ANSRUN#audit
#[tokio::test(flavor = "multi_thread")]
async fn records_what_the_run_is() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, machines) = group_with_machines(&mut conn, 3).await;
		let tags = vec!["app".to_owned()];
		let skip = vec!["tailscale".to_owned()];
		let assignments = BTreeMap::from([(machines[2], "tamanu-facility".to_owned())]);
		let limited = vec![machines[0], machines[1]];

		let lease = take(
			&mut conn,
			group,
			&machines,
			&RunSpec {
				intent: RunIntent::Upgrade,
				note: Some("rolling out"),
				playbook: Some("tamanu-upgrade.yml"),
				limit_pattern: Some("facility"),
				limited_to: Some(&limited),
				tags: &tags,
				skip_tags: &skip,
				assignments: &assignments,
			},
			"sam@example.org",
			false,
		)
		.await;

		let read = AnsibleLease::get(&mut conn, lease.id).await.unwrap();
		assert_eq!(read.playbook.as_deref(), Some("tamanu-upgrade.yml"));
		assert_eq!(read.limit_pattern.as_deref(), Some("facility"));
		assert_eq!(read.limited_to, Some(limited));
		assert_eq!(read.tags, tags);
		assert_eq!(read.skip_tags, skip);
		assert_eq!(read.assignments, assignments);
		assert!(read.pending(), "pending until a run reads it");
		assert!(read.release_reason.is_none());

		let whole = take(
			&mut conn,
			group,
			&machines,
			&spec(&BTreeMap::new()),
			"sam@example.org",
			false,
		)
		.await;
		assert!(whole.limited_to.is_none(), "the whole environment");
		assert!(whole.tags.is_empty() && whole.assignments.is_empty());
	})
	.await
}

/// The first read starts the lease, once.
// spec: ANSRUN#starting-a-run
#[tokio::test(flavor = "multi_thread")]
async fn the_first_read_starts_it_once() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, machines) = group_with_machines(&mut conn, 1).await;
		let lease = take(
			&mut conn,
			group,
			&machines,
			&spec(&BTreeMap::new()),
			"a",
			false,
		)
		.await;

		let started = AnsibleLease::start(&mut conn, lease.id).await.unwrap();
		assert!(
			started
				.as_ref()
				.is_some_and(|l| l.started_at.is_some() && !l.pending())
		);
		assert!(
			AnsibleLease::start(&mut conn, lease.id)
				.await
				.unwrap()
				.is_none(),
			"only the first read starts it"
		);
	})
	.await
}

/// A lease no run read within the pending period is released by Canopy, and a
/// started one is left alone.
// spec: ANSRUN#starting-a-run
#[tokio::test(flavor = "multi_thread")]
async fn sweeps_an_unread_lease_and_an_expired_one() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, machines) = group_with_machines(&mut conn, 1).await;
		let none = BTreeMap::new();

		let unread = take(&mut conn, group, &machines, &spec(&none), "a", false).await;
		let within = AnsibleLease::sweep(
			&mut conn,
			Timestamp::now() + PENDING_FOR - SignedDuration::from_secs(5),
		)
		.await
		.unwrap();
		assert!(
			within.unstarted.is_empty(),
			"still within the pending period"
		);

		let swept = AnsibleLease::sweep(
			&mut conn,
			Timestamp::now() + PENDING_FOR + SignedDuration::from_secs(5),
		)
		.await
		.unwrap();
		assert_eq!(swept.unstarted.len(), 1);
		assert_eq!(swept.unstarted[0].id, unread.id);
		let read = AnsibleLease::get(&mut conn, unread.id).await.unwrap();
		assert_eq!(read.release_reason, Some(ReleaseReason::Unstarted));
		assert!(read.released_by.is_none(), "Canopy released it");

		// A started lease is not unread, and runs out only at its expiry.
		let started = take(&mut conn, group, &machines, &spec(&none), "a", false).await;
		AnsibleLease::start(&mut conn, started.id).await.unwrap();
		let later = Timestamp::now() + PENDING_FOR + SignedDuration::from_mins(2);
		let nothing = AnsibleLease::sweep(&mut conn, later).await.unwrap();
		assert!(nothing.unstarted.is_empty() && nothing.expired.is_empty());

		let ran_out =
			AnsibleLease::sweep(&mut conn, started.expires_at + SignedDuration::from_secs(1))
				.await
				.unwrap();
		assert_eq!(ran_out.expired.len(), 1);
		let read = AnsibleLease::get(&mut conn, started.id).await.unwrap();
		assert_eq!(read.release_reason, Some(ReleaseReason::Expired));
		assert_eq!(
			read.released_at,
			Some(started.expires_at),
			"as of its expiry"
		);
	})
	.await
}

/// Releasing says why, and taking over says it was taken over.
// spec: ANSRUN#audit
#[tokio::test(flavor = "multi_thread")]
async fn says_why_a_lease_was_released() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, machines) = group_with_machines(&mut conn, 1).await;
		let none = BTreeMap::new();

		let first = take(&mut conn, group, &machines, &spec(&none), "a", false).await;
		assert!(
			AnsibleLease::release(&mut conn, first.id, Some("a"), ReleaseReason::Released)
				.await
				.unwrap()
		);
		assert_eq!(
			AnsibleLease::get(&mut conn, first.id)
				.await
				.unwrap()
				.release_reason,
			Some(ReleaseReason::Released)
		);

		let second = take(&mut conn, group, &machines, &spec(&none), "a", false).await;
		let over = take(&mut conn, group, &machines, &spec(&none), "b", true).await;
		assert_ne!(over.id, second.id);
		assert_eq!(
			AnsibleLease::get(&mut conn, second.id)
				.await
				.unwrap()
				.release_reason,
			Some(ReleaseReason::TakenOver)
		);
	})
	.await
}

/// The windows a lease's run declares end with the lease, follow its expiry as
/// it is extended, and go on to the new holder, bound to the new lease, when it
/// is taken over.
// spec: ANSRUN#work-under-way
#[tokio::test(flavor = "multi_thread")]
async fn keeps_the_windows_its_run_declared() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, machines) = group_with_machines(&mut conn, 2).await;
		let none = BTreeMap::new();

		let lease = take(&mut conn, group, &machines, &spec(&none), "a", false).await;
		let started = AnsibleLease::start(&mut conn, lease.id)
			.await
			.unwrap()
			.unwrap();
		MaintenanceWindow::declare_for_run(&mut conn, &started, None, Some("tamanu-install.yml"))
			.await
			.unwrap();
		let bound = MaintenanceWindow::bound_to_lease(&mut conn, lease.id)
			.await
			.unwrap();
		assert_eq!(bound.len(), 1);
		assert_eq!(bound[0].expected_end, started.expires_at);

		// Extending the lease carries its window with it.
		let extended = AnsibleLease::extend(&mut conn, lease.id).await.unwrap();
		let bound = MaintenanceWindow::bound_to_lease(&mut conn, lease.id)
			.await
			.unwrap();
		assert_eq!(bound[0].expected_end, extended.expires_at);

		// Taking the lease over passes the window to the new holder, bound to the
		// lease that took it over.
		let over = take(&mut conn, group, &machines, &spec(&none), "b", true).await;
		assert!(
			MaintenanceWindow::bound_to_lease(&mut conn, lease.id)
				.await
				.unwrap()
				.is_empty()
		);
		let passed = MaintenanceWindow::bound_to_lease(&mut conn, over.id)
			.await
			.unwrap();
		assert_eq!(passed.len(), 1);
		assert_eq!(passed[0].amended_by.as_deref(), Some("b"));
		assert_eq!(passed[0].declared_by.as_deref(), Some("a"));
		assert!(passed[0].ended_at.is_none(), "suspension does not lapse");

		// Releasing the lease ends the window.
		AnsibleLease::release(&mut conn, over.id, Some("b"), ReleaseReason::Released)
			.await
			.unwrap();
		assert!(
			MaintenanceWindow::bound_to_lease(&mut conn, over.id)
				.await
				.unwrap()
				.is_empty()
		);
		let ended = MaintenanceWindow::get(&mut conn, passed[0].id)
			.await
			.unwrap();
		assert!(ended.ended_at.is_some());
	})
	.await
}
