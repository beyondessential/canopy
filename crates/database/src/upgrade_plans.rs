//! Where each environment is going: the version a group's applications at one
//! rank intend to move to, and optionally when.
//!
//! A plan is a statement of intent. Nothing here performs or schedules an
//! upgrade; the date is presentational and Canopy decides a plan is met by
//! watching what the environment reports running.

use commons_errors::{AppError, Result};
use commons_types::{server::rank::ServerRank, version::VersionStr};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{Timestamp, civil::Date, civil::Time, tz::TimeZone};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{server_groups::ServerGroup, versions::Version};

/// An environment's recorded intention to move to a version.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::upgrade_plans)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct UpgradePlan {
	/// Unique identifier for this plan.
	pub id: Uuid,
	/// The group whose environment intends to move.
	pub group_id: Uuid,
	/// The rank of the environment that intends to move: the group's
	/// applications at that rank.
	pub rank: ServerRank,
	/// The version it intends to move to.
	pub target_version_id: Uuid,
	/// The day the upgrade is expected, where one is known.
	#[diesel(deserialize_as = jiff_diesel::NullableDate, serialize_as = jiff_diesel::NullableDate)]
	#[schema(value_type = Option<String>)]
	pub planned_for: Option<Date>,
	/// The hour it starts on that day, where one is known.
	#[diesel(deserialize_as = jiff_diesel::NullableTime, serialize_as = jiff_diesel::NullableTime)]
	#[schema(value_type = Option<String>)]
	pub planned_time: Option<Time>,
	/// The hour it ends on, where the window is known. Earlier than the start
	/// means the following morning.
	#[diesel(deserialize_as = jiff_diesel::NullableTime, serialize_as = jiff_diesel::NullableTime)]
	#[schema(value_type = Option<String>)]
	pub planned_end_time: Option<Time>,
	/// The IANA zone the planned time is a wall clock in.
	pub planned_zone: Option<String>,
	/// Whatever the operator needs the next reader to know.
	pub note: Option<String>,
	/// The operator who recorded it.
	pub created_by: Option<String>,
	/// When it was recorded.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub created_at: Timestamp,
	/// When the environment's reported version reached the target.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	#[schema(value_type = Option<String>)]
	pub met_at: Option<Timestamp>,
	/// When a newer plan replaced this one.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	#[schema(value_type = Option<String>)]
	pub superseded_at: Option<Timestamp>,
	/// The operator who last amended the date or note, where one has.
	pub amended_by: Option<String>,
	/// When it was last amended.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	#[schema(value_type = Option<String>)]
	pub amended_at: Option<Timestamp>,
	/// When the plan was withdrawn, if it was.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	#[schema(value_type = Option<String>)]
	pub withdrawn_at: Option<Timestamp>,
	/// The operator who withdrew it.
	pub withdrawn_by: Option<String>,
}

/// When an upgrade is expected: the day, the hour on it where that is settled,
/// and the hour it ends. The zone travels with the time because Canopy holds
/// none for a group.
#[derive(Debug, Clone, Default)]
pub struct PlannedWhen {
	pub date: Option<Date>,
	pub time: Option<Time>,
	pub end: Option<Time>,
	pub zone: Option<String>,
}

impl PlannedWhen {
	fn checked(self) -> Result<Self> {
		if self.time.is_some() != self.zone.is_some() {
			return Err(AppError::BadRequest(
				"a planned time needs the timezone it is a wall clock in".into(),
			));
		}
		if self.time.is_some() && self.date.is_none() {
			return Err(AppError::BadRequest(
				"a planned time needs the day it is on".into(),
			));
		}
		if self.end.is_some() && self.time.is_none() {
			return Err(AppError::BadRequest(
				"an end time needs the hour the upgrade starts".into(),
			));
		}
		if self.end.is_some() && self.end == self.time {
			return Err(AppError::BadRequest(
				"an upgrade cannot end at the hour it starts".into(),
			));
		}
		if let Some(zone) = &self.zone
			&& TimeZone::get(zone).is_err()
		{
			return Err(AppError::BadRequest(format!(
				"{zone} is not a known timezone"
			)));
		}
		Ok(self)
	}
}

impl UpgradePlan {
	/// Record where an environment is going, retiring any plan it already had.
	///
	/// The environment must exist, and the target must be published and ahead
	/// of what the environment runs: a plan to move somewhere it has already
	/// been is not a plan.
	// spec: UPG#a-plan
	pub async fn record(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
		target_version_id: Uuid,
		when: PlannedWhen,
		note: Option<&str>,
		created_by: &str,
	) -> Result<Self> {
		use crate::schema::upgrade_plans::dsl;

		let when = when.checked()?;
		let target = Version::get_by_id(db, target_version_id).await?;
		if target.status != commons_types::version::VersionStatus::Published {
			return Err(AppError::BadRequest(
				"an unpublished version cannot be planned for".into(),
			));
		}
		let group = ServerGroup::get_by_id(db, group_id).await?;
		let Some(environment) = ServerGroup::environment(db, group_id, rank).await? else {
			return Err(AppError::BadRequest(format!(
				"{} has no {rank} environment",
				group.name
			)));
		};
		if let Some(running) = environment.version
			&& target.as_semver() <= running.0
		{
			return Err(AppError::BadRequest(format!(
				"this environment already runs {running}, which is not behind {}",
				target.as_semver()
			)));
		}

		diesel::update(dsl::upgrade_plans)
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::rank.eq(rank))
			.filter(dsl::met_at.is_null())
			.filter(dsl::superseded_at.is_null())
			.filter(dsl::withdrawn_at.is_null())
			.set(dsl::superseded_at.eq(diesel::dsl::now))
			.execute(db)
			.await?;

