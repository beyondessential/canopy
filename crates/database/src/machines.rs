use commons_errors::{AppError, Result};
use commons_types::{geo::GeoPoint, server::TagMap, status::ShortStatus};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::pg_duration::PgDuration;

/// How long a restore window stays open once an operator allows restores for a
/// machine. Restores read the group's backup repo, so the window is deliberately
/// short-lived; opening it again re-arms it from the moment of the new request.
const RESTORE_WINDOW: SignedDuration = SignedDuration::from_hours(24);

/// A host in the fleet: a box, physical or virtual, that canopy monitors.
///
/// Distinct from the application server running on it. A machine carries the
/// facts that belong to the box — where it is, what identity speaks for it,
/// how long it may be silent — so a host running two workloads reports its
/// platform, memory and filesystems once rather than once per workload.
///
/// A machine hosts any number of applications, including none: one created but
/// not yet reporting presents as awaiting check-in rather than as an error.
// spec: FLT
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::machines)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Machine {
	/// Unique identifier for this machine.
	pub id: Uuid,
	/// The name its operator gave it. Distinct from the hostname the
	/// operating system reports, which is a reported figure rather than a
	/// field an operator sets.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub name: Option<String>,
	/// The group this machine belongs to. The one thing an operator supplies
	/// when creating a machine: which group a box belongs to is the one
	/// fact the box has no way of knowing. The applications on it take it.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub group_id: Option<Uuid>,
	/// The identity that authenticates this machine, if one is enrolled. A
	/// machine has at most one, and an identity belongs to at most one
	/// machine, so resolving either from the other is unambiguous.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub device_id: Option<Uuid>,
	/// Whether this machine is hosted in the cloud, if known.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub cloud: Option<bool>,
	/// Where this machine is, if known.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub geolocation: Option<GeoPoint>,
	/// How long this machine may go without reporting before it is considered
	/// unreachable. Only enforced while `is_monitored`; the value is kept
	/// while unmonitored so turning monitoring back on does not lose it.
	#[schema(value_type = i64)]
	pub alert_when_down_for: PgDuration,
	/// Whether this machine is actively monitored. Switching it off quiets
	/// the machine's own checks and does not touch the applications on it —
	/// a box excused from monitoring says nothing about its workloads.
	pub is_monitored: bool,
	/// Free-form operator notes about this machine.
	#[serde(default)]
	pub notes: String,
	/// Key/value tags for this machine. A check filed against a machine is
	/// graded by policy against these rather than against any application's.
	/// An application's type is not among them, not being a property of a box.
	#[serde(default)]
	pub tags: TagMap,
	/// When set, the machine is archived: out of the live fleet, with its
	/// record and history retained. Archiving a machine archives the
	/// applications on it.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub deleted_at: Option<Timestamp>,
	/// When an identity completed enrolment for this machine. While `None`,
	/// the machine is awaiting its first check-in.
	///
	/// Also the anchor a backup deadline counts from, which is why it belongs
	/// to the machine: anchoring on an application's registration would
	/// restart a box's backup clock every time a workload was added to it.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub registered_at: Option<Timestamp>,
	/// Until when this machine is allowed to mint restore credentials for
	/// itself (ad-hoc `bestool canopy restore`). An operator opens this window
	/// and it auto-expires; `None` (or a past instant) means restores are not
	/// currently allowed. Restores read the group's backup repo, so they are
	/// gated behind this deliberate, time-boxed opt-in rather than always
	/// available.
	///
	/// A restore rewrites the box, so the window belongs to the box: a machine
	/// carrying two workloads is opened once and restored once.
	// spec: BKO#allowing-a-restore
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub restore_allowed_until: Option<Timestamp>,
	/// Who opened the current restore window (Tailscale login), if any.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(treat_none_as_default_value = false)]
	pub restore_allowed_by: Option<String>,
	#[serde(skip)]
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[serde(skip)]
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

/// The fields an operator supplies when creating a machine. Everything else
/// either has a default or arrives by enrolment and reporting.
#[derive(Debug, Clone, Default, Deserialize, Insertable)]
#[diesel(table_name = crate::schema::machines)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewMachine {
	pub name: Option<String>,
	pub group_id: Option<Uuid>,
	pub cloud: Option<bool>,
	pub geolocation: Option<GeoPoint>,
}

