//! The lease a configuration run holds over an environment while it runs.
//!
//! An environment holds at most one, and the environment is served to its holder
//! alone, so two runs never act on one environment at once.
// spec: ANSRUN#run-leases

use std::{collections::BTreeMap, fmt::Display, str::FromStr};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::{
	deserialize::{self, FromSql, Queryable},
	expression::AsExpression,
	pg::{Pg, PgValue},
	prelude::*,
	sql_types::{Array, Jsonb, Nullable, Text, Uuid as SqlUuid},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::maintenance_windows::MaintenanceWindow;

/// How long a lease holds before it has to be extended. A run that dies stops
/// holding the environment once this passes.
pub const LEASE_DURATION: SignedDuration = SignedDuration::from_mins(30);

/// How long a lease waits for its run to read the environment before Canopy
/// releases it, so a closed tab or an abandoned dialog does not hold the
/// environment shut.
// spec: ANSRUN#starting-a-run
pub const PENDING_FOR: SignedDuration = SignedDuration::from_mins(5);

/// What a run intends to do to the environment it holds.
#[derive(
	Debug,
	Clone,
	Copy,
	Default,
	PartialEq,
	Eq,
	Serialize,
	Deserialize,
	AsExpression,
	utoipa::ToSchema,
)]
#[diesel(sql_type = Text)]
#[serde(rename_all = "lowercase")]
pub enum RunIntent {
	#[default]
	Configure,
	Upgrade,
}

impl Display for RunIntent {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Configure => write!(f, "configure"),
			Self::Upgrade => write!(f, "upgrade"),
		}
	}
}

#[derive(Debug, Clone, Copy)]
pub struct RunIntentFromStringError;

impl FromStr for RunIntent {
	type Err = RunIntentFromStringError;

	fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
		match value {
			"configure" => Ok(Self::Configure),
			"upgrade" => Ok(Self::Upgrade),
			_ => Err(RunIntentFromStringError),
		}
	}
}

impl TryFrom<String> for RunIntent {
	type Error = RunIntentFromStringError;

	fn try_from(value: String) -> std::result::Result<Self, Self::Error> {
		value.parse()
	}
}

impl From<RunIntent> for String {
	fn from(intent: RunIntent) -> Self {
		intent.to_string()
	}
}

impl std::error::Error for RunIntentFromStringError {}
impl Display for RunIntentFromStringError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		write!(f, "invalid run intent")
	}
}

/// Why a lease stopped holding.
#[derive(
	Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, AsExpression, utoipa::ToSchema,
)]
#[diesel(sql_type = Text)]
#[serde(rename_all = "snake_case")]
pub enum ReleaseReason {
	/// Its holder gave the environment back.
	Released,
	/// Another operator took the work over.
	TakenOver,
	/// No run read it in time, so Canopy released it.
	Unstarted,
	/// It ran out without being extended.
	Expired,
}

impl Display for ReleaseReason {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(match self {
			Self::Released => "released",
			Self::TakenOver => "taken_over",
			Self::Unstarted => "unstarted",
			Self::Expired => "expired",
		})
	}
}

impl FromStr for ReleaseReason {
	type Err = RunIntentFromStringError;

	fn from_str(value: &str) -> std::result::Result<Self, Self::Err> {
		match value {
			"released" => Ok(Self::Released),
			"taken_over" => Ok(Self::TakenOver),
			"unstarted" => Ok(Self::Unstarted),
			"expired" => Ok(Self::Expired),
			_ => Err(RunIntentFromStringError),
		}
	}
}

/// A nullable uuid array as read back: diesel cannot tell an array holds no
/// nulls, and these never do.
#[derive(Clone, Debug, Default)]
pub struct NullableUuids(Option<Vec<Uuid>>);

impl Queryable<Nullable<Array<Nullable<SqlUuid>>>, Pg> for NullableUuids {
	type Row = Option<Vec<Option<Uuid>>>;

	fn build(row: Self::Row) -> deserialize::Result<Self> {
		Ok(Self(row.map(|ids| ids.into_iter().flatten().collect())))
	}
}

impl From<NullableUuids> for Option<Vec<Uuid>> {
	fn from(ids: NullableUuids) -> Self {
		ids.0
	}
}

/// A text array as read back, for the same reason.
#[derive(Clone, Debug, Default)]
pub struct TextArray(Vec<String>);

impl Queryable<Array<Nullable<Text>>, Pg> for TextArray {
	type Row = Vec<Option<String>>;