		diesel::insert_into(dsl::upgrade_plans)
			.values((
				dsl::group_id.eq(group_id),
				dsl::rank.eq(rank),
				dsl::target_version_id.eq(target_version_id),
				dsl::planned_for.eq(when.date.map(jiff_diesel::Date::from)),
				dsl::planned_time.eq(when.time.map(jiff_diesel::Time::from)),
				dsl::planned_end_time.eq(when.end.map(jiff_diesel::Time::from)),
				dsl::planned_zone.eq(when.zone),
				dsl::note.eq(note),
				dsl::created_by.eq(created_by),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(|e| match e {
				diesel::result::Error::DatabaseError(
					diesel::result::DatabaseErrorKind::UniqueViolation,
					_,
				) => AppError::Conflict(
					"another plan was recorded for this environment at the same time".into(),
				),
				e => AppError::from(e),
			})
	}

	/// The environment's open plan, if it has one.
	// spec: UPG#a-plan
	pub async fn open_for_environment(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
	) -> Result<Option<Self>> {
		use crate::schema::upgrade_plans::dsl;

		dsl::upgrade_plans
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::rank.eq(rank))
			.filter(dsl::met_at.is_null())
			.filter(dsl::superseded_at.is_null())
			.filter(dsl::withdrawn_at.is_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every plan a group's environments have had, newest first.
	// spec: UPG#when-a-plan-is-met
	pub async fn history_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::upgrade_plans::dsl;

		dsl::upgrade_plans
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Plans that have closed across the fleet, most recently closed first.
	// spec: UPG#the-dashboard
	pub async fn closed_recent(db: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Self>> {
		use crate::schema::upgrade_plans::dsl;

		dsl::upgrade_plans
			.select(Self::as_select())
			.filter(
				dsl::met_at
					.is_not_null()
					.or(dsl::superseded_at.is_not_null())
					.or(dsl::withdrawn_at.is_not_null()),
			)
			.order(diesel::dsl::sql::<diesel::sql_types::Timestamptz>(
				"greatest(met_at, superseded_at, withdrawn_at) desc",
			))
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every open plan across the fleet, newest first.
	// spec: UPG#the-dashboard
	pub async fn all_open(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::upgrade_plans::dsl;

		dsl::upgrade_plans
			.select(Self::as_select())
			.filter(dsl::met_at.is_null())
			.filter(dsl::superseded_at.is_null())
			.filter(dsl::withdrawn_at.is_null())
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Plans that belong on a calendar: those with a day, still open or since
	/// met.
	///
	/// A replaced or withdrawn plan is not where the group is going, so it
	/// leaves the calendar; a met one stays as the record of what landed.
	// spec: UPG#the-calendar-feed
	pub async fn dated(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::upgrade_plans::dsl;

		dsl::upgrade_plans
			.select(Self::as_select())
			.filter(dsl::planned_for.is_not_null())
			.filter(dsl::superseded_at.is_null())
			.filter(dsl::withdrawn_at.is_null())
			.order(dsl::planned_for.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Amend an open plan's date and note.
	///
	/// The same plan better described, so it is not superseded and does not
	/// enter the history as a second plan. Changing where a group is going
	/// is a replacement instead, so the target is not amendable here.
	// spec: UPG#a-plan
	pub async fn amend(
		db: &mut AsyncPgConnection,
		id: Uuid,
		when: PlannedWhen,
		note: Option<&str>,
		amended_by: &str,
	) -> Result<Self> {
		use crate::schema::upgrade_plans::dsl;

		let when = when.checked()?;
		diesel::update(dsl::upgrade_plans)
			.filter(dsl::id.eq(id))
			.filter(dsl::met_at.is_null())
			.filter(dsl::superseded_at.is_null())
			.filter(dsl::withdrawn_at.is_null())
			.set((
				dsl::planned_for.eq(when.date.map(jiff_diesel::Date::from)),
				dsl::planned_time.eq(when.time.map(jiff_diesel::Time::from)),
				dsl::planned_end_time.eq(when.end.map(jiff_diesel::Time::from)),
				dsl::planned_zone.eq(when.zone),
				dsl::note.eq(note),
				dsl::amended_by.eq(amended_by),
				dsl::amended_at.eq(diesel::dsl::now),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.optional()?
			.ok_or_else(|| AppError::BadRequest("only an open plan can be amended".into()))
	}

	/// Withdraw a plan: the group is no longer going there.
	///
	/// The plan is retained. Where a group was going and the fact that it
	/// stopped going there is what the history exists to record, and a withdrawn
	/// plan reads differently from one that was met.
	// spec: UPG#a-plan
	pub async fn withdraw(
		db: &mut AsyncPgConnection,
		id: Uuid,
		withdrawn_by: &str,
	) -> Result<Option<Self>> {
		use crate::schema::upgrade_plans::dsl;

		diesel::update(dsl::upgrade_plans)
			.filter(dsl::id.eq(id))
			.filter(dsl::met_at.is_null())
			.filter(dsl::superseded_at.is_null())
			.filter(dsl::withdrawn_at.is_null())
			.set((
				dsl::withdrawn_at.eq(diesel::dsl::now),
				dsl::withdrawn_by.eq(withdrawn_by),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.optional()
			.map_err(AppError::from)
	}
}

/// How a plan stands: still where the environment is going, or the way it
/// closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PlanOutcome {
	/// Where the environment is going.
	Open,
	/// The environment's reported version reached the target.
	Met,
	/// A later plan took its place.
	Replaced,
	/// An operator said the environment is no longer going there.
	Withdrawn,
}

/// How a plan stands.
///
/// Met wins over the rest: a plan the environment reached is met however it was
/// stamped afterwards.
// spec: UPG#a-plan
pub fn outcome(plan: &UpgradePlan) -> PlanOutcome {
	if plan.met_at.is_some() {
		PlanOutcome::Met
	} else if plan.withdrawn_at.is_some() {
		PlanOutcome::Withdrawn
	} else if plan.superseded_at.is_some() {
		PlanOutcome::Replaced
	} else {
		PlanOutcome::Open
	}
}

/// When a plan closed, for one that has.
pub fn ended_at(plan: &UpgradePlan) -> Option<Timestamp> {
	[plan.met_at, plan.withdrawn_at, plan.superseded_at]
		.into_iter()
		.flatten()
		.max()
}

/// Close every open plan whose environment has reached its target, returning
/// how many were closed.
///
/// Reaching a version past the target closes the plan too: an environment that
/// jumped further has done the upgrade and then some, and holding the plan open
/// would report it as outstanding.
// spec: UPG#when-a-plan-is-met
pub async fn close_met_plans(db: &mut AsyncPgConnection) -> Result<usize> {
	use crate::schema::upgrade_plans::dsl;

	let open = UpgradePlan::all_open(db).await?;
	let group_ids: Vec<Uuid> = open.iter().map(|plan| plan.group_id).collect();
	let running: std::collections::HashMap<(Uuid, ServerRank), VersionStr> =
		ServerGroup::environments(db, &group_ids)
			.await?
			.into_iter()
			.filter_map(|env| env.version.map(|v| ((env.group_id, env.rank), v)))
			.collect();

	let mut closed = 0;
	for plan in open {
		let Some(running) = running.get(&(plan.group_id, plan.rank)) else {
			continue;
		};
		let target = Version::get_by_id(db, plan.target_version_id).await?;
		if running.0 < target.as_semver() {
			continue;
		}

		diesel::update(dsl::upgrade_plans)
			.filter(dsl::id.eq(plan.id))
			.set(dsl::met_at.eq(diesel::dsl::now))
			.execute(db)
			.await?;
		closed += 1;
	}

	Ok(closed)
}

/// The version the environment plans to move to, if it has an open plan.
///
/// This is what pre-upgrade testing targets in preference to the newest
/// published version, so an environment deliberately moving to an older minor is
/// held against that minor instead.
// spec: UPG#what-reads-a-plan
pub async fn planned_target(
	db: &mut AsyncPgConnection,
	group_id: Uuid,
	rank: ServerRank,
) -> Result<Option<Version>> {
	let Some(plan) = UpgradePlan::open_for_environment(db, group_id, rank).await? else {
		return Ok(None);
	};
	let target = Version::get_by_id(db, plan.target_version_id).await?;
	// A target yanked after the plan was recorded has no artefacts to fetch, so
	// it cannot steer testing; the plan stays open for the operator to revisit.
	if target.status != commons_types::version::VersionStatus::Published {
		return Ok(None);
	}
	Ok(Some(target))
}

/// Whether an open plan's date has passed without it being met.
///
/// Presentational only: an upgrade slipping is normal, and a date someone typed
/// is no basis for treating anything as failed.
// spec: UPG#the-dashboard
pub fn is_late(plan: &UpgradePlan, today: Date) -> bool {
	plan.met_at.is_none()
		&& plan.superseded_at.is_none()
		&& plan.withdrawn_at.is_none()
		&& plan.planned_for.is_some_and(|date| date < today)
}

/// A plan's target rendered as semver, for display beside what the environment
/// runs.
pub async fn target_version_str(
	db: &mut AsyncPgConnection,
	plan: &UpgradePlan,
) -> Result<VersionStr> {
	Version::get_by_id(db, plan.target_version_id)
		.await
		.map(|version| VersionStr(version.as_semver()))
}
