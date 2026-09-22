//! The sweep retires sessions nobody has presented for a day, and leaves live
//! ones alone.
//!
//! spec: SAFE

use commons_tests::db::TestDb;
use commons_types::safety::SafetyMode;
use database::operator_sessions::OperatorSession;
use jiff::{SignedDuration, Timestamp};

/// Backdate a session's last-seen, standing in for a client that went quiet.
async fn last_seen(
	conn: &mut database::diesel_async::AsyncPgConnection,
	id: uuid::Uuid,
	at: Timestamp,
) {
	use commons_tests::diesel_async::SimpleAsyncConnection;
	conn.batch_execute(&format!(
		"UPDATE operator_sessions SET last_seen_at = '{at}' WHERE id = '{id}'"
	))
	.await
	.expect("backdate");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_gone_quiet_is_retired_and_a_live_one_is_not() {
	TestDb::run(async |mut conn, _url| {
		let now = Timestamp::now();

		let quiet = OperatorSession::create(&mut conn, "quiet@example.com")
			.await
			.expect("create");
		let live = OperatorSession::create(&mut conn, "live@example.com")
			.await
			.expect("create");

		// One went quiet a day and an hour ago; the other was seen a minute ago.
		last_seen(
			&mut conn,
			quiet.id,
			now.checked_sub(jobs::session_sweep::IDLE_GRACE + SignedDuration::from_hours(1))
				.unwrap(),
		)
		.await;
		last_seen(
			&mut conn,
			live.id,
			now.checked_sub(SignedDuration::from_mins(1)).unwrap(),
		)
		.await;

		jobs::session_sweep::tick(&mut conn, now).await;

		assert!(
			OperatorSession::get(&mut conn, quiet.id)
				.await
				.expect("read")
				.is_none(),
			"a session nobody has presented for a day is retired"
		);
		assert!(
			OperatorSession::get(&mut conn, live.id)
				.await
				.expect("read")
				.is_some(),
			"a session seen a minute ago is left alone"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_sweep_is_not_what_ends_a_raise() {
	TestDb::run(async |mut conn, _url| {
		let now = Timestamp::now();
		let session = OperatorSession::create(&mut conn, "operator@example.com")
			.await
			.expect("create");
		OperatorSession::raise(
			&mut conn,
			session.id,
			"operator@example.com",
			SafetyMode::Danger,
		)
		.await
		.expect("raise")
		.expect("session exists");

		// A raise lasts ten minutes; the sweep's grace is a day. Sweeping in
		// between leaves the raise exactly where it was.
		jobs::session_sweep::tick(&mut conn, now).await;

		let held = OperatorSession::get(&mut conn, session.id)
			.await
			.expect("read")
			.expect("still there");
		assert_eq!(held.effective_mode(now), SafetyMode::Danger);
	})
	.await;
}
