//! The lease a configuration run holds over an environment while it runs.
//!
//! An environment holds at most one, and the inventory is served to its holder
//! alone, so two runs never act on one environment at once.
// spec: INV#run-leases

use std::{fmt::Display, str::FromStr};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::{expression::AsExpression, prelude::*, sql_types::Text};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How long a lease holds before it has to be extended. A run that dies stops
/// holding the environment once this passes.
pub const LEASE_DURATION: SignedDuration = SignedDuration::from_mins(30);

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

/// A run's hold on one environment.
#[derive(Clone, Debug, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::inventory_leases)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct InventoryLease {
	/// Unique identifier of this lease, which a run names to read the
	/// inventory.
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
}

impl InventoryLease {
	/// Whether this lease still holds the environment shut at `now`.
	pub fn holds_at(&self, now: Timestamp) -> bool {
		self.released_at.is_none() && self.expires_at > now
	}

	/// The environment's unreleased lease, expired or not. A caller deciding
	/// whether the environment is held pairs this with [`Self::holds_at`].
	pub async fn open_for(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
	) -> Result<Option<Self>> {
		use crate::schema::inventory_leases::dsl;

		dsl::inventory_leases
			.select(Self::as_select())
			.filter(dsl::server_group_id.eq(group_id))
			.filter(dsl::rank.eq(rank.to_string()))
			.filter(dsl::released_at.is_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Take the environment's lease, releasing an expired one in the way.
	/// Refuses one another operator still holds unless `take_over`.
	///
	/// Takes the group's row so concurrent takes on one environment queue
	/// rather than racing the open-lease index into a database error.
	pub async fn take(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
		intent: RunIntent,
		held_by: Option<&str>,
		note: Option<&str>,
		take_over: bool,
	) -> Result<Self> {
		use crate::schema::inventory_leases::dsl;
		use diesel_async::AsyncConnection;

		db.transaction::<_, AppError, _>(async |conn| {
			let _group: Uuid = crate::schema::server_groups::table
				.select(crate::schema::server_groups::id)
				.find(group_id)
				.for_update()
				.first(conn)
				.await
				.map_err(AppError::from)?;

			if let Some(open) = Self::open_for(conn, group_id, rank).await? {
				if !take_over
					&& open.holds_at(Timestamp::now())
					&& open
						.held_by
						.as_deref()
						.is_some_and(|who| Some(who) != held_by)
				{
					return Err(AppError::Conflict(open.held_by_another()));
				}
				Self::release(conn, open.id, held_by).await?;
			}

			let expires: jiff_diesel::Timestamp = (Timestamp::now() + LEASE_DURATION).into();
			diesel::insert_into(dsl::inventory_leases)
				.values((
					dsl::server_group_id.eq(group_id),
					dsl::rank.eq(rank.to_string()),
					dsl::intent.eq(intent.to_string()),
					dsl::held_by.eq(held_by),
					dsl::note.eq(note),
					dsl::expires_at.eq(expires),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await
				.map_err(AppError::from)
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

	/// Push an unreleased lease's expiry out, so a run still going keeps it.
	pub async fn extend(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::inventory_leases::dsl;

		let expires: jiff_diesel::Timestamp = (Timestamp::now() + LEASE_DURATION).into();
		diesel::update(
			dsl::inventory_leases
				.find(id)
				.filter(dsl::released_at.is_null()),
		)
		.set(dsl::expires_at.eq(expires))
		.returning(Self::as_select())
		.get_result(db)
		.await
		.optional()
		.map_err(AppError::from)?
		.ok_or_else(|| AppError::NotFound("that lease is no longer held".into()))
	}

	/// Give the environment back. Answers whether the lease was still held.
	pub async fn release(db: &mut AsyncPgConnection, id: Uuid, by: Option<&str>) -> Result<bool> {
		use crate::schema::inventory_leases::dsl;

		let released = diesel::update(
			dsl::inventory_leases
				.find(id)
				.filter(dsl::released_at.is_null()),
		)
		.set((
			dsl::released_at.eq(Into::<jiff_diesel::Timestamp>::into(Timestamp::now())),
			dsl::released_by.eq(by),
		))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(released > 0)
	}

	/// Find a lease by identifier.
	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::inventory_leases::dsl;

		dsl::inventory_leases
			.select(Self::as_select())
			.find(id)
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.ok_or_else(|| AppError::NotFound("no such lease".into()))
	}
}
