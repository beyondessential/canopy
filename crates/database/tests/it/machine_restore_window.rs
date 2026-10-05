//! Model-level tests for the per-machine restore window.
//!
//! A restore rewrites the box, so the window is the box's: opened once for a
//! machine however many workloads it carries.
//!
//! spec: BKO#allowing-a-restore

use database::machines::{Machine, NewMachine};
use diesel_async::SimpleAsyncConnection;
use jiff::Timestamp;

#[tokio::test(flavor = "multi_thread")]
async fn restore_window_opens_for_a_day_then_closes() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		let created = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		assert!(!created.restore_allowed(), "restores start disallowed");
		assert!(created.restore_allowed_until.is_none());

		// Opening the window returns an expiry roughly a day out.
		let until = Machine::allow_restore(&mut conn, created.id, Some("op@example"))
			.await
			.unwrap();
		let secs = until.duration_since(Timestamp::now()).as_secs();
		assert!(
			(23 * 3600..=25 * 3600).contains(&secs),
			"window should be ~24h, got {secs}s"
		);

		let reloaded = Machine::get_by_id(&mut conn, created.id).await.unwrap();
		assert!(reloaded.restore_allowed(), "window is open");
		// Postgres `timestamptz` keeps microseconds, so the round-tripped value
		// drops the nanosecond tail of the returned instant — compare with a
		// tolerance rather than for exact equality.
		let stored = reloaded.restore_allowed_until.expect("expiry persisted");
		assert!(
			stored.duration_since(until).as_secs().abs() <= 1,
			"stored expiry {stored} should match the returned {until}"
		);
		assert_eq!(reloaded.restore_allowed_by.as_deref(), Some("op@example"));

		// Closing the window clears both the expiry and the recorded operator.
		Machine::disallow_restore(&mut conn, created.id)
			.await
			.unwrap();
		let reloaded = Machine::get_by_id(&mut conn, created.id).await.unwrap();
		assert!(!reloaded.restore_allowed(), "window is closed");
		assert!(reloaded.restore_allowed_until.is_none());
		assert!(reloaded.restore_allowed_by.is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn expired_window_reads_as_closed() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		let created = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		// A window that already lapsed: set, but in the past.
		conn.batch_execute(&format!(
			"UPDATE machines \
			 SET restore_allowed_until = now() - interval '1 hour', \
			     restore_allowed_by = 'op@example' \
			 WHERE id = '{}'",
			created.id
		))
		.await
		.unwrap();

		let reloaded = Machine::get_by_id(&mut conn, created.id).await.unwrap();
		assert!(
			reloaded.restore_allowed_until.is_some(),
			"the lapsed expiry is still stored"
		);
		assert!(
			!reloaded.restore_allowed(),
			"a past expiry must read as closed"
		);
	})
	.await;
}

/// The window is the box's, not the fleet's: opening one machine for restore
/// says nothing about any other.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_reaches_only_the_machine_it_was_opened_on() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let other = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		Machine::allow_restore(&mut conn, machine.id, Some("op@example"))
			.await
			.unwrap();

		let machine = Machine::get_by_id(&mut conn, machine.id).await.unwrap();
		let other = Machine::get_by_id(&mut conn, other.id).await.unwrap();
		assert!(machine.restore_allowed());
		assert!(!other.restore_allowed());
	})
	.await;
}
