//! Client sessions on the private server's administrative surface and the safety
//! mode each is in.
//!
//! Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//!
//! The server holds each session's mode as a row rather than in a signed token,
//! so a raise can be ended before its expiry, the live sessions can be
//! enumerated, and the mode survives a restart. A session begins read-only; a
//! raise sets a higher mode and an expiry ten minutes out, and the mode returns
//! to read-only once that passes.

use commons_errors::{AppError, Result};
use commons_types::safety::SafetyMode;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

/// How long a raise lasts from the moment it is made. The operator-facing
/// countdown runs to this, and the session returns to read-only once it passes.
pub const RAISE_DURATION: SignedDuration = SignedDuration::from_mins(10);

/// How many sessions one login may hold at once. An operator works in a handful
/// of clients; far past that, the oldest are ones they no longer hold.
pub const SESSIONS_PER_LOGIN: i64 = 10;

/// How much longer than [`RAISE_DURATION`] the server honours a raised grade.
/// Not offered to the operator and not shown anywhere: it exists only so a
/// request already in flight as the raise ends is not refused for an expiry the
/// operator had no way to anticipate.
pub const HONOUR_GRACE: SignedDuration = SignedDuration::from_mins(1);

#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = crate::schema::operator_sessions)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct OperatorSession {
	/// The session identifier the client carries in a request header.
	pub id: Uuid,
	/// The login this session belongs to. A session is usable only by its login.
	pub login: String,
	/// The mode recorded for the session. This is the *stored* mode, which may
	/// have lapsed; use [`Self::effective_mode`] to get the mode a request
	/// should be decided against.
	pub mode: SafetyMode,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub raise_expires_at: Option<Timestamp>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub last_seen_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
}

impl OperatorSession {
	/// Mint a fresh read-only session for a login. The server mints one when a
	/// client connects, so a session always exists and a later raise modifies
	/// the one already there.
	pub async fn create(db: &mut AsyncPgConnection, login: &str) -> Result<Self> {
		use crate::schema::operator_sessions::dsl;

		let session = diesel::insert_into(dsl::operator_sessions)
			.values((dsl::id.eq(Uuid::new_v4()), dsl::login.eq(login)))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;
		Self::retire_beyond_cap(db, login).await?;
		Ok(session)
	}

	/// Drop a login's oldest sessions past [`SESSIONS_PER_LOGIN`].
	///
	/// A client asks for a session whenever it cannot present one of its own, so
	/// without a cap a caller reaching the surface could mint rows at request
	/// rate. Retiring the oldest also keeps the list of who is raised and where
	/// worth reading, rather than padded with sessions nobody holds.
	async fn retire_beyond_cap(db: &mut AsyncPgConnection, login: &str) -> Result<()> {
		use crate::schema::operator_sessions::dsl;

		let beyond: Vec<Uuid> = dsl::operator_sessions
			.select(dsl::id)
			.filter(dsl::login.eq(login))
			.order(dsl::created_at.desc())
			.offset(SESSIONS_PER_LOGIN)
			.load(db)
			.await
			.map_err(AppError::from)?;
		if beyond.is_empty() {
			return Ok(());
		}
		diesel::delete(dsl::operator_sessions)
			.filter(dsl::id.eq_any(beyond))
			.execute(db)
			.await
			.map_err(AppError::from)
			.map(|_| ())
	}