/// A partial update to a machine. An absent field is left alone; a present
/// `Option` field sets or clears it.
///
/// `device_id` and `registered_at` are deliberately absent: an identity is
/// bound by enrolment, not by an operator editing a form.
#[derive(Debug, Clone, Default, Deserialize, AsChangeset)]
#[diesel(table_name = crate::schema::machines)]
#[diesel(check_for_backend(diesel::pg::Pg))]
#[diesel(treat_none_as_null = false)]
pub struct MachineUpdate {
	pub name: Option<Option<String>>,
	pub group_id: Option<Option<Uuid>>,
	pub cloud: Option<Option<bool>>,
	pub geolocation: Option<Option<GeoPoint>>,
	pub is_monitored: Option<bool>,
	#[diesel(serialize_as = PgDuration)]
	pub alert_when_down_for: Option<PgDuration>,
	pub notes: Option<String>,
	pub tags: Option<TagMap>,
}

impl Machine {
	/// Create a machine. An operator supplies the group; enrolment and
	/// reporting fill in the rest.
	pub async fn create(db: &mut AsyncPgConnection, new: NewMachine) -> Result<Self> {
		diesel::insert_into(crate::schema::machines::table)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Apply an operator's edit.
	///
	/// Moving a machine between groups moves the applications on it, so this
	/// owns the two things that used to hang off setting an application's
	/// group directly:
	///
	/// - open issues are re-evaluated for anything that gains a group, so
	///   those warranting promotion to an incident do so;
	/// - both the old and the new group recompute their cached effective
	///   version, whose canonical member may have changed.
	///
	/// A trigger propagating the group onto the applications would do neither,
	/// which is why the group write goes through here rather than through raw
	/// SQL.
	// spec: FLT#groups
	pub async fn update(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		updates: MachineUpdate,
	) -> Result<Self> {
		use crate::schema::machines::dsl;

		if let Some(tags) = &updates.tags {
			crate::tags::reject_reserved_keys(tags)?;
		}

		let before = Self::get_by_id(db, machine_id).await?;

		diesel::update(dsl::machines.filter(dsl::id.eq(machine_id)))
			.set(updates)
			.execute(db)
			.await
			.map_err(AppError::from)?;

		let after = Self::get_by_id(db, machine_id).await?;

		if before.group_id != after.group_id {
			// The applications take their machine's group; they never hold one
			// of their own choosing.
			diesel::update(crate::schema::applications::table)
				.filter(crate::schema::applications::machine_id.eq(machine_id))
				.set(crate::schema::applications::group_id.eq(after.group_id))
				.execute(db)
				.await
				.map_err(AppError::from)?;

			if before.group_id.is_none() && after.group_id.is_some() {
				for application in after.applications(db).await? {
					crate::issues::reevaluate_open_issues_for_server(db, application.id).await?;
				}
			}

			for group in [before.group_id, after.group_id].into_iter().flatten() {
				crate::server_groups::ServerGroup::recompute_version(db, group).await?;
			}
		}

		Ok(after)
	}

	/// Bind an identity to this machine without claiming it has enrolled.
	///
	/// An operator naming a tailnet node on the create form is saying which box
	/// this is, not that the box has checked in. `registered_at` stays null so
	/// the machine reads as awaiting enrolment, and so a backup deadline starts
	/// counting when the box actually arrives rather than when someone typed
	/// its address.
	// spec: FLT#identities
	pub async fn bind_device(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		device_id: Uuid,
	) -> Result<()> {
		use crate::schema::machines::dsl;
		diesel::update(dsl::machines.filter(dsl::id.eq(machine_id)))
			.set(dsl::device_id.eq(Some(device_id)))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Bind an identity to this machine and mark it enrolled. Idempotent on
	/// `registered_at`: a re-enrolment does not restart the clock a backup
	/// deadline counts from.
	// spec: FLT#identities
	pub async fn mark_registered(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		device_id: Uuid,
	) -> Result<()> {
		use crate::schema::machines::dsl;
		diesel::update(dsl::machines.filter(dsl::id.eq(machine_id)))
			.set((
				dsl::device_id.eq(Some(device_id)),
				dsl::registered_at.eq(diesel::dsl::sql::<
					diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
				>("COALESCE(machines.registered_at, NOW())")),
			))
			.execute(db)
			.await?;
		Ok(())
	}

	pub async fn get_by_id(db: &mut AsyncPgConnection, machine_id: Uuid) -> Result<Self> {
		use crate::schema::machines::dsl;
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::id.eq(machine_id))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// Like [`Machine::get_by_id`] but takes a `FOR UPDATE` row lock.
	///
	/// A report that finds no application for what it describes creates one,
	/// so two pushes arriving together for one box would otherwise each see
	/// nothing and each create. Serialising them on the machine row means the
	/// second sees what the first created.
	pub async fn get_by_id_for_update(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
	) -> Result<Self> {
		use crate::schema::machines::dsl;
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::id.eq(machine_id))
			.for_update()
			.first(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_by_ids(db: &mut AsyncPgConnection, ids: &[Uuid]) -> Result<Vec<Self>> {
		use crate::schema::machines::dsl;
		if ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The machine an identity speaks for, if it speaks for one at all.
	///
	/// An identity that authenticates something other than a machine — an
	/// operator's credential, a relay — belongs to no machine, so this is the
	/// resolution step a machine-gated route takes and an admin-gated one
	/// never reaches.
	// spec: FLT#identities
	pub async fn get_by_device_id(
		db: &mut AsyncPgConnection,
		device: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::machines::dsl;
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::device_id.eq(device))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every machine still in the live fleet.
	pub async fn list_live(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::machines::dsl;
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::deleted_at.is_null())
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The live machines in a group.
	pub async fn list_for_group(db: &mut AsyncPgConnection, group: Uuid) -> Result<Vec<Self>> {
		use crate::schema::machines::dsl;
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group))
			.filter(dsl::deleted_at.is_null())
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Bulk-fetch names for a set of machine ids, for surfaces embedding a
	/// machine's display name beside its id.
	pub async fn names_by_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Option<String>>> {
		use crate::schema::machines::dsl;

		if ids.is_empty() {
			return Ok(std::collections::HashMap::new());
		}
		let rows: Vec<(Uuid, Option<String>)> = dsl::machines
			.select((dsl::id, dsl::name))
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().collect())
	}

	/// Bulk-fetch `(group_id, group_name)` for a set of machine ids, so a
	/// surface listing machine-scoped rows can name the group each belongs
	/// to without a query per row.
	pub async fn group_refs_by_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, (Option<Uuid>, Option<String>)>> {
		use crate::schema::{machines, server_groups};
		use std::collections::HashMap;

		if ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, Option<Uuid>, Option<String>)> = machines::table
			.left_join(server_groups::table.on(server_groups::id.nullable().eq(machines::group_id)))
			.select((
				machines::id,
				machines::group_id,
				server_groups::name.nullable(),
			))
			.filter(machines::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.map(|(id, gid, gn)| (id, (gid, gn)))
			.collect())
	}

	/// This machine's reachability, from when it last reported and its own
	/// down threshold.
	///
	/// A box's silence is its own fact. An application on it reports on its own
	/// schedule and against its own threshold, so a machine that has gone quiet
	/// and a workload that has are two different findings, and a box carrying
	/// two workloads still has one answer here.
	// spec: CHK#reachability
	pub fn reachability(&self, last_reported_at: Option<Timestamp>) -> ShortStatus {
		ShortStatus::grade(last_reported_at, self.alert_when_down_for.0)
	}

	/// Open the restore window for a machine: allow it to mint restore
	/// credentials for itself until [`RESTORE_WINDOW`] from now. Re-arming an
	/// already-open window resets the expiry. Returns the new expiry so callers
	/// can echo it back to the operator. `allowed_by` is the operator's identity
	/// (Tailscale login) for audit.
	// spec: BKO#allowing-a-restore
	pub async fn allow_restore(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		allowed_by: Option<&str>,
	) -> Result<Timestamp> {
		use crate::schema::machines::dsl;
		let until = Timestamp::now() + RESTORE_WINDOW;
		diesel::update(dsl::machines.filter(dsl::id.eq(machine_id)))
			.set((
				dsl::restore_allowed_until.eq(jiff_diesel::NullableTimestamp::from(Some(until))),
				dsl::restore_allowed_by.eq(allowed_by),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(until)
	}

	/// Close the restore window for a machine immediately (clears both the
	/// expiry and the recorded operator). Clearing an already-closed window is a
	/// no-op.
	// spec: BKO#allowing-a-restore
	pub async fn disallow_restore(db: &mut AsyncPgConnection, machine_id: Uuid) -> Result<()> {
		use crate::schema::machines::dsl;
		diesel::update(dsl::machines.filter(dsl::id.eq(machine_id)))
			.set((
				dsl::restore_allowed_until.eq(jiff_diesel::NullableTimestamp::from(None)),
				dsl::restore_allowed_by.eq::<Option<String>>(None),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Whether this machine's restore window is currently open (set and not yet
	/// expired).
	pub fn restore_allowed(&self) -> bool {
		self.restore_allowed_until
			.is_some_and(|until| until > Timestamp::now())
	}

	/// Several machines at once, for a view that has a page of them and would
	/// otherwise ask one query per box.
	pub async fn get_many(db: &mut AsyncPgConnection, ids: &[Uuid]) -> Result<Vec<Self>> {
		use crate::schema::machines::dsl;
		if ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::machines
			.select(Self::as_select())
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The applications running on this machine.
	pub async fn applications(
		&self,
		db: &mut AsyncPgConnection,
	) -> Result<Vec<crate::applications::Application>> {
		use crate::schema::applications::dsl;
		dsl::applications
			.select(crate::applications::Application::as_select())
			.filter(dsl::machine_id.eq(self.id))
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The environment this machine serves: the highest rank among the live
	/// applications on it, and none where they are all unranked.
	///
	/// A box is not given a production workload and a demo one, so deriving
	/// the rank leaves a mixed box well-defined without anyone keeping a
	/// machine's rank in step with what runs on it.
	// spec: FLT#environments
	pub async fn rank(
		db: &mut AsyncPgConnection,
		machine: Uuid,
	) -> Result<Option<commons_types::server::rank::ServerRank>> {
		use crate::schema::applications::dsl;
		let ranks: Vec<Option<commons_types::server::rank::ServerRank>> = dsl::applications
			.select(dsl::rank)
			.filter(dsl::machine_id.eq(machine))
			.filter(dsl::deleted_at.is_null())
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(ranks
			.into_iter()
			.flatten()
			.min_by_key(|rank| crate::server_groups::rank_priority(Some(*rank))))
	}

	/// This machine's tags over its group's, so a check filed against a
	/// machine is graded by policy against the tags of its own target rather
	/// than against some application that happens to run on it.
	///
	/// An application's type is not among them: it is not a property of a box.
	// spec: FLT#what-each-carries
	pub async fn tags_merged_with_group(&self, db: &mut AsyncPgConnection) -> Result<TagMap> {
		let Some(gid) = self.group_id else {
			return Ok(self.tags.clone());
		};
		let group = crate::server_groups::ServerGroup::get_by_id(db, gid).await?;
		Ok(self.tags.merged_with(&group.tags))
	}

	/// Archive a machine and, with it, the applications on it — a box going
	/// away takes its workloads with it. Archival is not deletion: the records
	/// and their history remain.
	///
	/// Releases the box's identity: the device is unbound and its credentials
	/// revoked, so a machine coming back has to come back through enrolment.
	/// Idempotent.
	// spec: FLT#archival
	pub async fn archive(db: &mut AsyncPgConnection, machine_id: Uuid) -> Result<()> {
		use diesel_async::AsyncConnection;

		db.transaction::<_, AppError, _>(async |conn| {
			let machine = Self::get_by_id_for_update(conn, machine_id).await?;
			if machine.deleted_at.is_some() {
				return Ok(());
			}

			if let Some(device_id) = machine.device_id {
				crate::devices::Device::revoke(conn, device_id).await?;
			}

			let now = jiff_diesel::Timestamp::from(Timestamp::now());
			diesel::update(crate::schema::machines::table)
				.filter(crate::schema::machines::id.eq(machine_id))
				.set((
					crate::schema::machines::deleted_at.eq(Some(now)),
					crate::schema::machines::device_id.eq(None::<Uuid>),
					crate::schema::machines::registered_at.eq(None::<jiff_diesel::Timestamp>),
				))
				.execute(conn)
				.await?;
			diesel::update(crate::schema::applications::table)
				.filter(crate::schema::applications::machine_id.eq(machine_id))
				.filter(crate::schema::applications::deleted_at.is_null())
				.set(crate::schema::applications::deleted_at.eq(Some(now)))
				.execute(conn)
				.await?;
			Ok(())
		})
		.await
	}
}
