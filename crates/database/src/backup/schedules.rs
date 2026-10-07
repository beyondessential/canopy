//! Where backup schedules are stored, and how a machine's resolves.
//!
//! A machine's schedule for a type is its own override when it has one,
//! otherwise its group's override, otherwise the fleet-wide default (see
//! [BKO](../../../../.workhorse/specs/private-server/backup.md)). Every layer
//! stores a schedule as the same three columns, and every set or clear of a
//! layer is appended to a history, which is also what works out when a
//! machine's resolved schedule last changed.
//!
//! [`ScheduleBook`] is the one place that resolution is done. The schedulers,
//! the staleness scan, the operator API and the fleet query interface all read
//! through it, so a machine is never commanded to back up on a cadence nothing
//! then monitors, or the reverse.

use std::collections::HashMap;

use commons_errors::{AppError, Result};
use commons_types::backup::{
	BackupType,
	schedule::{EffectiveSchedule, Schedule, ScheduleLayer, ZoneSource, resolve_zone},
};
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	backups::{BackupTypeDefault, NewBackupTypeDefault, ServerGroupBackupSchedule},
	pg_duration::PgDuration,
};

/// Which layer a history or an edit is about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LayerKey {
	Fleet,
	Group(Uuid),
	Machine(Uuid),
}

impl LayerKey {
	pub fn layer(self) -> ScheduleLayer {
		match self {
			Self::Fleet => ScheduleLayer::Fleet,
			Self::Group(_) => ScheduleLayer::Group,
			Self::Machine(_) => ScheduleLayer::Machine,
		}
	}

	fn group_id(self) -> Option<Uuid> {
		match self {
			Self::Group(id) => Some(id),
			_ => None,
		}
	}

	fn machine_id(self) -> Option<Uuid> {
		match self {
			Self::Machine(id) => Some(id),
			_ => None,
		}
	}
}

fn interval_of(secs: Option<i64>) -> Option<PgDuration> {
	secs.map(|s| PgDuration(SignedDuration::from_secs(s)))
}

// ---------------------------------------------------------------------------
// machine_backup_schedule — a machine's own override
// ---------------------------------------------------------------------------

