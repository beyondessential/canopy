//! The lease a run holds: what it records of the run, how it starts, and how it
//! is released, taken over, swept and kept with the windows its run declares.
//!
//! spec: ANSRUN#run-leases

use std::collections::BTreeMap;

use commons_tests::diesel_async::{RunQueryDsl, SimpleAsyncConnection};
use commons_types::server::rank::ServerRank;
use database::{
	ansible_leases::{AnsibleLease, PENDING_FOR, ReleaseReason, RunIntent, RunSpec},
	maintenance_windows::MaintenanceWindow,
};
use diesel::{QueryableByName, sql_query, sql_types};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

#[derive(QueryableByName)]
struct EndedBy {
	#[diesel(sql_type = sql_types::Text)]
	by: String,
}

async fn ended_notices(conn: &mut commons_tests::diesel_async::AsyncPgConnection) -> Vec<String> {
	sql_query(
		"SELECT payload->>'by' AS by FROM slack_outbox \
		 WHERE kind = 'maintenance_ended' ORDER BY created_at",
	)
	.get_results::<EndedBy>(conn)
	.await
	.unwrap()
	.into_iter()
	.map(|notice| notice.by)
	.collect()
}

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

		let run = RunSpec {
			intent: RunIntent::Upgrade,
			note: Some("rolling out"),
			playbook: Some("tamanu-upgrade.yml"),
			limit_pattern: Some("facility"),
			limited_to: Some(&limited),
			tags: &tags,
			skip_tags: &skip,
			assignments: &assignments,
		};
		let lease = take(&mut conn, group, &machines, &run, "sam@example.org", false).await;

		let read = AnsibleLease::get(&mut conn, lease.id).await.unwrap();
		assert!(read.pending(), "pending until a run reads it");
		let RunSpec {
			intent,
			note,
			playbook,
			limit_pattern,
			limited_to,
			tags: run_tags,
			skip_tags: run_skip,
			assignments: run_assignments,
		} = run;
		let AnsibleLease {
			id,
			server_group_id,
			rank,
			intent: read_intent,
			held_by,
			note: read_note,
			taken_at,
			expires_at,
			released_at,
			released_by,
			playbook: read_playbook,
			limit_pattern: read_limit_pattern,
			limited_to: read_limited_to,
			tags: read_tags,
			skip_tags: read_skip,
			assignments: read_assignments,
			started_at,
			release_reason,
		} = read;
		assert_eq!(id, lease.id);
		assert_eq!(server_group_id, group);
		assert_eq!(rank, ServerRank::Production);
		assert_eq!(read_intent, RunIntent::Upgrade, "the intent reads back");
		assert_eq!(read_intent, intent);
		assert_eq!(held_by.as_deref(), Some("sam@example.org"));
		assert_eq!(read_note.as_deref(), Some("rolling out"));
		assert_eq!(read_note.as_deref(), note);
		assert!(expires_at > taken_at, "it holds from when it is taken");
		assert_eq!(read_playbook.as_deref(), Some("tamanu-upgrade.yml"));
		assert_eq!(read_playbook.as_deref(), playbook);
		assert_eq!(read_limit_pattern.as_deref(), Some("facility"));
		assert_eq!(read_limit_pattern.as_deref(), limit_pattern);
		assert_eq!(read_limited_to.as_deref(), limited_to);
		assert_eq!(read_limited_to, Some(limited));
		assert_eq!(read_tags, tags);
		assert_eq!(read_tags, run_tags);
		assert_eq!(read_skip, skip);
		assert_eq!(read_skip, run_skip);
		assert_eq!(read_assignments, assignments);
		assert_eq!(&read_assignments, run_assignments);
		assert!(started_at.is_none(), "no run has read it");
		assert!(released_at.is_none() && released_by.is_none());
		assert!(release_reason.is_none());

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

