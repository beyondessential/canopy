//! The windows Canopy opens when an Ansible run starts.
//!
//! A run's window is bound to the run's lease and covers what the run acts on,
//! for the operator holding the lease, who is its declarer. It lasts the run
//! rather than to an end of its own: its expected end follows the lease's expiry
//! while the lease is held, so the sweep that ends any expired window ends it
//! when the lease runs out, and it ends when the lease is released. Its
//! declarer can hold it, giving it an expected end, which frees it from the
//! lease.
// spec: ANSRUN#work-under-way

use std::collections::HashSet;

use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use uuid::Uuid;

use super::{Cover, Ended, Grain, Insert, MaintenanceWindow, cover_columns, ended_is_history};
use crate::ansible_leases::AnsibleLease;
use crate::machines::Machine;

/// The windows covering a run, as its first read finds them.
#[derive(Clone, Debug, Default)]
pub struct RunWindows {
	/// The windows this declared for the run.
	pub declared: Vec<MaintenanceWindow>,
	/// The holder's own windows that already cover part of what the run acts
	/// on, and serve for what they cover.
	pub serving: Vec<MaintenanceWindow>,
}

impl MaintenanceWindow {
	/// Declare the window for a run starting under `lease`, over what it acts
	/// on: the lease's environment, or only `machine_ids` of its machines. The
	/// lease's holder is the declarer.
	///
	/// A window of the holder's own that already covers any of that serves for
	/// what it covers, and a window of the run's covers the rest, so nothing is
	/// declared where the holder's windows cover it all. A machine another
	/// operator's window names cannot be covered, and refuses with a conflict
	/// rather than being named twice.
	///
	/// Takes the group's row first, so this and a take of the lease, a move, or
	/// another declaration over the group's machines are decided one after
	/// another.
	// spec: ANSRUN#work-under-way
	pub async fn declare_for_run(
		db: &mut AsyncPgConnection,
		lease: &AnsibleLease,
		machine_ids: Option<&[Uuid]>,
		note: Option<&str>,
	) -> Result<RunWindows> {
		let now = Timestamp::now();
		if !lease.holds_at(now) {
			return Err(AppError::BadRequest(
				"that lease no longer holds, so there is no run to declare maintenance for".into(),
			));
		}
		let (group, rank) = (lease.server_group_id, lease.rank);
		let holder = lease.held_by.as_deref();

		let windows = db
			.transaction::<_, AppError, _>(async |conn| {
				crate::ansible_leases::lock_group(conn, group).await?;

				let members: Vec<Uuid> = Machine::list_for_group(conn, group)
					.await?
					.into_iter()
					.filter(|machine| machine.environment_rank() == Some(rank))
					.map(|machine| machine.id)
					.collect();
				if members.is_empty() {
					return Ok(RunWindows::default());
				}
				let acts_on: Vec<Uuid> = match machine_ids {
					None => members.clone(),
					Some(ids) => {
						if let Some(stray) = ids.iter().find(|id| !members.contains(id)) {
							return Err(AppError::BadRequest(format!(
								"machine {stray} is not in the lease's environment"
							)));
						}
						let mut ids = ids.to_vec();
						ids.sort();
						ids.dedup();
						if ids.is_empty() {
							return Err(AppError::BadRequest(
								"a run limited to no machine acts on nothing".into(),
							));
						}
						ids
					}
				};

				// A window past its expected end is over, whether or not the sweep
				// has stamped it yet, so it is ended here rather than left to name
				// the machines this run acts on.
				let open = Self::open_over(conn, group, rank, &members).await?;
				let past: Vec<Uuid> = open
					.iter()
					.filter(|window| !window.holds_at(now))
					.map(|window| window.id)
					.collect();
				let open: Vec<Self> = if past.is_empty() {
					open
				} else {
					Self::end_expired(conn, Some(&past)).await?;
					open.into_iter()
						.filter(|window| !past.contains(&window.id))
						.collect()
				};
				let mut covered: HashSet<Uuid> = HashSet::new();
				let mut serving: Vec<Self> = Vec::new();
				for window in &open {
					let speaks_for = holder.is_some()
						&& (window.declared_by.as_deref() == holder
							|| window.amended_by.as_deref() == holder);
					if !speaks_for || !window.holds_at(now) {
						continue;
					}
					let named = window.named_machines();
					let reaches: Vec<Uuid> = if named.is_empty() {
						acts_on.clone()
					} else {
						acts_on
							.iter()
							.copied()
							.filter(|machine| named.contains(machine))
							.collect()
					};
					if !reaches.is_empty() {
						covered.extend(reaches);
						serving.push(window.clone());
					}
				}

				let remainder: Vec<Uuid> = acts_on
					.iter()
					.copied()
					.filter(|machine| !covered.contains(machine))
					.collect();
				if remainder.is_empty() {
					return Ok(RunWindows {
						declared: Vec::new(),
						serving,
					});
				}

				let held_elsewhere = open.iter().find(|window| {
					!serving.iter().any(|own| own.id == window.id)
						&& window
							.named_machines()
							.iter()
							.any(|machine| remainder.contains(machine))
				});
				if let Some(window) = held_elsewhere {
					return Err(AppError::Conflict(format!(
						"a machine this run acts on is named by a window {}",
						window.declared_by.as_deref().map_or_else(
							|| "already open".to_string(),
							|by| format!("declared by {by}")
						),
					)));
				}

				let cover = if remainder.len() == members.len() {
					Cover::Grain(Grain::environment(group, rank))
				} else {
					Cover::machines(remainder)?
				};
				if !Self::open_for_cover_unchecked(conn, &cover)
					.await?
					.is_empty()
				{
					return Err(AppError::Conflict(
						"what this run acts on already has a window of its own".into(),
					));
				}
				let columns = cover_columns(conn, &cover).await?;
				let declared = Self::insert(
					conn,
					&columns,
					lease.expires_at,
					note,
					holder,
					Insert {
						plan: None,
						lease: Some(lease.id),
					},
				)
				.await?;
				Ok(RunWindows {
					declared: vec![declared],
					serving,
				})
			})
			.await?;

		let closed_because = match holder {
			Some(login) => format!("maintenance declared by {login}"),
			None => "maintenance being declared".to_string(),
		};
		for window in &windows.declared {
			window.announce_declared(db).await?;
			window.reevaluate_covered(db, Some(&closed_because)).await?;
		}
		Ok(windows)
	}