/// A machine's override of its schedule for one backup type. Keyed by machine,
/// so it stays with the machine through a group move and through the type
/// being disabled and enabled again.
// spec: BKO#scheduling
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::machine_backup_schedule)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MachineBackupSchedule {
	pub machine_id: Uuid,
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	pub r#type: BackupType,
	pub expected_interval: Option<PgDuration>,
	pub expected_cron: Option<String>,
	pub schedule_zone: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

impl MachineBackupSchedule {
	pub fn schedule(&self) -> Schedule {
		Schedule::from_columns(
			self.expected_interval.map(|d| d.0.as_secs()),
			self.expected_cron.clone(),
			self.schedule_zone.clone(),
		)
	}

	pub async fn get(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
	) -> Result<Option<Self>> {
		use crate::schema::machine_backup_schedule::dsl;
		dsl::machine_backup_schedule
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::type_.eq(r#type.as_str()))
			.select(Self::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	pub async fn list_for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::machine_backup_schedule::dsl;
		dsl::machine_backup_schedule
			.filter(dsl::machine_id.eq(machine_id))
			.order(dsl::type_)
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every machine override in the group, for the group view and for what the
	/// escrow carries.
	pub async fn list_for_group(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::{machine_backup_schedule as ms, machines};
		ms::table
			.inner_join(machines::table.on(machines::id.eq(ms::machine_id)))
			.filter(machines::group_id.eq(group_id))
			.filter(machines::deleted_at.is_null())
			.order((ms::machine_id, ms::type_))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::machine_backup_schedule::dsl;
		dsl::machine_backup_schedule
			.order((dsl::machine_id, dsl::type_))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	async fn upsert(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
		schedule: &Schedule,
	) -> Result<()> {
		use crate::schema::machine_backup_schedule::dsl;
		let (interval, cron, zone) = schedule.to_columns();
		let interval = interval_of(interval);
		diesel::insert_into(dsl::machine_backup_schedule)
			.values((
				dsl::machine_id.eq(machine_id),
				dsl::type_.eq(r#type.as_str()),
				dsl::expected_interval.eq(interval),
				dsl::expected_cron.eq(&cron),
				dsl::schedule_zone.eq(&zone),
			))
			.on_conflict((dsl::machine_id, dsl::type_))
			.do_update()
			.set((
				dsl::expected_interval.eq(interval),
				dsl::expected_cron.eq(&cron),
				dsl::schedule_zone.eq(&zone),
				dsl::updated_at.eq(diesel::dsl::now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	async fn delete(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
	) -> Result<()> {
		use crate::schema::machine_backup_schedule::dsl;
		diesel::delete(
			dsl::machine_backup_schedule
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::type_.eq(r#type.as_str())),
		)
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// backup_schedule_history — every set and clear of a layer
// ---------------------------------------------------------------------------

/// One change to a layer: what it held after, who changed it, and when.
// spec: BKO#scheduling
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::backup_schedule_history)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ScheduleChange {
	pub id: i64,
	pub layer: String,
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	pub r#type: BackupType,
	pub group_id: Option<Uuid>,
	pub machine_id: Option<Uuid>,
	/// `manual`, `interval` or `cron`; none when the change cleared the layer.
	pub kind: Option<String>,
	pub interval: Option<PgDuration>,
	pub cron: Option<String>,
	pub zone: Option<String>,
	pub changed_by: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub changed_at: Timestamp,
}

impl ScheduleChange {
	/// What the layer held after the change; none if it was cleared.
	pub fn schedule(&self) -> Option<Schedule> {
		self.kind.as_ref()?;
		Some(Schedule::from_columns(
			self.interval.map(|d| d.0.as_secs()),
			self.cron.clone(),
			self.zone.clone(),
		))
	}

	fn key(&self) -> LayerKey {
		match (self.layer.as_str(), self.group_id, self.machine_id) {
			("machine", _, Some(m)) => LayerKey::Machine(m),
			("group", Some(g), _) => LayerKey::Group(g),
			_ => LayerKey::Fleet,
		}
	}

	async fn record(
		db: &mut AsyncPgConnection,
		key: LayerKey,
		r#type: &BackupType,
		schedule: Option<&Schedule>,
		by: Option<&str>,
	) -> Result<()> {
		use crate::schema::backup_schedule_history::dsl;
		let (kind, interval, cron, zone) = match schedule {
			None => (None, None, None, None),
			Some(schedule) => {
				let kind = match schedule {
					Schedule::Manual => "manual",
					Schedule::Interval { .. } => "interval",
					Schedule::Cron { .. } => "cron",
				};
				let (interval, cron, zone) = schedule.to_columns();
				(Some(kind), interval_of(interval), cron, zone)
			}
		};
		diesel::insert_into(dsl::backup_schedule_history)
			.values((
				dsl::layer.eq(key.layer().as_str()),
				dsl::type_.eq(r#type.as_str()),
				dsl::group_id.eq(key.group_id()),
				dsl::machine_id.eq(key.machine_id()),
				dsl::kind.eq(kind),
				dsl::interval.eq(interval),
				dsl::cron.eq(cron),
				dsl::zone.eq(zone),
				dsl::changed_by.eq(by),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// A layer's changes for one type, newest first.
	pub async fn list_for_layer(
		db: &mut AsyncPgConnection,
		key: LayerKey,
		r#type: &BackupType,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_schedule_history::dsl;
		let mut q = dsl::backup_schedule_history
			.filter(dsl::layer.eq(key.layer().as_str()))
			.filter(dsl::type_.eq(r#type.as_str()))
			.into_boxed();
		if let Some(group) = key.group_id() {
			q = q.filter(dsl::group_id.eq(group));
		}
		if let Some(machine) = key.machine_id() {
			q = q.filter(dsl::machine_id.eq(machine));
		}
		q.order((dsl::changed_at.desc(), dsl::id.desc()))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::backup_schedule_history::dsl;
		dsl::backup_schedule_history
			.order((dsl::changed_at, dsl::id))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}
}

// ---------------------------------------------------------------------------
// Setting and clearing layers
// ---------------------------------------------------------------------------

/// Write the fleet default for a type, recording the change in its history
/// when the schedule is different from what it held.
// spec: BKO#scheduling
pub async fn upsert_default(
	db: &mut AsyncPgConnection,
	new: NewBackupTypeDefault,
	by: Option<&str>,
) -> Result<BackupTypeDefault> {
	new.schedule()
		.validate()
		.map_err(|e| AppError::BadRequest(e.to_string()))?;
	db.transaction::<_, AppError, _>(async |conn| {
		let before = BackupTypeDefault::get(conn, &new.r#type)
			.await?
			.map(|d| d.schedule());
		let after = new.schedule();
		let ty = new.r#type.clone();
		let row = BackupTypeDefault::upsert(conn, new).await?;
		if before.as_ref() != Some(&after) {
			ScheduleChange::record(conn, LayerKey::Fleet, &ty, Some(&after), by).await?;
		}
		Ok(row)
	})
	.await
}

impl NewBackupTypeDefault {
	pub fn schedule(&self) -> Schedule {
		Schedule::from_columns(
			self.default_interval.map(|d| d.0.as_secs()),
			self.default_cron.clone(),
			self.default_zone.clone(),
		)
	}
}

/// Set a group's schedule override for a type, recording the change when it
/// is one.
// spec: BKO#scheduling
pub async fn set_group_schedule(
	db: &mut AsyncPgConnection,
	group_id: Uuid,
	r#type: &BackupType,
	schedule: &Schedule,
	by: Option<&str>,
) -> Result<()> {
	schedule
		.validate()
		.map_err(|e| AppError::BadRequest(e.to_string()))?;
	db.transaction::<_, AppError, _>(async |conn| {
		let before = ServerGroupBackupSchedule::get(conn, group_id, r#type)
			.await?
			.and_then(|s| s.schedule());
		ServerGroupBackupSchedule::set_schedule(conn, group_id, r#type, schedule).await?;
		if before.as_ref() != Some(schedule) {
			ScheduleChange::record(conn, LayerKey::Group(group_id), r#type, Some(schedule), by)
				.await?;
		}
		Ok(())
	})
	.await
}

/// Clear a group's schedule override for a type, recording the change when
/// there was one to clear.
// spec: BKO#scheduling
pub async fn clear_group_schedule(
	db: &mut AsyncPgConnection,
	group_id: Uuid,
	r#type: &BackupType,
	by: Option<&str>,
) -> Result<()> {
	db.transaction::<_, AppError, _>(async |conn| {
		let before = ServerGroupBackupSchedule::get(conn, group_id, r#type)
			.await?
			.and_then(|s| s.schedule());
		ServerGroupBackupSchedule::clear_schedule(conn, group_id, r#type).await?;
		if before.is_some() {
			ScheduleChange::record(conn, LayerKey::Group(group_id), r#type, None, by).await?;
		}
		Ok(())
	})
	.await
}

/// Set a machine's schedule override for a type, recording the change when it
/// is one.
// spec: BKO#scheduling
pub async fn set_machine_schedule(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#type: &BackupType,
	schedule: &Schedule,
	by: Option<&str>,
) -> Result<()> {
	schedule
		.validate()
		.map_err(|e| AppError::BadRequest(e.to_string()))?;
	db.transaction::<_, AppError, _>(async |conn| {
		let before = MachineBackupSchedule::get(conn, machine_id, r#type)
			.await?
			.map(|s| s.schedule());
		MachineBackupSchedule::upsert(conn, machine_id, r#type, schedule).await?;
		if before.as_ref() != Some(schedule) {
			ScheduleChange::record(
				conn,
				LayerKey::Machine(machine_id),
				r#type,
				Some(schedule),
				by,
			)
			.await?;
		}
		Ok(())
	})
	.await
}

/// Clear a machine's schedule override for a type, recording the change when
/// there was one to clear.
// spec: BKO#scheduling
pub async fn clear_machine_schedule(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#type: &BackupType,
	by: Option<&str>,
) -> Result<()> {
	db.transaction::<_, AppError, _>(async |conn| {
		let before = MachineBackupSchedule::get(conn, machine_id, r#type).await?;
		MachineBackupSchedule::delete(conn, machine_id, r#type).await?;
		if before.is_some() {
			ScheduleChange::record(conn, LayerKey::Machine(machine_id), r#type, None, by).await?;
		}
		Ok(())
	})
	.await
}

// ---------------------------------------------------------------------------
// machine_reported_timezone
// ---------------------------------------------------------------------------

/// The operating system timezone a machine last reported, and when it last
/// changed.
// spec: BKO#when-a-backup-is-due
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::machine_reported_timezone)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MachineReportedTimezone {
	pub machine_id: Uuid,
	pub timezone: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub changed_at: Timestamp,
}

impl MachineReportedTimezone {
	/// Note a timezone a machine's report carried. Its moment of change moves
	/// only when the zone it is read as differs from the last reported one's,
	/// so a machine repeating itself, renaming its zone, or swapping one
	/// unrecognised name for another takes nothing with it.
	pub async fn observe(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		timezone: &str,
	) -> Result<()> {
		use crate::schema::machine_reported_timezone::dsl;
		let read_as = |reported: &str| resolve_zone(None, Some(reported)).name;
		if let Some(held) = Self::get(db, machine_id).await? {
			if held.timezone == timezone {
				return Ok(());
			}
			if read_as(&held.timezone) == read_as(timezone) {
				diesel::update(
					dsl::machine_reported_timezone.filter(dsl::machine_id.eq(machine_id)),
				)
				.set(dsl::timezone.eq(timezone))
				.execute(db)
				.await
				.map_err(AppError::from)?;
				return Ok(());
			}
		}
		diesel::insert_into(dsl::machine_reported_timezone)
			.values((dsl::machine_id.eq(machine_id), dsl::timezone.eq(timezone)))
			.on_conflict(dsl::machine_id)
			.do_update()
			.set((
				dsl::timezone.eq(timezone),
				dsl::changed_at.eq(diesel::dsl::now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	pub async fn get(db: &mut AsyncPgConnection, machine_id: Uuid) -> Result<Option<Self>> {
		use crate::schema::machine_reported_timezone::dsl;
		dsl::machine_reported_timezone
			.filter(dsl::machine_id.eq(machine_id))
			.select(Self::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// The layers a machine's schedules resolve through, read once.
///
/// Loaded for the machines and groups a caller is about, or for everything,
/// which is small: it holds a schedule per override and a history entry per
/// change an operator made.
// spec: BKO#scheduling
#[derive(Debug, Default)]
pub struct ScheduleBook {
	defaults: HashMap<BackupType, Schedule>,
	groups: HashMap<(Uuid, BackupType), Schedule>,
	machines: HashMap<(Uuid, BackupType), Schedule>,
	history: HashMap<(LayerKey, BackupType), Vec<ScheduleChange>>,
	zones: HashMap<Uuid, MachineReportedTimezone>,
}

impl ScheduleBook {
	/// Load the layers for these groups and machines, or for all of them.
	pub async fn load(
		db: &mut AsyncPgConnection,
		groups: Option<&[Uuid]>,
		machines: Option<&[Uuid]>,
	) -> Result<Self> {
		let mut book = Self::default();

		for default in BackupTypeDefault::list(db).await? {
			book.defaults
				.insert(default.r#type.clone(), default.schedule());
		}

		{
			use crate::schema::server_group_backup_schedule::dsl;
			let mut q = dsl::server_group_backup_schedule.into_boxed();
			if let Some(groups) = groups {
				q = q.filter(dsl::group_id.eq_any(groups.to_vec()));
			}
			let rows: Vec<ServerGroupBackupSchedule> = q.load(db).await?;
			for row in rows {
				if let Some(schedule) = row.schedule() {
					book.groups.insert((row.group_id, row.r#type), schedule);
				}
			}
		}

		{
			use crate::schema::machine_backup_schedule::dsl;
			let mut q = dsl::machine_backup_schedule
				.select(MachineBackupSchedule::as_select())
				.into_boxed();
			if let Some(machines) = machines {
				q = q.filter(dsl::machine_id.eq_any(machines.to_vec()));
			}
			for row in q.load(db).await? {
				book.machines
					.insert((row.machine_id, row.r#type.clone()), row.schedule());
			}
		}

		{
			use crate::schema::backup_schedule_history::dsl;
			let mut q = dsl::backup_schedule_history
				.select(ScheduleChange::as_select())
				.order((dsl::changed_at, dsl::id))
				.into_boxed();
			if groups.is_some() || machines.is_some() {
				q = q.filter(
					dsl::layer.eq("fleet").or(dsl::group_id
						.eq_any(groups.unwrap_or_default().to_vec())
						.or(dsl::machine_id.eq_any(machines.unwrap_or_default().to_vec()))),
				);
			}
			for change in q.load(db).await? {
				book.history
					.entry((change.key(), change.r#type.clone()))
					.or_default()
					.push(change);
			}
		}

		{
			use crate::schema::machine_reported_timezone::dsl;
			let mut q = dsl::machine_reported_timezone
				.select(MachineReportedTimezone::as_select())
				.into_boxed();
			if let Some(machines) = machines {
				q = q.filter(dsl::machine_id.eq_any(machines.to_vec()));
			}
			for zone in q.load(db).await? {
				book.zones.insert(zone.machine_id, zone);
			}
		}

		Ok(book)
	}

	/// The operating system timezone the machine last reported, as reported.
	pub fn reported_zone(&self, machine_id: Uuid) -> Option<&str> {
		self.zones.get(&machine_id).map(|z| z.timezone.as_str())
	}

	/// The schedule a machine has for a type, where it comes from, the zone it
	/// is read in, and when it took effect.
	// spec: BKO#scheduling
	pub fn resolve(
		&self,
		machine_id: Uuid,
		group_id: Option<Uuid>,
		r#type: &BackupType,
	) -> EffectiveSchedule {
		let (schedule, layer) = if let Some(s) = self.machines.get(&(machine_id, r#type.clone())) {
			(s.clone(), Some(ScheduleLayer::Machine))
		} else if let Some(s) = group_id.and_then(|g| self.groups.get(&(g, r#type.clone()))) {
			(s.clone(), Some(ScheduleLayer::Group))
		} else if let Some(s) = self.defaults.get(r#type) {
			(s.clone(), Some(ScheduleLayer::Fleet))
		} else {
			(Schedule::Manual, None)
		};

		let reported = self.zones.get(&machine_id);
		let zone = match &schedule {
			Schedule::Cron { zone, .. } => Some(resolve_zone(
				zone.as_deref(),
				reported.map(|r| r.timezone.as_str()),
			)),
			_ => None,
		};

		let layers_since = self.layers_changed_at(machine_id, group_id, r#type);
		let mut since = layers_since;
		// A zone read off the machine takes effect anew when the machine
		// reports a different one.
		if let (Some(zone), Some(reported)) = (&zone, reported)
			&& zone.source != ZoneSource::Schedule
		{
			since = Some(since.map_or(reported.changed_at, |s| s.max(reported.changed_at)));
		}

		EffectiveSchedule {
			schedule,
			layer,
			zone,
			since,
			layers_since,
		}
	}

	/// The latest change to any layer that altered what the machine's schedule
	/// resolves to, found by replaying the layers' histories in order. A change
	/// to a layer another shadows resolves to the same thing and so doesn't
	/// count; clearing the shadowing one does.
	fn layers_changed_at(
		&self,
		machine_id: Uuid,
		group_id: Option<Uuid>,
		r#type: &BackupType,
	) -> Option<Timestamp> {
		let mut changes: Vec<&ScheduleChange> = Vec::new();
		let mut gather = |key: LayerKey| {
			if let Some(entries) = self.history.get(&(key, r#type.clone())) {
				changes.extend(entries);
			}
		};
		gather(LayerKey::Fleet);
		if let Some(group) = group_id {
			gather(LayerKey::Group(group));
		}
		gather(LayerKey::Machine(machine_id));
		changes.sort_by_key(|c| (c.changed_at, c.id));

		let (mut fleet, mut group, mut machine) = (None, None, None);
		let resolved =
			|fleet: &Option<Schedule>, group: &Option<Schedule>, machine: &Option<Schedule>| {
				machine
					.clone()
					.or_else(|| group.clone())
					.or_else(|| fleet.clone())
					.unwrap_or(Schedule::Manual)
			};
		let mut current = resolved(&fleet, &group, &machine);
		let mut since = None;
		for change in changes {
			match change.key() {
				LayerKey::Fleet => fleet = change.schedule(),
				LayerKey::Group(_) => group = change.schedule(),
				LayerKey::Machine(_) => machine = change.schedule(),
			}
			let after = resolved(&fleet, &group, &machine);
			if after != current {
				since = Some(change.changed_at);
				current = after;
			}
		}
		since
	}
}