	/// Fetch a session by its identifier. `None` for an unknown identifier —
	/// the caller treats that as read-only rather than refusing the request.
	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Option<Self>> {
		use crate::schema::operator_sessions::dsl;

		dsl::operator_sessions
			.select(Self::as_select())
			.filter(dsl::id.eq(id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every session a login currently holds, newest first. Reads who is raised
	/// and where.
	pub async fn for_login(db: &mut AsyncPgConnection, login: &str) -> Result<Vec<Self>> {
		use crate::schema::operator_sessions::dsl;

		dsl::operator_sessions
			.select(Self::as_select())
			.filter(dsl::login.eq(login))
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Raise the session to `mode`, expiring [`RAISE_DURATION`] from now. Scoped
	/// to `login` so an identifier is never usable by another login. Returns the
	/// updated row, or `None` if no session with that id belongs to that login.
	pub async fn raise(
		db: &mut AsyncPgConnection,
		id: Uuid,
		login: &str,
		mode: SafetyMode,
	) -> Result<Option<Self>> {
		use crate::schema::operator_sessions::dsl;

		let expires_at = Timestamp::now()
			.checked_add(RAISE_DURATION)
			.map_err(|e| AppError::custom(format!("bad raise duration: {e}")))?;

		diesel::update(dsl::operator_sessions)
			.filter(dsl::id.eq(id))
			.filter(dsl::login.eq(login))
			.set((
				dsl::mode.eq(mode),
				dsl::raise_expires_at.eq(jiff_diesel::Timestamp::from(expires_at)),
				dsl::last_seen_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Lower the session back to read-only at once, without waiting for the
	/// remaining time to run out. Scoped to `login`. Returns the updated row, or
	/// `None` if no session with that id belongs to that login.
	pub async fn lower(db: &mut AsyncPgConnection, id: Uuid, login: &str) -> Result<Option<Self>> {
		use crate::schema::operator_sessions::dsl;

		diesel::update(dsl::operator_sessions)
			.filter(dsl::id.eq(id))
			.filter(dsl::login.eq(login))
			.set((
				dsl::mode.eq(SafetyMode::ReadOnly),
				dsl::raise_expires_at.eq(None::<jiff_diesel::Timestamp>),
				dsl::last_seen_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Bump the session's idle liveness, so the idle sweep only retires sessions
	/// genuinely gone quiet.
	///
	/// Scoped to `login` like every other operation on a session: an identifier
	/// is not its holder's to keep alive unless the session is theirs.
	/// Returns how many rows it wrote: nothing, for an identifier that is
	/// unknown or another login's.
	pub async fn touch(db: &mut AsyncPgConnection, id: Uuid, login: &str) -> Result<usize> {
		use crate::schema::operator_sessions::dsl;

		diesel::update(dsl::operator_sessions)
			.filter(dsl::id.eq(id))
			.filter(dsl::login.eq(login))
			.set(dsl::last_seen_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())))
			.execute(db)
			.await
			.map_err(AppError::from)
	}

	/// Retire sessions not seen since `cutoff`, returning how many were removed.
	/// Driven by the periodic idle sweep in the jobs crate.
	pub async fn retire_idle(db: &mut AsyncPgConnection, cutoff: Timestamp) -> Result<usize> {
		use crate::schema::operator_sessions::dsl;

		diesel::delete(dsl::operator_sessions)
			.filter(dsl::last_seen_at.lt(jiff_diesel::Timestamp::from(cutoff)))
			.execute(db)
			.await
			.map_err(AppError::from)
	}

	/// The mode a request should be decided against, given the current time.
	///
	/// A raise is honoured until [`HONOUR_GRACE`] past its expiry; once that
	/// passes the session reads as read-only, whatever mode is stored. Doubt
	/// resolves downwards: a stored raise with no expiry is treated as lapsed.
	pub fn effective_mode(&self, now: Timestamp) -> SafetyMode {
		if self.mode == SafetyMode::ReadOnly {
			return SafetyMode::ReadOnly;
		}
		match self.raise_expires_at {
			// An overflow adding the grace can only push the deadline further
			// out, so falling back to the un-graced expiry is the safe reading.
			Some(expires_at) => {
				let deadline = expires_at.checked_add(HONOUR_GRACE).unwrap_or(expires_at);
				if now <= deadline {
					self.mode
				} else {
					SafetyMode::ReadOnly
				}
			}
			None => SafetyMode::ReadOnly,
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn session(mode: SafetyMode, raise_expires_at: Option<Timestamp>) -> OperatorSession {
		OperatorSession {
			id: Uuid::new_v4(),
			login: "op@bes.au".into(),
			mode,
			raise_expires_at,
			last_seen_at: Timestamp::now(),
			created_at: Timestamp::now(),
		}
	}

	#[test]
	fn a_read_only_session_is_read_only() {
		let now = Timestamp::now();
		assert_eq!(
			session(SafetyMode::ReadOnly, None).effective_mode(now),
			SafetyMode::ReadOnly
		);
	}

	#[test]
	fn a_live_raise_holds_at_its_mode() {
		let now = Timestamp::now();
		let expires = now.checked_add(SignedDuration::from_mins(5)).unwrap();
		assert_eq!(
			session(SafetyMode::Danger, Some(expires)).effective_mode(now),
			SafetyMode::Danger
		);
	}

	#[test]
	fn a_lapsed_raise_reads_as_read_only_but_within_grace_still_holds() {
		let now = Timestamp::now();

		// Just inside the honour grace past expiry: still the raised mode.
		let just_lapsed = now.checked_sub(SignedDuration::from_secs(30)).unwrap();
		assert_eq!(
			session(SafetyMode::Write, Some(just_lapsed)).effective_mode(now),
			SafetyMode::Write
		);

		// Past the grace: read-only, whatever mode is stored.
		let long_lapsed = now.checked_sub(SignedDuration::from_mins(2)).unwrap();
		assert_eq!(
			session(SafetyMode::Write, Some(long_lapsed)).effective_mode(now),
			SafetyMode::ReadOnly
		);
	}

	#[test]
	fn a_stored_raise_without_an_expiry_is_treated_as_lapsed() {
		let now = Timestamp::now();
		assert_eq!(
			session(SafetyMode::Danger, None).effective_mode(now),
			SafetyMode::ReadOnly
		);
	}
}
