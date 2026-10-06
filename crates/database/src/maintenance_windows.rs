//! Operator declarations that an application, a machine, a group, or one of a
//! group's environments is being worked on.
//!
//! While a window suspends a target its checks are observed, graded, and
//! presented exactly as they would be without it. What the window holds back
//! is what those results feed: no issue on the target opens or joins an
//! incident, so nothing notifies, and an operator working through a window
//! watches the check they are fixing come good.
//!
//! A window over a machine covers every application on it: taking a box down
//! to patch it stops everything running on it, so that is one declaration with
//! N consequences. A window over one application covers that application
//! alone, for work stopping one product on a box that serves several.
//!
//! Suspension outlasts the window itself by [`SETTLE`]. A machine is back
//! before the sources on it have reported again, and a machine whose every
//! source is stale is unreachable, so ending suspension the instant the work
//! finishes would page for a server that has just come back, for as long as
//! the work took.

use std::collections::{HashMap, HashSet};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::applications::Application;
use crate::inventory_leases::InventoryLease;
use crate::issues::Scope;
use crate::machines::Machine;
use crate::server_groups::{ServerGroup, environment_name};
use crate::slack_outbox::{KIND_MAINTENANCE_DECLARED, KIND_MAINTENANCE_ENDED, SlackOutbox, vars};
use crate::upgrade_plans::UpgradePlan;

mod descent;
pub use descent::{Descent, DescentEntry, Grain, line_of_descent};

/// How long suspension outlasts the window, giving the reporters on a
/// server time to be heard from before Canopy pages for them. The same for
/// every window.
pub const SETTLE: SignedDuration = SignedDuration::from_mins(10);

/// The targets a window covers right now, which of them it still holds rather
/// than settling over, and at which grain each window was declared.
///
/// The grain is kept rather than flattened to the machines a window reaches: a
/// reader who cannot tell an environment's window from every box in that
/// environment having its own has lost the fact the operator declared.
// spec: MNT#presentation
#[derive(Clone, Debug, Default)]
pub struct SuspendedTargets {
	pub applications: HashSet<Uuid>,
	pub machines: HashSet<Uuid>,
	pub environments: HashSet<(Uuid, ServerRank)>,
	pub groups: HashSet<Uuid>,
	pub holding_applications: HashSet<Uuid>,
	pub holding_machines: HashSet<Uuid>,
	pub holding_environments: HashSet<(Uuid, ServerRank)>,
	pub holding_groups: HashSet<Uuid>,
	/// The environment each machine serves, for the boxes an environment
	/// window reaches, so a caller holding a machine id can answer without
	/// going back to the database.
	covered_by_environment: HashMap<Uuid, (Uuid, ServerRank)>,
}

/// A window's targets with the two timestamps that say whether it still holds.
type SuspensionRow = (
	Option<Uuid>,
	Option<Uuid>,
	Option<Uuid>,
	Option<ServerRank>,
	Option<jiff_diesel::Timestamp>,
	jiff_diesel::Timestamp,
);

impl SuspendedTargets {
	/// Is this box suspended at all, by its own window, its environment's or
	/// its group's?
	pub fn suspends(&self, machine: Uuid, group: Option<Uuid>) -> bool {
		self.machines.contains(&machine)
			|| self.in_suspended_environment(machine)
			|| group.is_some_and(|g| self.groups.contains(&g))
	}

	/// Is every window over this box ended, leaving it in the settle period?
	pub fn settling(&self, machine: Uuid, group: Option<Uuid>) -> bool {
		self.suspends(machine, group)
			&& !self.holding_machines.contains(&machine)
			&& !self.in_holding_environment(machine)
			&& !group.is_some_and(|g| self.holding_groups.contains(&g))
	}

	/// Is this application suspended, by a window of its own or by any window
	/// over the box it runs on? A cluster-hosted application has no box
	/// (`machine` is `None`), so only a window over it or its group reaches it.
	pub fn suspends_application(
		&self,
		application: Uuid,
		machine: Option<Uuid>,
		group: Option<Uuid>,
	) -> bool {
		self.applications.contains(&application)
			|| match machine {
				Some(machine) => self.suspends(machine, group),
				None => group.is_some_and(|g| self.groups.contains(&g)),
			}
	}

	/// Is every window covering this application ended, leaving it in the
	/// settle period?
	pub fn settling_application(
		&self,
		application: Uuid,
		machine: Option<Uuid>,
		group: Option<Uuid>,
	) -> bool {
		self.suspends_application(application, machine, group)
			&& !self.holding_applications.contains(&application)
			&& machine.is_none_or(|machine| {
				!self.holding_machines.contains(&machine) && !self.in_holding_environment(machine)
			}) && !group.is_some_and(|g| self.holding_groups.contains(&g))
	}

	/// A window declared over this application in particular, as against one
	/// reaching it through the box it runs on.
	pub fn application_window(&self, application: Uuid) -> bool {
		self.applications.contains(&application)
	}

	/// A window declared over this box in particular, as against one it falls
	/// under through its environment or its group.
	pub fn machine_window(&self, machine: Uuid) -> bool {
		self.machines.contains(&machine)
	}

	/// A window declared over one of a group's environments.
	pub fn environment_window(&self, group: Uuid, rank: ServerRank) -> bool {
		self.environments.contains(&(group, rank))
	}

	pub fn environment_window_settling(&self, group: Uuid, rank: ServerRank) -> bool {
		self.environments.contains(&(group, rank))
			&& !self.holding_environments.contains(&(group, rank))
	}

	/// A window over one of a group's environments that is still holding, rather
	/// than one that has ended and is serving out the settle period. The settle
	/// period suppresses alerts; it does not mean anyone is still working.
	pub fn environment_holding(&self, group: Uuid, rank: ServerRank) -> bool {
		self.holding_environments.contains(&(group, rank))
	}

	/// A window over the group itself that is still holding.
	pub fn group_holding(&self, group: Uuid) -> bool {
		self.holding_groups.contains(&group)
	}

	/// A window declared over the group itself.
	pub fn group_window(&self, group: Uuid) -> bool {
		self.groups.contains(&group)
	}