	fn build(row: Self::Row) -> deserialize::Result<Self> {
		Ok(Self(row.into_iter().flatten().collect()))
	}
}

impl From<TextArray> for Vec<String> {
	fn from(list: TextArray) -> Self {
		list.0
	}
}

/// The application type each machine with nothing on it is assigned for a run.
#[derive(Clone, Debug, Default, diesel::FromSqlRow)]
pub struct Assignments(BTreeMap<Uuid, String>);

impl FromSql<Jsonb, Pg> for Assignments {
	fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
		let value = <serde_json::Value as FromSql<Jsonb, Pg>>::from_sql(bytes)?;
		Ok(Self(serde_json::from_value(value)?))
	}
}

impl From<Assignments> for BTreeMap<Uuid, String> {
	fn from(assignments: Assignments) -> Self {
		assignments.0
	}
}

/// What a run is, as its lease records it. Further arguments the operator
/// types are never among it.
#[derive(Clone, Debug)]
pub struct RunSpec<'a> {
	pub intent: RunIntent,
	pub note: Option<&'a str>,
	/// The playbook, where the lease was taken to start one.
	pub playbook: Option<&'a str>,
	/// The limit as typed, and the machines it matched.
	pub limit_pattern: Option<&'a str>,
	pub limited_to: Option<&'a [Uuid]>,
	pub tags: &'a [String],
	pub skip_tags: &'a [String],
	/// The application type each machine with nothing on it is assigned.
	pub assignments: &'a BTreeMap<Uuid, String>,
}

/// What a release is conditional on, where a sweep decided it from a read that
/// may since have gone stale.
#[derive(Clone, Copy)]
enum Guard {
	None,
	/// The lease has not been started.
	Unread,
}

/// What a sweep of the leases released.
#[derive(Debug, Default)]
pub struct Swept {
	/// Leases no run read in time.
	pub unstarted: Vec<AnsibleLease>,
	/// Leases that ran out.
	pub expired: Vec<AnsibleLease>,
}

/// A run's hold on one environment.
#[derive(Clone, Debug, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::ansible_leases)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct AnsibleLease {
	/// Unique identifier of this lease, which a run names to read the
	/// environment.
	pub id: Uuid,
	/// The group the environment belongs to.
	pub server_group_id: Uuid,
	/// The rank of the environment within it.
	#[diesel(deserialize_as = String)]
	pub rank: ServerRank,
	/// What the run holding it intends.
	#[diesel(deserialize_as = String)]
	pub intent: RunIntent,
	/// The login the lease is held by.
	pub held_by: Option<String>,
	/// What the holder said they are doing.
	pub note: Option<String>,
	/// When it was taken.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub taken_at: Timestamp,
	/// When it stops holding unless extended.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub expires_at: Timestamp,
	/// When it was released, and `None` while it is still held.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub released_at: Option<Timestamp>,
	/// Who released it.
	pub released_by: Option<String>,
	/// The playbook the lease was taken to run, where it was taken for one.
	pub playbook: Option<String>,
	/// The limit as the operator typed it.
	pub limit_pattern: Option<String>,
	/// The machines that limit matched, null where the run acts on the whole
	/// environment.
	#[diesel(deserialize_as = NullableUuids)]
	pub limited_to: Option<Vec<Uuid>>,
	/// The tags the run runs.
	#[diesel(deserialize_as = TextArray)]
	pub tags: Vec<String>,
	/// The tags the run skips.
	#[diesel(deserialize_as = TextArray)]
	pub skip_tags: Vec<String>,
	/// The application type assigned to each machine with nothing on it, by
	/// machine identifier.
	#[diesel(deserialize_as = Assignments)]
	pub assignments: BTreeMap<Uuid, String>,
	/// When the run first read the environment under this lease. A lease is
	/// pending until then.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub started_at: Option<Timestamp>,
	/// Why the lease stopped holding, where it has.
	#[diesel(deserialize_as = ReleaseReasonColumn)]
	pub release_reason: Option<ReleaseReason>,
}

/// A release reason as stored.
#[derive(Clone, Copy, Debug)]
pub struct ReleaseReasonColumn(Option<ReleaseReason>);

impl Queryable<Nullable<Text>, Pg> for ReleaseReasonColumn {
	type Row = Option<String>;

	fn build(row: Self::Row) -> deserialize::Result<Self> {
		Ok(Self(row.and_then(|reason| reason.parse().ok())))
	}
}