	/// The open windows still bound to `lease_id`.
	pub async fn bound_to_lease(db: &mut AsyncPgConnection, lease_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(dsl::lease_id.eq(lease_id))
			.filter(dsl::ended_at.is_null())
			.order(dsl::declared_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// End every open window still bound to the lease, which has ended. A window
	/// is over when its lease is, and no later than its expected end where the
	/// lease had already run out. Ending twice changes nothing.
	pub async fn end_for_lease(
		db: &mut AsyncPgConnection,
		lease_id: Uuid,
		by: Option<&str>,
	) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		let mut ended = Vec::new();
		for window in Self::bound_to_lease(db, lease_id).await? {
			let at = now.min(window.expected_end);
			let updated: Option<Self> = diesel::update(
				dsl::maintenance_windows
					.filter(dsl::id.eq(window.id))
					.filter(dsl::ended_at.is_null()),
			)
			.set((
				dsl::ended_at.eq(jiff_diesel::Timestamp::from(at)),
				dsl::ended_by.eq(by),
				dsl::updated_at.eq(jiff_diesel::Timestamp::from(now)),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.optional()
			.map_err(AppError::from)?;
			if let Some(window) = updated {
				window.announce_ended(db, Ended::Lease).await?;
				ended.push(window);
			}
		}
		Ok(ended)
	}

	/// Follow the lease's expiry with the expected end of the windows bound to
	/// it, so they last as long as the lease holds.
	pub async fn track_lease(
		db: &mut AsyncPgConnection,
		lease_id: Uuid,
		expires_at: Timestamp,
	) -> Result<()> {
		use crate::schema::maintenance_windows::dsl;
		diesel::update(
			dsl::maintenance_windows
				.filter(dsl::lease_id.eq(lease_id))
				.filter(dsl::ended_at.is_null()),
		)
		.set((
			dsl::expected_end.eq(jiff_diesel::Timestamp::from(expires_at)),
			dsl::updated_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
		))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	/// Pass the open windows bound to the lease to the operator who took the
	/// lease over, as an amendment: suspension does not lapse partway through
	/// the work, and the new holder is served the environment the windows cover.
	pub async fn pass_lease(
		db: &mut AsyncPgConnection,
		lease_id: Uuid,
		new_holder: &str,
	) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		let now = jiff_diesel::Timestamp::from(Timestamp::now());
		diesel::update(
			dsl::maintenance_windows
				.filter(dsl::lease_id.eq(lease_id))
				.filter(dsl::ended_at.is_null()),
		)
		.set((
			dsl::amended_by.eq(new_holder),
			dsl::amended_at.eq(now),
			dsl::updated_at.eq(now),
		))
		.returning(Self::as_select())
		.get_results(db)
		.await
		.map_err(AppError::from)
	}

	/// Give a window that lasts a run an expected end, so it no longer ends with
	/// the lease but as any window does. Only the operator whose work it is can:
	/// its declarer, or the one whose amendment stands. Recorded as an
	/// amendment.
	// spec: MNT#ending
	pub async fn hold(
		db: &mut AsyncPgConnection,
		id: Uuid,
		expected_end: Timestamp,
		by: Option<&str>,
	) -> Result<Self> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		if expected_end <= now {
			return Err(AppError::BadRequest(
				"a maintenance window ends in the future".into(),
			));
		}
		db.transaction::<_, AppError, _>(async |conn| {
			let window: Self = dsl::maintenance_windows
				.select(Self::as_select())
				.filter(dsl::id.eq(id))
				.for_update()
				.first(conn)
				.await
				.map_err(AppError::from)?;
			if window.ended_at.is_some() {
				return Err(ended_is_history());
			}
			if window.lease_id.is_none() {
				return Err(AppError::BadRequest(
					"only a window that lasts a run is held; amend this one's end instead".into(),
				));
			}
			let speaks_for = [window.declared_by.as_deref(), window.amended_by.as_deref()];
			if by.is_none() || !speaks_for.contains(&by) {
				return Err(AppError::Conflict(
					"only the operator whose run this is can hold its window".into(),
				));
			}
			diesel::update(dsl::maintenance_windows.filter(dsl::id.eq(id)))
				.set((
					dsl::expected_end.eq(jiff_diesel::Timestamp::from(expected_end)),
					dsl::lease_id.eq(None::<Uuid>),
					dsl::amended_by.eq(by),
					dsl::amended_at.eq(jiff_diesel::Timestamp::from(now)),
					dsl::updated_at.eq(jiff_diesel::Timestamp::from(now)),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await
				.map_err(AppError::from)
		})
		.await
	}
}