	pub fn group_window_settling(&self, group: Uuid) -> bool {
		self.groups.contains(&group) && !self.holding_groups.contains(&group)
	}

	fn in_suspended_environment(&self, machine: Uuid) -> bool {
		self.covered_by_environment
			.get(&machine)
			.is_some_and(|env| self.environments.contains(env))
	}

	fn in_holding_environment(&self, machine: Uuid) -> bool {
		self.covered_by_environment
			.get(&machine)
			.is_some_and(|env| self.holding_environments.contains(env))
	}
}

/// A declaration that an application, a machine, a group, or one of a group's
/// environments is being worked on.
#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::maintenance_windows)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MaintenanceWindow {
	/// Unique identifier of this window.
	pub id: Uuid,
	/// Set for a window over one application, covering that application's
	/// checks and nothing else on the box it runs on.
	pub application_id: Option<Uuid>,
	/// Set for a window over one machine, covering the machine's own checks
	/// and those of every application running on it.
	pub machine_id: Option<Uuid>,
	/// Set for a window over a group, covering the group's own checks and
	/// those of every machine in it.
	pub server_group_id: Option<Uuid>,
	/// Set with `server_group_id` for a window over one of the group's
	/// environments, covering the machines serving that environment and
	/// nothing else of the group.
	pub rank: Option<ServerRank>,
	/// When the operator expects the work to finish. A window reaching it
	/// ends itself.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub expected_end: Timestamp,
	/// What is being done, where the operator said.
	pub note: Option<String>,
	/// The operator who declared the window. `None` if not recorded.
	pub declared_by: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	/// When the window was declared.
	pub declared_at: Timestamp,
	/// The operator who last amended the window, where one has.
	pub amended_by: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	/// When the window was last amended.
	pub amended_at: Option<Timestamp>,
	/// When the window stopped holding. `None` while it holds.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub ended_at: Option<Timestamp>,
	/// The operator who lifted the window. `None` where its expected end
	/// passed instead.
	pub ended_by: Option<String>,
	/// Stamped once the settle period has elapsed and the target's issues
	/// have been re-evaluated.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub settled_at: Option<Timestamp>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	/// When this record was created.
	pub created_at: Timestamp,
	/// When this record was last modified.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// The upgrade plan this window was declared from, which keeps it over the
	/// plan's environment for as long as it holds.
	pub upgrade_plan_id: Option<Uuid>,
}

/// A target a window covered before it moved, over the span it covered it.
///
/// What a move leaves uncovered settles as though the window had ended over
/// it, so a move is read as settling until [`SETTLE`] after it, and remains
/// that target's maintenance history after.
// spec: MNT#moving-a-window
#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::maintenance_window_moves)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MaintenanceWindowMove {
	pub id: Uuid,
	/// The window that moved.
	pub window_id: Uuid,
	pub application_id: Option<Uuid>,
	pub machine_id: Option<Uuid>,
	pub server_group_id: Option<Uuid>,
	pub rank: Option<ServerRank>,
	/// When the window started covering this target.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub covered_from: Timestamp,
	/// When the window moved off it.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub moved_at: Timestamp,
	/// The operator who moved it.
	pub moved_by: Option<String>,
	/// Stamped once the settle period after the move has elapsed and this
	/// target's issues have been re-evaluated.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub settled_at: Option<Timestamp>,
}

impl MaintenanceWindowMove {
	pub fn grain(&self) -> Option<Grain> {
		Grain::from_columns(
			self.application_id,
			self.machine_id,
			self.server_group_id,
			self.rank,
		)
	}

	/// The most recent move of `window`, where it has moved.
	pub async fn latest_for_window(
		db: &mut AsyncPgConnection,
		window: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::maintenance_window_moves::dsl;
		dsl::maintenance_window_moves
			.select(Self::as_select())
			.filter(dsl::window_id.eq(window))
			.order(dsl::moved_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}
}

/// What an amendment changes. A field left `None` keeps the window's own, so
/// amending someone else's window changes only what the operator chose to.
// spec: MNT#choosing-what-to-cover
#[derive(Clone, Debug, Default)]
pub struct Amendment {
	/// A new target, moving the window there.
	pub target: Option<Grain>,
	pub expected_end: Option<Timestamp>,
	/// `Some(None)` clears the note.
	pub note: Option<Option<String>>,
}

/// What keeps a window over the target it covers.
// spec: MNT#moving-a-window
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum HeldInPlace {
	/// It was declared from this upgrade plan, which it holds open.
	UpgradePlan {
		/// The plan it was declared from.
		plan_id: Uuid,
	},
	/// A configuration run's lease is being served against it.
	RunLease {
		/// The operator running.
		held_by: Option<String>,
	},
}

/// One span of a window over a target, as the target's history reads it.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct TargetWindow {
	/// The window, as it stands now.
	pub window: MaintenanceWindow,
	/// When the window started covering this target: its declaration, or the
	/// move that brought it here.
	#[schema(value_type = String, format = DateTime)]
	pub covered_from: Timestamp,
	/// When the window moved off this target, for a span that ended that way.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub moved_at: Option<Timestamp>,
	/// Where it moved to, as it reads to an operator.
	pub moved_to: Option<String>,
	/// The environment this span covered, where it was over one of a group's
	/// environments rather than the target as a whole. For a span that moved
	/// off, this is not the window's `rank`, which is where it went.
	pub covered_rank: Option<ServerRank>,
}

impl TargetWindow {
	/// When this span stopped covering its target: the move off it, or the
	/// window ending. `None` while it still covers it.
	pub fn ended(&self) -> Option<Timestamp> {
		self.moved_at.or(self.window.ended_at)
	}
}

impl MaintenanceWindow {
	/// The grain this window covers now.
	pub fn grain(&self) -> Option<Grain> {
		Grain::from_columns(
			self.application_id,
			self.machine_id,
			self.server_group_id,
			self.rank,
		)
	}

	/// When this window started covering its current target: the move that
	/// brought it there, or its declaration where it has never moved.
	pub fn covered_from(&self, latest_move: Option<&MaintenanceWindowMove>) -> Timestamp {
		latest_move.map_or(self.declared_at, |last| last.moved_at)
	}