impl From<ReleaseReasonColumn> for Option<ReleaseReason> {
	fn from(reason: ReleaseReasonColumn) -> Self {
		reason.0
	}
}

impl AnsibleLease {
	/// Whether this lease still holds the environment shut at `now`.
	pub fn holds_at(&self, now: Timestamp) -> bool {
		self.released_at.is_none() && self.expires_at > now
	}

	/// Whether it holds for somebody other than `login`, which is what a take
	/// is refused for and what taking over goes around.
	pub fn holds_for_another(&self, login: Option<&str>, now: Timestamp) -> bool {
		self.holds_at(now)
			&& self
				.held_by
				.as_deref()
				.is_some_and(|who| Some(who) != login)
	}

	/// The environment's unreleased lease, expired or not. A caller deciding
	/// whether the environment is held pairs this with [`Self::holds_at`].
	pub async fn open_for(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
	) -> Result<Option<Self>> {
		use crate::schema::ansible_leases::dsl;

		dsl::ansible_leases
			.select(Self::as_select())
			.filter(dsl::server_group_id.eq(group_id))
			.filter(dsl::rank.eq(rank.to_string()))
			.filter(dsl::released_at.is_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every unreleased lease on the group's environments, expired or not.
	pub async fn open_for_group(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::ansible_leases::dsl;

		dsl::ansible_leases
			.select(Self::as_select())
			.filter(dsl::server_group_id.eq(group_id))
			.filter(dsl::released_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Whether no run has read the environment under this lease yet.
	pub fn pending(&self) -> bool {
		self.started_at.is_none() && self.released_at.is_none()
	}

	/// When a pending lease is released if no run has read it by then.
	pub fn pending_until(&self) -> Timestamp {
		self.taken_at + PENDING_FOR
	}

	/// Take the environment's lease, releasing an expired one in the way.
	/// Refuses one another operator still holds unless `take_over`, and gives
	/// back instead of taking where another operator's maintenance window holds
	/// over the environment: over its group, the environment itself, or any of
	/// `machine_ids` (see [`MaintenanceWindow::refusing_run`]).
	///
	/// Taking over passes the windows the lease's run declared to the new holder
	/// and keeps them bound, now to the new lease, so suspension does not lapse
	/// partway through the work.
	///
	/// Takes the group's row so concurrent takes on one environment queue
	/// rather than racing the open-lease index into a database error. A window
	/// moving over any part of the group takes the same row, so the window this
	/// is checked against cannot move off the environment before the lease is
	/// in. The no-key form is what makes that cheap: `FOR UPDATE` would also
	/// queue behind every job writing a row that references the group.
	// spec: ANSRUN#work-under-way
	#[allow(clippy::too_many_arguments)]
	pub async fn take(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
		machine_ids: &[Uuid],
		run: &RunSpec<'_>,
		held_by: Option<&str>,
		take_over: bool,
	) -> Result<std::result::Result<Self, MaintenanceWindow>> {
		use crate::schema::ansible_leases::dsl;
		use diesel_async::AsyncConnection;

		db.transaction::<_, AppError, _>(async |conn| {
			lock_group(conn, group_id).await?;

			let open = Self::open_for(conn, group_id, rank).await?;
			let now = Timestamp::now();
			let taking_over = open
				.as_ref()
				.filter(|open| take_over && open.holds_for_another(held_by, now))
				.map(|open| open.id);

			// The windows the lease being taken over declared go to the new
			// holder, so they are not what refuses the take.
			let windows: Vec<MaintenanceWindow> =
				MaintenanceWindow::open_over(conn, group_id, rank, machine_ids)
					.await?
					.into_iter()
					.filter(|window| taking_over.is_none_or(|id| window.lease_id != Some(id)))
					.collect();
			if let Some(window) =
				MaintenanceWindow::refusing_run(&windows, held_by.unwrap_or_default(), now)
			{
				return Ok(Err(window.clone()));
			}

			let mut taken_over: Option<Uuid> = None;
			if let Some(open) = open {
				let another = open.holds_for_another(held_by, now);
				if !take_over && another {
					return Err(AppError::Conflict(open.held_by_another()));
				}
				let reason = if another {
					ReleaseReason::TakenOver
				} else if open.holds_at(now) {
					ReleaseReason::Released
				} else {
					ReleaseReason::Expired
				};
				// A takeover keeps the windows the run declared, passing them on
				// below; anything else ends them with the lease.
				Self::release_inner(conn, open.id, held_by, reason, !another, Guard::None).await?;
				if another {
					taken_over = Some(open.id);
				}
			}

			let expires: jiff_diesel::Timestamp = (Timestamp::now() + LEASE_DURATION).into();
			let taken: Self = diesel::insert_into(dsl::ansible_leases)
				.values((
					dsl::server_group_id.eq(group_id),
					dsl::rank.eq(rank.to_string()),
					dsl::intent.eq(run.intent.to_string()),
					dsl::held_by.eq(held_by),
					dsl::note.eq(run.note),
					dsl::playbook.eq(run.playbook),
					dsl::limit_pattern.eq(run.limit_pattern),
					dsl::limited_to.eq(run.limited_to.map(<[Uuid]>::to_vec)),
					dsl::tags.eq(run.tags),
					dsl::skip_tags.eq(run.skip_tags),
					dsl::assignments
						.eq(serde_json::to_value(run.assignments).map_err(AppError::custom)?),
					dsl::expires_at.eq(expires),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await
				.map_err(AppError::from)?;

			if let Some(previous) = taken_over {
				if let Some(holder) = held_by {
					MaintenanceWindow::pass_lease(conn, previous, holder).await?;
				}
				Self::rebind_windows(conn, previous, taken.id).await?;
			}
			Ok(Ok(taken))
		})
		.await
	}

	/// Who holds this lease and until when, for a refusal that has to say who
	/// to wait for.
	pub fn held_by_another(&self) -> String {
		format!(
			"that environment's run lease is held by {} until {}{}",
			self.held_by.as_deref().unwrap_or("an operator"),
			self.expires_at.strftime("%Y-%m-%d %H:%M UTC"),
			self.note
				.as_deref()
				.map(|note| format!("; {note}"))
				.unwrap_or_default(),
		)
	}

	/// Push an unreleased lease's expiry out, so a run still going keeps it, and
	/// the windows its run declared with it.
	pub async fn extend(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::ansible_leases::dsl;

		let expires: jiff_diesel::Timestamp = (Timestamp::now() + LEASE_DURATION).into();
		let extended: Self = diesel::update(
			dsl::ansible_leases
				.find(id)
				.filter(dsl::released_at.is_null()),
		)
		.set(dsl::expires_at.eq(expires))
		.returning(Self::as_select())
		.get_result(db)
		.await
		.optional()
		.map_err(AppError::from)?
		.ok_or_else(|| AppError::NotFound("that lease is no longer held".into()))?;
		MaintenanceWindow::track_lease(db, id, extended.expires_at).await?;
		Ok(extended)
	}

	/// Give the environment back, ending the windows the lease's run declared.
	/// Answers whether the lease was still held.
	pub async fn release(
		db: &mut AsyncPgConnection,
		id: Uuid,
		by: Option<&str>,
		reason: ReleaseReason,
	) -> Result<bool> {
		Self::release_inner(db, id, by, reason, true, Guard::None).await
	}

	/// Give the environment back while no run has read the lease, as an
	/// operator giving up on a run that never started does. Answers whether it
	/// was released: a lease its run has started is left held, since releasing
	/// it would end the maintenance the run declared.
	// spec: ANSRUN#starting-a-run
	pub async fn release_unstarted(
		db: &mut AsyncPgConnection,
		id: Uuid,
		by: Option<&str>,
	) -> Result<bool> {
		Self::release_inner(db, id, by, ReleaseReason::Released, true, Guard::Unread).await
	}

	async fn release_inner(
		db: &mut AsyncPgConnection,
		id: Uuid,
		by: Option<&str>,
		reason: ReleaseReason,
		end_windows: bool,
		guard: Guard,
	) -> Result<bool> {
		use crate::schema::ansible_leases::dsl;

		// What a sweep read is checked again as it is written, so a lease its
		// run started or extended in between is left for the next tick.
		let released_at = Into::<jiff_diesel::Timestamp>::into(Timestamp::now());
		let reason = reason.to_string();
		let open = dsl::ansible_leases
			.find(id)
			.filter(dsl::released_at.is_null());
		let released = match guard {
			Guard::None => {
				diesel::update(open)
					.set((
						dsl::released_at.eq(released_at),
						dsl::released_by.eq(by),
						dsl::release_reason.eq(&reason),
					))
					.execute(db)
					.await
			}
			Guard::Unread => {
				diesel::update(open.filter(dsl::started_at.is_null()))
					.set((
						dsl::released_at.eq(released_at),
						dsl::released_by.eq(by),
						dsl::release_reason.eq(&reason),
					))
					.execute(db)
					.await
			}
		}
		.map_err(AppError::from)?;
		if released > 0 && end_windows {
			MaintenanceWindow::end_for_lease(db, id, by).await?;
		}
		Ok(released > 0)
	}

	/// Keep the open windows bound to a lease that has been taken over bound to
	/// the lease that took it over.
	async fn rebind_windows(db: &mut AsyncPgConnection, from: Uuid, to: Uuid) -> Result<()> {
		use crate::schema::maintenance_windows::dsl;

		diesel::update(
			dsl::maintenance_windows
				.filter(dsl::lease_id.eq(from))
				.filter(dsl::ended_at.is_null()),
		)
		.set(dsl::lease_id.eq(to))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	/// Stamp the lease as read by its run, once. Answers the lease where this
	/// is the read that started it, and nothing where it had started already or
	/// no longer holds.
	// spec: ANSRUN#starting-a-run
	pub async fn start(db: &mut AsyncPgConnection, id: Uuid) -> Result<Option<Self>> {
		use crate::schema::ansible_leases::dsl;

		diesel::update(
			dsl::ansible_leases
				.find(id)
				.filter(dsl::released_at.is_null())
				.filter(dsl::started_at.is_null()),
		)
		.set(dsl::started_at.eq(Into::<jiff_diesel::Timestamp>::into(Timestamp::now())))
		.returning(Self::as_select())
		.get_result(db)
		.await
		.optional()
		.map_err(AppError::from)
	}

	/// Release what has gone unread or run out, for Canopy to do itself: a
	/// lease no run read within [`PENDING_FOR`] of being taken, and one past its
	/// expiry. An expired lease is released as of its expiry.
	// spec: ANSRUN#starting-a-run
	pub async fn sweep(db: &mut AsyncPgConnection, now: Timestamp) -> Result<Swept> {
		use crate::schema::ansible_leases::dsl;

		let mut swept = Swept::default();

		let unread: Vec<Self> = dsl::ansible_leases
			.select(Self::as_select())
			.filter(dsl::released_at.is_null())
			.filter(dsl::started_at.is_null())
			.filter(dsl::taken_at.le(Into::<jiff_diesel::Timestamp>::into(now - PENDING_FOR)))
			.load(db)
			.await
			.map_err(AppError::from)?;
		for lease in unread {
			if Self::release_inner(
				db,
				lease.id,
				None,
				ReleaseReason::Unstarted,
				true,
				Guard::Unread,
			)
			.await?
			{
				swept.unstarted.push(lease);
			}
		}

		let ran_out: Vec<Self> = dsl::ansible_leases
			.select(Self::as_select())
			.filter(dsl::released_at.is_null())
			.filter(dsl::expires_at.le(Into::<jiff_diesel::Timestamp>::into(now)))
			.load(db)
			.await
			.map_err(AppError::from)?;
		for lease in ran_out {
			let released = diesel::update(
				dsl::ansible_leases
					.find(lease.id)
					.filter(dsl::released_at.is_null())
					.filter(dsl::expires_at.le(Into::<jiff_diesel::Timestamp>::into(now))),
			)
			.set((
				dsl::released_at.eq(Into::<jiff_diesel::Timestamp>::into(lease.expires_at)),
				dsl::release_reason.eq(ReleaseReason::Expired.to_string()),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
			if released > 0 {
				MaintenanceWindow::end_for_lease(db, lease.id, None).await?;
				swept.expired.push(lease);
			}
		}
		Ok(swept)
	}

	/// Find a lease by identifier.
	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::ansible_leases::dsl;

		dsl::ansible_leases
			.select(Self::as_select())
			.find(id)
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.ok_or_else(|| AppError::NotFound("no such lease".into()))
	}
}

/// Hold the group's row for the rest of the transaction, so taking a lease on
/// any of its environments and moving a window over any part of it are decided
/// one after the other. The no-key form only queues behind other holders of
/// this lock, not behind every write referencing the group.
pub async fn lock_group(conn: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
	crate::schema::server_groups::table
		.select(crate::schema::server_groups::id)
		.find(group_id)
		.for_no_key_update()
		.first::<Uuid>(conn)
		.await
		.map_err(AppError::from)?;
	Ok(())
}