/// A lease no run read within the pending period is released by Canopy, and a
/// started one is left alone. An operator can give up on an unread lease, but
/// not on a started one.
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
		assert!(
			!AnsibleLease::release_unstarted(&mut conn, started.id, Some("a"))
				.await
				.unwrap(),
			"release_unstarted leaves a lease its run has read"
		);
		let read = AnsibleLease::get(&mut conn, started.id).await.unwrap();
		assert!(
			read.released_at.is_none() && read.release_reason.is_none(),
			"release_unstarted: the started lease is still held"
		);
		assert!(
			!AnsibleLease::release_unstarted(&mut conn, unread.id, Some("a"))
				.await
				.unwrap(),
			"release_unstarted: a lease already released is not released again"
		);
		let read = AnsibleLease::get(&mut conn, unread.id).await.unwrap();
		assert_eq!(
			read.release_reason,
			Some(ReleaseReason::Unstarted),
			"release_unstarted: the sweep's reason is kept"
		);
		assert!(read.released_by.is_none());
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

		// An operator giving up on a lease no run has read releases it as an
		// operator does, not as Canopy's sweep does.
		let pending = take(&mut conn, group, &machines, &spec(&none), "b", false).await;
		assert!(
			AnsibleLease::release_unstarted(&mut conn, pending.id, Some("b"))
				.await
				.unwrap(),
			"release_unstarted releases a lease no run has read"
		);
		let read = AnsibleLease::get(&mut conn, pending.id).await.unwrap();
		assert_eq!(
			read.release_reason,
			Some(ReleaseReason::Released),
			"release_unstarted: recorded as the holder releasing, not the sweep's unstarted"
		);
		assert_eq!(read.released_by.as_deref(), Some("b"));
		assert!(read.released_at.is_some());
		assert!(
			AnsibleLease::open_for(&mut conn, group, ServerRank::Production)
				.await
				.unwrap()
				.is_none(),
			"release_unstarted: nothing holds the environment"
		);
		assert!(
			!AnsibleLease::release_unstarted(&mut conn, pending.id, Some("b"))
				.await
				.unwrap(),
			"release_unstarted: released once"
		);
	})
	.await
}

/// Only the first read starts a lease. The windows its run declares end with
/// the lease, follow its expiry as it is extended, and go on to the new holder,
/// bound to the new lease, when it is taken over. A lease records why it was
/// released, and that it was taken over.
// spec: ANSRUN#work-under-way, ANSRUN#starting-a-run, ANSRUN#audit
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
		assert!(started.started_at.is_some() && !started.pending());
		assert!(
			AnsibleLease::start(&mut conn, lease.id)
				.await
				.unwrap()
				.is_none(),
			"only the first read starts it"
		);
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
		assert_eq!(
			bound[0].amended_at, None,
			"following the lease is not an amendment"
		);

		// Taking the lease over passes the window to the new holder, bound to the
		// lease that took it over.
		let over = take(&mut conn, group, &machines, &spec(&none), "b", true).await;
		assert_eq!(
			AnsibleLease::get(&mut conn, lease.id)
				.await
				.unwrap()
				.release_reason,
			Some(ReleaseReason::TakenOver),
			"the lease taken over says so"
		);
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
		assert!(passed[0].amended_at.is_some());
		assert_eq!(passed[0].declared_by.as_deref(), Some("a"));
		assert!(passed[0].ended_at.is_none(), "suspension does not lapse");
		let now = Timestamp::now();
		assert!(
			MaintenanceWindow::refusing_run(&passed, "b", now).is_none(),
			"the new holder is served the environment the window covers"
		);
		assert!(
			MaintenanceWindow::refusing_run(&passed, "stranger", now).is_some(),
			"anyone else is refused"
		);

		// Releasing the lease ends the window.
		assert!(
			AnsibleLease::release(&mut conn, over.id, Some("b"), ReleaseReason::Released)
				.await
				.unwrap(),
			"the lease was still held"
		);
		assert_eq!(
			AnsibleLease::get(&mut conn, over.id)
				.await
				.unwrap()
				.release_reason,
			Some(ReleaseReason::Released)
		);
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
		assert_eq!(
			ended_notices(&mut conn).await,
			vec!["the run's lease ending".to_owned()],
			"the run's own release is not an operator lifting a window"
		);
		let targets = MaintenanceWindow::suspended_targets(&mut conn)
			.await
			.unwrap();
		assert!(
			targets.settling(machines[0], Some(group)),
			"it settles as any window does"
		);

		let again = MaintenanceWindow::end_for_lease(&mut conn, over.id, Some("b"))
			.await
			.unwrap();
		assert!(again.is_empty(), "ending is idempotent");
		assert_eq!(ended_notices(&mut conn).await.len(), 1);
	})
	.await
}