	/// The target this window covers.
	pub fn scope(&self) -> Scope {
		// A maintenance window never covers a cluster (see `fleet_columns`), so
		// there is no cluster column to read back here.
		Scope::from_columns(
			self.application_id,
			self.machine_id,
			self.server_group_id,
			None,
		)
	}

	/// When the window itself ended, or is due to: an operator's lift where
	/// there was one, the expected end otherwise.
	pub fn window_end(&self) -> Timestamp {
		self.ended_at.unwrap_or(self.expected_end)
	}

	/// When suspension over the target ends.
	pub fn suspension_end(&self) -> Timestamp {
		self.window_end() + SETTLE
	}

	/// Is the window still holding, as opposed to settling or over?
	pub fn holds_at(&self, now: Timestamp) -> bool {
		self.ended_at.is_none() && now < self.expected_end
	}

	/// Is the target still suspended, whether the window holds or is
	/// settling?
	pub fn suspends_at(&self, now: Timestamp) -> bool {
		now < self.suspension_end()
	}

	/// Declare a window over `scope`, narrowed to one of the group's
	/// environments where `rank` is given, or amend the open one if the target
	/// already has it: a target has at most one open window.
	pub async fn declare(
		db: &mut AsyncPgConnection,
		scope: Scope,
		rank: Option<ServerRank>,
		expected_end: Timestamp,
		note: Option<&str>,
		by: Option<&str>,
	) -> Result<Self> {
		Self::declare_inner(db, scope, rank, expected_end, note, by, None).await
	}

	/// Declare over an upgrade plan's environment from the plan's offer. A
	/// window this opens is the plan's and stays over its environment; one the
	/// environment already had is amended and stays the operator's own.
	// spec: MNT#moving-a-window
	pub async fn declare_from_plan(
		db: &mut AsyncPgConnection,
		plan: Uuid,
		scope: Scope,
		rank: Option<ServerRank>,
		expected_end: Timestamp,
		note: Option<&str>,
		by: Option<&str>,
	) -> Result<Self> {
		let plan = UpgradePlan::get_open(db, plan).await?;
		if scope != Scope::Group(plan.group_id) || rank != Some(plan.rank) {
			return Err(AppError::BadRequest(
				"a window declared from an upgrade plan is over the plan's environment".into(),
			));
		}
		Self::declare_inner(
			db,
			Scope::Group(plan.group_id),
			Some(plan.rank),
			expected_end,
			note,
			by,
			Some(plan.id),
		)
		.await
	}

