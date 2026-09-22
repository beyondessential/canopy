//! Retires operator sessions that have gone quiet.
//!
//! Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//!
//! A session's own liveness is separate from the raise it may be carrying. A
//! raise lapses on its own after ten minutes, and a lapsed raise reads as
//! read-only wherever it is decided, so this sweep is not what ends one — most
//! sessions never raise at all. What it does is stop the table growing without
//! bound as clients come and go: a row nobody has presented for a day is a
//! browser tab that was closed.
//!
//! Retiring a session a client still holds costs that client nothing it can
//! notice. An unknown session identifier reads as read-only rather than being
//! refused, and asking for a session mints a fresh one, so a client that comes
//! back after a day carries on from read-only — which is where it would have
//! been anyway.

use std::time::Duration;

use database::operator_sessions::OperatorSession;
use jiff::{SignedDuration, Timestamp};
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, warn};

/// How long a session may go unseen before it is retired. Well clear of the ten
/// minutes a raise lasts: this is about abandoned clients, not about ending a
/// raise, which expires on its own.
pub const IDLE_GRACE: SignedDuration = SignedDuration::from_hours(24);

const TICK: Duration = Duration::from_secs(3600);

/// One pass. Public so a test can drive it without waiting out [`TICK`].
pub async fn tick(db: &mut database::diesel_async::AsyncPgConnection, now: Timestamp) {
	let Some(cutoff) = now.checked_sub(IDLE_GRACE).ok() else {
		warn!("session-sweep: could not work out the cutoff; not sweeping");
		return;
	};

	match OperatorSession::retire_idle(db, cutoff).await {
		Ok(0) => debug!("session-sweep: nothing to retire"),
		Ok(n) => debug!("session-sweep: retired {n} idle session(s)"),
		Err(e) => warn!("session-sweep: retiring idle sessions failed: {e}"),
	}
}

pub fn spawn() -> JoinHandle<()> {
	let pool = database::init();
	task::spawn(async move {
		loop {
			sleep(TICK).await;
			let Ok(mut db) = pool.get().await else {
				error!("Failed to get database connection");
				continue;
			};
			tick(&mut db, Timestamp::now()).await;
		}
	})
}