	async fn declare_inner(
		db: &mut AsyncPgConnection,
		scope: Scope,
		rank: Option<ServerRank>,
		expected_end: Timestamp,
		note: Option<&str>,
		by: Option<&str>,
		plan: Option<Uuid>,
	) -> Result<Self> {
		use crate::schema::maintenance_windows::dsl;
		let Some((application, machine, group)) = fleet_columns(scope) else {
			return Err(AppError::BadRequest(
				"a maintenance window covers an application, a machine or a group".into(),
			));
		};
		if rank.is_some() && group.is_none() {
			return Err(AppError::BadRequest(
				"an environment is a group's applications at one rank, so a window over one names the group".into(),
			));
		}
		if expected_end <= Timestamp::now() {
			return Err(AppError::BadRequest(
				"a maintenance window ends in the future".into(),
			));
		}

		if let Some(open) = Self::open_for(db, scope, rank).await? {
			return diesel::update(dsl::maintenance_windows.filter(dsl::id.eq(open.id)))
				.set((
					dsl::expected_end.eq(jiff_diesel::Timestamp::from(expected_end)),
					dsl::note.eq(note),
					dsl::amended_by.eq(by),
					dsl::amended_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
					dsl::updated_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
				))
				.returning(Self::as_select())
				.get_result(db)
				.await
				.map_err(AppError::from);
		}

		let window: Self = diesel::insert_into(dsl::maintenance_windows)
			.values((
				dsl::application_id.eq(application),
				dsl::machine_id.eq(machine),
				dsl::server_group_id.eq(group),
				dsl::rank.eq(rank),
				dsl::expected_end.eq(jiff_diesel::Timestamp::from(expected_end)),
				dsl::note.eq(note),
				dsl::declared_by.eq(by),
				dsl::upgrade_plan_id.eq(plan),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;

		let label = target_label(db, scope, rank).await?;
		SlackOutbox::enqueue(
			db,
			KIND_MAINTENANCE_DECLARED,
			None,
			None,
			None,
			vars::maintenance_declared(&label, by, &format_when(expected_end), note),
			Timestamp::now(),
		)
		.await?;

		// The target's issues leave their incident now, rather than waiting
		// for each check to be re-graded on its next report.
		let closed_because = match by {
			Some(login) => format!("maintenance declared by {login}"),
			None => "maintenance being declared".to_string(),
		};
		crate::issues::reevaluate_open_issues_for_scope(db, scope, Some(&closed_because)).await?;

		Ok(window)
	}

	/// Amend an open window, changing only what `amendment` names. A new
	/// target moves the window there: it stays the same window, the target it
	/// left settles as though the window had ended over it, and what it newly
	/// covers is suspended from now. A move is recorded and audited like any
	/// amendment, and notifies no one.
	// spec: MNT#moving-a-window
	pub async fn amend(
		db: &mut AsyncPgConnection,
		id: Uuid,
		amendment: Amendment,
		by: Option<&str>,
	) -> Result<Self> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		if amendment.expected_end.is_some_and(|end| end <= now) {
			return Err(AppError::BadRequest(
				"a maintenance window ends in the future".into(),
			));
		}

		// The window is read locked, and the checks run against that read, so a
		// concurrent amendment waits for this one rather than writing back what
		// it read before, and a move is refused or made against what it stands
		// on when it is written.
		let (amended, to) = db
			.transaction::<_, AppError, _>(async |conn| {
				let window: Self = dsl::maintenance_windows
					.select(Self::as_select())
					.filter(dsl::id.eq(id))
					.for_update()
					.first(conn)
					.await
					.map_err(AppError::from)?;
				// A window past its expected end is still open until the sweep
				// stamps it, and amending its end then is the operator saying the
				// work ran long.
				if window.ended_at.is_some() {
					return Err(ended_is_history());
				}
				let Some(from) = window.grain() else {
					return Err(AppError::BadRequest(
						"a maintenance window covers an application, a machine or a group".into(),
					));
				};
				let to = amendment.target.filter(|to| *to != from);
				let expected_end = amendment.expected_end.unwrap_or(window.expected_end);
				let note = amendment
					.note
					.clone()
					.unwrap_or_else(|| window.note.clone());

				if let Some(to) = to {
					if expected_end <= now {
						return Err(AppError::BadRequest(
							"this window is past its expected end; extend it to move it".into(),
						));
					}
					window.check_movable(conn, from, to, now).await?;
					let latest = MaintenanceWindowMove::latest_for_window(conn, id).await?;
					let covered_from = window.covered_from(latest.as_ref());
					use crate::schema::maintenance_window_moves::dsl as moves;
					let (from_application, from_machine, from_group, from_rank) = from.columns();
					diesel::insert_into(moves::maintenance_window_moves)
						.values((
							moves::window_id.eq(id),
							moves::application_id.eq(from_application),
							moves::machine_id.eq(from_machine),
							moves::server_group_id.eq(from_group),
							moves::rank.eq(from_rank),
							moves::covered_from.eq(jiff_diesel::Timestamp::from(covered_from)),
							moves::moved_at.eq(jiff_diesel::Timestamp::from(now)),
							moves::moved_by.eq(by),
						))
						.execute(conn)
						.await
						.map_err(AppError::from)?;
				}

				let (application, machine, group, rank) = to.unwrap_or(from).columns();
				let amended = diesel::update(dsl::maintenance_windows.filter(dsl::id.eq(id)))
					.set((
						dsl::application_id.eq(application),
						dsl::machine_id.eq(machine),
						dsl::server_group_id.eq(group),
						dsl::rank.eq(rank),
						dsl::expected_end.eq(jiff_diesel::Timestamp::from(expected_end)),
						dsl::note.eq(note.as_deref()),
						dsl::amended_by.eq(by),
						dsl::amended_at.eq(jiff_diesel::Timestamp::from(now)),
						dsl::updated_at.eq(jiff_diesel::Timestamp::from(now)),
					))
					.returning(Self::as_select())
					.get_result(conn)
					.await
					.map_err(|err| match err {
						diesel::result::Error::DatabaseError(
							diesel::result::DatabaseErrorKind::UniqueViolation,
							_,
						) => {
							AppError::Conflict("that target already has a window of its own".into())
						}
						err => AppError::from(err),
					})?;
				Ok((amended, to))
			})
			.await?;

		if let Some(to) = to {
			let scope = to.scope();
			let closed_because = match by {
				Some(login) => format!("maintenance declared by {login}"),
				None => "maintenance being declared".to_string(),
			};
			crate::issues::reevaluate_open_issues_for_scope(db, scope, Some(&closed_because))
				.await?;
		}
		Ok(amended)
	}

	/// Refuse a move this window cannot make: off its target's line of descent,
	/// onto a target holding a window of its own, for a window an upgrade plan
	/// holds to its environment, or while a run lease is served against it.
	async fn check_movable(
		&self,
		db: &mut AsyncPgConnection,
		from: Grain,
		to: Grain,
		now: Timestamp,
	) -> Result<()> {
		if self.upgrade_plan_id.is_some() {
			return Err(AppError::Conflict(
				"this window was declared from an upgrade plan and stays over the plan's environment".into(),
			));
		}
		if let Some(lease) = self.serving_lease(db, from, now).await? {
			return Err(AppError::Conflict(format!(
				"a configuration run by {} is under way against this window; it stays where it is until the lease is released",
				lease.held_by.as_deref().unwrap_or("another operator"),
			)));
		}
		if !line_of_descent(db, from).await?.includes(to) {
			return Err(AppError::BadRequest(
				"a window moves to a grain on its target's line of descent".into(),
			));
		}
		let (scope, rank) = (to.scope(), to.rank());
		if Self::open_for(db, scope, rank).await?.is_some() {
			return Err(AppError::Conflict(
				"that target already has a window of its own".into(),
			));
		}
		Ok(())
	}

	/// Why this window cannot move, where it cannot: the plan it was declared
	/// from, or a run lease being served against it.
	pub async fn held_in_place(&self, db: &mut AsyncPgConnection) -> Result<Option<HeldInPlace>> {
		if let Some(plan_id) = self.upgrade_plan_id {
			return Ok(Some(HeldInPlace::UpgradePlan { plan_id }));
		}
		let Some(grain) = self.grain() else {
			return Ok(None);
		};
		Ok(self
			.serving_lease(db, grain, Timestamp::now())
			.await?
			.map(|lease| HeldInPlace::RunLease {
				held_by: lease.held_by,
			}))
	}

	/// A live run lease on an environment this window covers, held by an
	/// operator the window speaks for: the declarer, or the standing amender.
	// spec: INV#work-under-way
	async fn serving_lease(
		&self,
		db: &mut AsyncPgConnection,
		grain: Grain,
		now: Timestamp,
	) -> Result<Option<InventoryLease>> {
		let environments: Vec<(Uuid, ServerRank)> = match (grain.scope(), grain.rank()) {
			(Scope::Group(group_id), Some(rank)) => vec![(group_id, rank)],
			// A group's window covers every environment it has a lease on,
			// whether or not anything in it is still ranked at that rank, so
			// the leases are read directly rather than through its machines.
			(Scope::Group(group_id), None) => InventoryLease::open_for_group(db, group_id)
				.await?
				.into_iter()
				.map(|lease| (group_id, lease.rank))
				.collect(),
			(Scope::Machine(machine_id), _) => {
				let machine = Machine::get_by_id(db, machine_id).await?;
				match (machine.group_id, Machine::rank(db, machine_id).await?) {
					(Some(group), Some(rank)) => vec![(group, rank)],
					_ => Vec::new(),
				}
			}
			(Scope::Application(application_id), _) => {
				let application = Application::get_by_id(db, application_id).await?;
				match (application.group_id, application.rank) {
					(Some(group), Some(rank)) => vec![(group, rank)],
					_ => Vec::new(),
				}
			}
			(Scope::Cluster(_) | Scope::Global, _) => Vec::new(),
		};
		let speaks_for = [self.declared_by.as_deref(), self.amended_by.as_deref()];
		for (group, rank) in environments {
			if let Some(lease) = InventoryLease::open_for(db, group, rank).await?
				&& lease.holds_at(now)
				&& lease.held_by.is_some()
				&& speaks_for.contains(&lease.held_by.as_deref())
			{
				return Ok(Some(lease));
			}
		}
		Ok(None)
	}

	/// Lift a window before its expected end. A window already ended is
	/// returned untouched, so a double lift is idempotent.
	pub async fn lift(db: &mut AsyncPgConnection, id: Uuid, by: Option<&str>) -> Result<Self> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		let lifted: Option<Self> = diesel::update(
			dsl::maintenance_windows
				.filter(dsl::id.eq(id))
				.filter(dsl::ended_at.is_null()),
		)
		.set((
			dsl::ended_at.eq(jiff_diesel::Timestamp::from(now)),
			dsl::ended_by.eq(by),
			dsl::updated_at.eq(jiff_diesel::Timestamp::from(now)),
		))
		.returning(Self::as_select())
		.get_result(db)
		.await
		.optional()
		.map_err(AppError::from)?;
		match lifted {
			Some(window) => {
				window.announce_ended(db).await?;
				Ok(window)
			}
			None => Self::get(db, id).await,
		}
	}

	/// Tell operators the target is being watched again from the end of its
	/// settle period. Whether an operator lifted the window or its expected
	/// end passed is what `ended_by` records.
	async fn announce_ended(&self, db: &mut AsyncPgConnection) -> Result<()> {
		let label = target_label(db, self.scope(), self.rank).await?;
		SlackOutbox::enqueue(
			db,
			KIND_MAINTENANCE_ENDED,
			None,
			None,
			None,
			vars::maintenance_ended(
				&label,
				self.ended_by.as_deref(),
				&format_when(self.suspension_end()),
			),
			Timestamp::now(),
		)
		.await?;
		Ok(())
	}

	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::maintenance_windows::dsl;
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(dsl::id.eq(id))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// The target's window while it holds. A settling window is over as far
	/// as declaring goes: a fresh declaration opens a new one. A group's own
	/// window and each of its environments' are distinct targets.
	pub async fn open_for(
		db: &mut AsyncPgConnection,
		scope: Scope,
		rank: Option<ServerRank>,
	) -> Result<Option<Self>> {
		use crate::schema::maintenance_windows::dsl;
		let Some((application, machine, group)) = fleet_columns(scope) else {
			return Ok(None);
		};
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(
				dsl::ended_at
					.is_null()
					.and(dsl::application_id.is_not_distinct_from(application))
					.and(dsl::machine_id.is_not_distinct_from(machine))
					.and(dsl::server_group_id.is_not_distinct_from(group))
					.and(dsl::rank.is_not_distinct_from(rank)),
			)
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The open windows over an environment: the group's own, the
	/// environment's, and any over the given machines. A window over another of
	/// the group's environments is not over this one. A window past its
	/// expected end stays open until the sweep stamps it, so pair with
	/// [`Self::holds_at`].
	// spec: INV#work-under-way
	pub async fn open_over(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
		machine_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(dsl::ended_at.is_null())
			.filter(
				dsl::server_group_id
					.eq(group_id)
					.and(dsl::rank.is_null().or(dsl::rank.eq(rank)))
					.or(dsl::machine_id.eq_any(machine_ids)),
			)
			.order(dsl::declared_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every window still holding, most recently declared first.
	pub async fn list_open(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(dsl::ended_at.is_null())
			.order(dsl::declared_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The target's windows, open and ended, each over the span it covered
	/// the target: those still covering it first, then the rest by when they
	/// stopped, most recent first. A group's include the windows over its
	/// environments, and a window that moved off the target is listed over the
	/// span it covered there, so a quiet spell is attributable from either end
	/// of a move.
	///
	/// Both sources are read in that same order and to `limit` each, so the
	/// merge keeps exactly the `limit` spans a single ordered read would.
	// spec: MNT#moving-a-window
	pub async fn list_for_scope(
		db: &mut AsyncPgConnection,
		scope: Scope,
		limit: i64,
	) -> Result<Vec<TargetWindow>> {
		use crate::schema::maintenance_window_moves::dsl as moves;
		use crate::schema::maintenance_windows::dsl;
		let Some((application, machine, group)) = fleet_columns(scope) else {
			return Ok(Vec::new());
		};
		let current: Vec<Self> = dsl::maintenance_windows
			.select(Self::as_select())
			.filter(
				dsl::application_id
					.is_not_distinct_from(application)
					.and(dsl::machine_id.is_not_distinct_from(machine))
					.and(dsl::server_group_id.is_not_distinct_from(group)),
			)
			.order((dsl::ended_at.desc().nulls_first(), dsl::declared_at.desc()))
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)?;
		let moved_off: Vec<MaintenanceWindowMove> = moves::maintenance_window_moves
			.select(MaintenanceWindowMove::as_select())
			.filter(
				moves::application_id
					.is_not_distinct_from(application)
					.and(moves::machine_id.is_not_distinct_from(machine))
					.and(moves::server_group_id.is_not_distinct_from(group)),
			)
			.order(moves::moved_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)?;

		let mut spans = spans_over_target(db, current, moved_off).await?;
		// A span with no end is still covering the target.
		spans.sort_by_key(|span| {
			std::cmp::Reverse((span.ended().is_none(), span.ended(), span.covered_from))
		});
		spans.truncate(usize::try_from(limit).unwrap_or(usize::MAX));
		Ok(spans)
	}

	/// Is a check covered by `(application_id, machine_id, group_id)`
	/// suspended? A target is covered by its own window, by the windows over
	/// what contains it, and stays suspended until the last of them has
	/// settled. A group's own checks are the group's window's alone.
	///
	/// `application_id` is set only for an application's own check, and
	/// `machine_id` is the machine a window would have to name to cover the
	/// check: for a machine's own check, itself; for an application's, the
	/// machine it runs on, since taking the box down stops the workload too.
	/// An application's window covers that application and none of its
	/// neighbours on the box, which is the whole reason the grain exists.
	// spec: MNT#declaring
	pub async fn suspends(
		db: &mut AsyncPgConnection,
		application_id: Option<Uuid>,
		machine_id: Option<Uuid>,
		group_id: Option<Uuid>,
	) -> Result<bool> {
		use crate::schema::maintenance_windows::dsl;
		if application_id.is_none() && machine_id.is_none() && group_id.is_none() {
			return Ok(false);
		}
		use crate::schema::maintenance_window_moves::dsl as moves;
		let cutoff = jiff_diesel::Timestamp::from(Timestamp::now() - SETTLE);
		// What a move left uncovered settles as though the window had ended
		// over it, so its move row is read alongside the windows.
		// spec: MNT#moving-a-window
		let ranks: Vec<Option<ServerRank>> = dsl::maintenance_windows
			.select(dsl::rank)
			.filter(
				dsl::application_id
					.eq(application_id)
					.or(dsl::machine_id.eq(machine_id))
					.or(dsl::server_group_id.eq(group_id)),
			)
			.filter(window_suspending(cutoff))
			.union_all(
				moves::maintenance_window_moves
					.select(moves::rank)
					.filter(
						moves::application_id
							.eq(application_id)
							.or(moves::machine_id.eq(machine_id))
							.or(moves::server_group_id.eq(group_id)),
					)
					.filter(move_settling(cutoff)),
			)
			.load(db)
			.await
			.map_err(AppError::from)?;

		if ranks.iter().any(Option::is_none) {
			return Ok(true);
		}
		// Nothing covers the target, so its box's rank cannot matter. Without
		// this, every uncovered call still pays for the rank query below.
		if ranks.is_empty() {
			return Ok(false);
		}
		let Some(machine_id) = machine_id else {
			return Ok(false);
		};
		let Some(rank) = Machine::environment_ranks(db, &[machine_id])
			.await?
			.remove(&machine_id)
		else {
			return Ok(false);
		};
		Ok(ranks.contains(&Some(rank)))
	}

	/// The machines and groups currently suspended, for callers judging many
	/// targets in one pass.
	///
	/// An application is suspended by a window of its own, or by one over the
	/// box it runs on appearing here instead. An environment's window is kept
	/// as the environment's, since it covers nothing of the group itself.
	pub async fn suspended_targets(db: &mut AsyncPgConnection) -> Result<SuspendedTargets> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		let cutoff = jiff_diesel::Timestamp::from(now - SETTLE);
		use crate::schema::maintenance_window_moves::dsl as moves;
		// A move row reads as a window that ended when it moved, so what it
		// left uncovered suspends without holding, at the grain it was over.
		// spec: MNT#moving-a-window
		let rows: Vec<SuspensionRow> = dsl::maintenance_windows
			.select((
				dsl::application_id,
				dsl::machine_id,
				dsl::server_group_id,
				dsl::rank,
				dsl::ended_at,
				dsl::expected_end,
			))
			.filter(window_suspending(cutoff))
			.union_all(
				moves::maintenance_window_moves
					.select((
						moves::application_id,
						moves::machine_id,
						moves::server_group_id,
						moves::rank,
						moves::moved_at.nullable(),
						moves::moved_at,
					))
					.filter(move_settling(cutoff)),
			)
			.load(db)
			.await
			.map_err(AppError::from)?;
		let mut targets = SuspendedTargets::default();
		for (application, machine, group, rank, ended_at, expected_end) in rows {
			let holds = ended_at.is_none() && now < Timestamp::from(expected_end);
			if let Some(id) = application {
				targets.applications.insert(id);
				if holds {
					targets.holding_applications.insert(id);
				}
				continue;
			}
			match (machine, group, rank) {
				(Some(id), _, _) => {
					targets.machines.insert(id);
					if holds {
						targets.holding_machines.insert(id);
					}
				}
				(None, Some(id), None) => {
					targets.groups.insert(id);
					if holds {
						targets.holding_groups.insert(id);
					}
				}
				(None, Some(id), Some(rank)) => {
					targets.environments.insert((id, rank));
					if holds {
						targets.holding_environments.insert((id, rank));
					}
				}
				(None, None, _) => {}
			}
		}
		targets.covered_by_environment = environment_of_machines(db, &targets.environments).await?;
		Ok(targets)
	}

	/// End every window whose expected end has passed, stamping the end at
	/// that expected end rather than at now: the window was over then, and
	/// backdating keeps the settle period honest however late this runs.
	/// Returns what it ended, for notification.
	pub async fn sweep_expired(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		let now = jiff_diesel::Timestamp::from(Timestamp::now());
		let expired: Vec<Self> = diesel::update(
			dsl::maintenance_windows
				.filter(dsl::ended_at.is_null())
				.filter(dsl::expected_end.le(now)),
		)
		.set((
			dsl::ended_at.eq(dsl::expected_end.nullable()),
			dsl::updated_at.eq(now),
		))
		.returning(Self::as_select())
		.get_results(db)
		.await
		.map_err(AppError::from)?;
		for window in &expired {
			window.announce_ended(db).await?;
		}
		Ok(expired)
	}

	/// End windows that have reached their expected end, then re-evaluate
	/// the targets whose settle period has since elapsed, so anything still
	/// degraded contributes again. Returns how many of each.
	///
	/// A target still covered by another window is unaffected by the second
	/// pass: membership re-evaluation consults the windows over it, so the
	/// last one to end is the one that lets its issues back in.
	pub async fn sweep(db: &mut AsyncPgConnection) -> Result<(usize, usize)> {
		let ended = Self::sweep_expired(db).await?;
		let settled = Self::claim_settled(db).await?;
		for window in &settled {
			crate::issues::reevaluate_open_issues_for_scope(db, window.scope(), None).await?;
		}
		// What a move left uncovered is watched again once its settle period
		// has passed, without a notice of its own.
		// spec: MNT#moving-a-window
		let moved = Self::claim_settled_moves(db).await?;
		for moved in &moved {
			if let Some(grain) = moved.grain() {
				crate::issues::reevaluate_open_issues_for_scope(db, grain.scope(), None).await?;
			}
		}
		Ok((ended.len(), settled.len() + moved.len()))
	}

	/// Claim the moves whose settle period has elapsed, so the targets they
	/// left can be re-evaluated once each.
	pub async fn claim_settled_moves(
		db: &mut AsyncPgConnection,
	) -> Result<Vec<MaintenanceWindowMove>> {
		use crate::schema::maintenance_window_moves::dsl;
		let now = Timestamp::now();
		let cutoff = jiff_diesel::Timestamp::from(now - SETTLE);
		diesel::update(
			dsl::maintenance_window_moves
				.filter(dsl::settled_at.is_null())
				.filter(dsl::moved_at.le(cutoff)),
		)
		.set(dsl::settled_at.eq(jiff_diesel::Timestamp::from(now)))
		.returning(MaintenanceWindowMove::as_select())
		.get_results(db)
		.await
		.map_err(AppError::from)
	}

	/// Claim the ended windows whose settle period has elapsed, so their
	/// targets can be re-evaluated once each.
	pub async fn claim_settled(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		let cutoff = jiff_diesel::Timestamp::from(now - SETTLE);
		diesel::update(
			dsl::maintenance_windows
				.filter(dsl::settled_at.is_null())
				.filter(dsl::ended_at.is_not_null())
				.filter(dsl::ended_at.le(cutoff)),
		)
		.set(dsl::settled_at.eq(jiff_diesel::Timestamp::from(now)))
		.returning(Self::as_select())
		.get_results(db)
		.await
		.map_err(AppError::from)
	}
}

/// How a window's target reads in a notification.
pub async fn target_label(
	db: &mut AsyncPgConnection,
	scope: Scope,
	rank: Option<ServerRank>,
) -> Result<String> {
	match Grain::new(scope, rank) {
		Ok(grain) => Ok(target_labels(db, &[grain])
			.await?
			.remove(&grain)
			.unwrap_or_default()),
		Err(_) => match scope {
			// A window never covers a cluster (see `fleet_columns`), so this is
			// unreachable; label it by its grain rather than panicking.
			Scope::Cluster(cid) => Ok(crate::KubernetesCluster::get_by_id(db, cid).await?.name),
			_ => Ok("Canopy".to_string()),
		},
	}
}

/// How each of `grains` reads to an operator, in one read per kind however
/// many there are: a machine and an application under their group's name, an
/// application under its environment's, an environment as its group's name
/// with its rank.
pub async fn target_labels(
	db: &mut AsyncPgConnection,
	grains: &[Grain],
) -> Result<HashMap<Grain, String>> {
	let mut machine_ids = Vec::new();
	let mut application_ids = Vec::new();
	let mut group_ids = Vec::new();
	for grain in grains {
		match grain.scope() {
			Scope::Machine(id) => machine_ids.push(id),
			Scope::Application(id) => application_ids.push(id),
			Scope::Group(id) => group_ids.push(id),
			Scope::Cluster(_) | Scope::Global => {}
		}
	}
	let machines: HashMap<Uuid, Machine> = Machine::get_by_ids(db, &machine_ids)
		.await?
		.into_iter()
		.map(|machine| (machine.id, machine))
		.collect();
	let applications: HashMap<Uuid, Application> = if application_ids.is_empty() {
		HashMap::new()
	} else {
		Application::get_by_ids(db, &application_ids)
			.await?
			.into_iter()
			.map(|application| (application.id, application))
			.collect()
	};
	group_ids.extend(machines.values().filter_map(|machine| machine.group_id));
	group_ids.extend(
		applications
			.values()
			.filter_map(|application| application.group_id),
	);
	group_ids.sort();
	group_ids.dedup();
	let groups = ServerGroup::names_by_ids(db, &group_ids).await?;

	let mut labels = HashMap::with_capacity(grains.len());
	for grain in grains {
		let label = match grain.scope() {
			Scope::Machine(id) => {
				let Some(machine) = machines.get(&id) else {
					continue;
				};
				match machine.group_id.and_then(|gid| groups.get(&gid)) {
					Some(group) => format!("{group} {}", machine.name),
					None => machine.name.clone(),
				}
			}
			Scope::Application(id) => {
				let Some(application) = applications.get(&id) else {
					continue;
				};
				let own = application.display_name();
				match application.group_id.and_then(|gid| groups.get(&gid)) {
					Some(group) => {
						let within = match application.rank {
							Some(rank) => environment_name(group, rank),
							None => group.clone(),
						};
						format!("{within} {own}")
					}
					None => own,
				}
			}
			Scope::Group(id) => {
				let Some(group) = groups.get(&id) else {
					continue;
				};
				match grain.rank() {
					Some(rank) => environment_name(group, rank),
					None => group.clone(),
				}
			}
			Scope::Cluster(_) | Scope::Global => continue,
		};
		labels.insert(*grain, label);
	}
	Ok(labels)
}

fn format_when(at: Timestamp) -> String {
	at.strftime("%Y-%m-%d %H:%M UTC").to_string()
}

/// The spans of `current` (windows over a target now) and `moved_off` (moves
/// off it) over that target: when each started covering it, and for a move,
/// where the window went next.
async fn spans_over_target(
	db: &mut AsyncPgConnection,
	current: Vec<MaintenanceWindow>,
	moved_off: Vec<MaintenanceWindowMove>,
) -> Result<Vec<TargetWindow>> {
	use crate::schema::maintenance_window_moves::dsl as moves;
	use crate::schema::maintenance_windows::dsl;

	let mut window_ids: Vec<Uuid> = current.iter().map(|window| window.id).collect();
	window_ids.extend(moved_off.iter().map(|moved| moved.window_id));
	let all_moves: Vec<MaintenanceWindowMove> = moves::maintenance_window_moves
		.select(MaintenanceWindowMove::as_select())
		.filter(moves::window_id.eq_any(&window_ids))
		.order(moves::moved_at.asc())
		.load(db)
		.await
		.map_err(AppError::from)?;
	let mut windows: HashMap<Uuid, MaintenanceWindow> = current
		.iter()
		.map(|window| (window.id, window.clone()))
		.collect();
	let missing: Vec<Uuid> = moved_off
		.iter()
		.map(|moved| moved.window_id)
		.filter(|id| !windows.contains_key(id))
		.collect();
	if !missing.is_empty() {
		let loaded: Vec<MaintenanceWindow> = dsl::maintenance_windows
			.select(MaintenanceWindow::as_select())
			.filter(dsl::id.eq_any(&missing))
			.load(db)
			.await
			.map_err(AppError::from)?;
		windows.extend(loaded.into_iter().map(|window| (window.id, window)));
	}

	let mut out: Vec<TargetWindow> = Vec::with_capacity(current.len() + moved_off.len());
	for window in current {
		let covered_from =
			window.covered_from(all_moves.iter().rfind(|moved| moved.window_id == window.id));
		let rank = window.rank;
		out.push(TargetWindow {
			window,
			covered_from,
			moved_at: None,
			moved_to: None,
			covered_rank: rank,
		});
	}
	// Where each moved-off span went next: the target the following move
	// left, or the window's own where that was its last move.
	let mut next: Vec<(MaintenanceWindowMove, MaintenanceWindow, Option<Grain>)> = Vec::new();
	for moved in moved_off {
		let Some(window) = windows.get(&moved.window_id).cloned() else {
			continue;
		};
		let to = all_moves
			.iter()
			.find(|later| later.window_id == moved.window_id && later.moved_at > moved.moved_at)
			.and_then(MaintenanceWindowMove::grain)
			.or_else(|| window.grain());
		next.push((moved, window, to));
	}
	let destinations: Vec<Grain> = next.iter().filter_map(|(_, _, to)| *to).collect();
	let labels = target_labels(db, &destinations).await?;
	for (moved, window, to) in next {
		out.push(TargetWindow {
			window,
			covered_from: moved.covered_from,
			moved_at: Some(moved.moved_at),
			moved_to: to.and_then(|to| labels.get(&to).cloned()),
			covered_rank: moved.rank,
		});
	}
	Ok(out)
}

fn ended_is_history() -> AppError {
	AppError::BadRequest(
		"a window that has ended is history; suspending again is a fresh declaration".into(),
	)
}

type WindowSuspending = diesel::dsl::Or<
	diesel::dsl::And<
		diesel::dsl::IsNull<crate::schema::maintenance_windows::ended_at>,
		diesel::dsl::Gt<crate::schema::maintenance_windows::expected_end, jiff_diesel::Timestamp>,
	>,
	diesel::dsl::Gt<crate::schema::maintenance_windows::ended_at, jiff_diesel::Timestamp>,
>;

/// Windows whose suspension runs past `cutoff` (now less the settle period):
/// still open with an end after it, or ended after it.
fn window_suspending(cutoff: jiff_diesel::Timestamp) -> WindowSuspending {
	use crate::schema::maintenance_windows::dsl;
	dsl::ended_at
		.is_null()
		.and(dsl::expected_end.gt(cutoff))
		.or(dsl::ended_at.gt(cutoff))
}

type MoveSettling = diesel::dsl::And<
	diesel::dsl::IsNull<crate::schema::maintenance_window_moves::settled_at>,
	diesel::dsl::Gt<crate::schema::maintenance_window_moves::moved_at, jiff_diesel::Timestamp>,
>;

/// Moves whose settle period runs past `cutoff`. A move is only stamped
/// settled once that period has passed, so the `settled_at` test changes
/// nothing about which rows match; it is there so the unsettled index serves
/// the read instead of a scan of all history.
fn move_settling(cutoff: jiff_diesel::Timestamp) -> MoveSettling {
	use crate::schema::maintenance_window_moves::dsl;
	dsl::settled_at.is_null().and(dsl::moved_at.gt(cutoff))
}

/// The `(application, machine, group)` storage columns for a window's scope.
/// Canopy-wide is not a target a window covers: Canopy's own checks are its
/// self-monitoring, and fleet work never suspends them.
fn fleet_columns(scope: Scope) -> Option<(Option<Uuid>, Option<Uuid>, Option<Uuid>)> {
	match scope {
		Scope::Application(id) => Some((Some(id), None, None)),
		Scope::Machine(id) => Some((None, Some(id), None)),
		Scope::Group(id) => Some((None, None, Some(id))),
		// A cluster is not a target a window covers, any more than canopy-wide
		// is: fleet work is declared over an application, a machine, or a group.
		Scope::Cluster(_) | Scope::Global => None,
	}
}

/// The environment each machine serves, for the machines in the groups these
/// environments belong to, by [`Machine::environment_rank`]'s rule, so this and
/// [`MaintenanceWindow::suspends`] read a box the same way. A pending or
/// archived machine serves none.
// spec: MNT#declaring
async fn environment_of_machines(
	db: &mut AsyncPgConnection,
	environments: &HashSet<(Uuid, ServerRank)>,
) -> Result<HashMap<Uuid, (Uuid, ServerRank)>> {
	use crate::schema::machines::dsl;

	if environments.is_empty() {
		return Ok(HashMap::new());
	}
	let group_ids: Vec<Uuid> = environments.iter().map(|(group, _)| *group).collect();
	let serving: Vec<(Uuid, Uuid, ServerRank)> = dsl::machines
		.select((
			dsl::id,
			dsl::group_id.assume_not_null(),
			dsl::rank.assume_not_null(),
		))
		.filter(dsl::group_id.eq_any(&group_ids))
		.filter(dsl::rank.is_not_null())
		.filter(dsl::deleted_at.is_null())
		.load(db)
		.await
		.map_err(AppError::from)?;
	Ok(serving
		.into_iter()
		.map(|(machine, group, rank)| (machine, (group, rank)))
		.collect())
}
