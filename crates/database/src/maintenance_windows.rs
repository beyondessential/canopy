//! Operator declarations that a machine, a group, or one of a group's
//! environments is being worked on.
//!
//! While a window suspends a target its checks are observed, graded, and
//! presented exactly as they would be without it. What the window holds back
//! is what those results feed: no issue on the target opens or joins an
//! incident, so nothing notifies, and an operator working through a window
//! watches the check they are fixing come good.
//!
//! A window is over the machine rather than over one workload on it. Taking a
//! box down to patch it stops everything running on it, so a window naming one
//! application would leave the others alerting through work that was always
//! going to stop them. One declaration, N consequences.
//!
//! Suspension outlasts the window itself by [`SETTLE`]. A machine is back
//! before the sources on it have reported again, and a machine whose every
//! source is stale is unreachable, so ending suspension the instant the work
//! finishes would page for a server that has just come back, for as long as
//! the work took.

use std::collections::HashSet;

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::issues::Scope;
use crate::machines::Machine;
use crate::server_groups::{ServerGroup, environment_name, rank_priority};
use crate::slack_outbox::{KIND_MAINTENANCE_DECLARED, KIND_MAINTENANCE_ENDED, SlackOutbox, vars};

/// How long suspension outlasts the window, giving the reporters on a
/// server time to be heard from before Canopy pages for them. The same for
/// every window.
pub const SETTLE: SignedDuration = SignedDuration::from_mins(10);

/// The targets a window covers right now, and which of them it still holds
/// rather than settling over.
#[derive(Clone, Debug, Default)]
pub struct SuspendedTargets {
	pub machines: HashSet<Uuid>,
	pub groups: HashSet<Uuid>,
	pub holding_machines: HashSet<Uuid>,
	pub holding_groups: HashSet<Uuid>,
}

/// A window's targets with the two timestamps that say whether it still holds.
type SuspensionRow = (
	Option<Uuid>,
	Option<Uuid>,
	Option<ServerRank>,
	Option<jiff_diesel::Timestamp>,
	jiff_diesel::Timestamp,
);

impl SuspendedTargets {
	/// Is this box suspended, by its own window or its group's?
	pub fn suspends(&self, machine: Uuid, group: Option<Uuid>) -> bool {
		self.machines.contains(&machine) || group.is_some_and(|g| self.groups.contains(&g))
	}

	/// Is every window over this box ended, leaving it in the settle period?
	pub fn settling(&self, machine: Uuid, group: Option<Uuid>) -> bool {
		self.suspends(machine, group)
			&& !self.holding_machines.contains(&machine)
			&& !group.is_some_and(|g| self.holding_groups.contains(&g))
	}
}

/// A declaration that a machine, a group, or one of a group's environments is
/// being worked on.
#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::maintenance_windows)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MaintenanceWindow {
	/// Unique identifier of this window.
	pub id: Uuid,
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
}

impl MaintenanceWindow {
	/// The target this window covers.
	pub fn scope(&self) -> Scope {
		Scope::from_columns(None, self.machine_id, self.server_group_id)
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
		use crate::schema::maintenance_windows::dsl;
		let Some((server, group)) = fleet_columns(scope) else {
			return Err(AppError::BadRequest(
				"a maintenance window covers a machine or a group".into(),
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
				dsl::machine_id.eq(server),
				dsl::server_group_id.eq(group),
				dsl::rank.eq(rank),
				dsl::expected_end.eq(jiff_diesel::Timestamp::from(expected_end)),
				dsl::note.eq(note),
				dsl::declared_by.eq(by),
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
		let Some((server, group)) = fleet_columns(scope) else {
			return Ok(None);
		};
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(
				dsl::ended_at
					.is_null()
					.and(dsl::machine_id.is_not_distinct_from(server))
					.and(dsl::server_group_id.is_not_distinct_from(group))
					.and(dsl::rank.is_not_distinct_from(rank)),
			)
			.first(db)
			.await
			.optional()
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

	/// The target's windows, open and ended, most recently declared first. A
	/// group's include the windows over its environments.
	pub async fn list_for_scope(
		db: &mut AsyncPgConnection,
		scope: Scope,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::maintenance_windows::dsl;
		let Some((server, group)) = fleet_columns(scope) else {
			return Ok(Vec::new());
		};
		dsl::maintenance_windows
			.select(Self::as_select())
			.filter(
				dsl::machine_id
					.is_not_distinct_from(server)
					.and(dsl::server_group_id.is_not_distinct_from(group)),
			)
			.order(dsl::declared_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Is a check covered by `(machine_id, group_id)` suspended? A target is
	/// covered by its machine's window, by its group's, and by the window over
	/// the environment the machine serves, and stays suspended until the last
	/// of them has settled. A group's own checks are the group's window's
	/// alone.
	///
	/// `machine_id` is the machine a window would have to name to cover this
	/// check: for a machine's own check, itself; for an application's, the
	/// machine it runs on, since taking the box down stops the workload too.
	pub async fn suspends(
		db: &mut AsyncPgConnection,
		machine_id: Option<Uuid>,
		group_id: Option<Uuid>,
	) -> Result<bool> {
		use crate::schema::maintenance_windows::dsl;
		if machine_id.is_none() && group_id.is_none() {
			return Ok(false);
		}
		let cutoff = jiff_diesel::Timestamp::from(Timestamp::now() - SETTLE);
		let ranks: Vec<Option<ServerRank>> = dsl::maintenance_windows
			.select(dsl::rank)
			.filter(
				dsl::machine_id
					.eq(machine_id)
					.or(dsl::server_group_id.eq(group_id)),
			)
			.filter(
				dsl::ended_at
					.is_null()
					.and(dsl::expected_end.gt(cutoff))
					.or(dsl::ended_at.gt(cutoff)),
			)
			.load(db)
			.await
			.map_err(AppError::from)?;

		if ranks.iter().any(Option::is_none) {
			return Ok(true);
		}
		let Some(machine_id) = machine_id else {
			return Ok(false);
		};
		let Some(rank) = Machine::rank(db, machine_id).await? else {
			return Ok(false);
		};
		Ok(ranks.contains(&Some(rank)))
	}

	/// The machines and groups currently suspended, for callers judging many
	/// targets in one pass.
	///
	/// A window is declared over a machine, so an application is suspended by
	/// its machine's id appearing here rather than its own. An environment's
	/// window counts as one over each machine serving it, since it covers
	/// nothing of the group itself.
	pub async fn suspended_targets(db: &mut AsyncPgConnection) -> Result<SuspendedTargets> {
		use crate::schema::maintenance_windows::dsl;
		let now = Timestamp::now();
		let cutoff = jiff_diesel::Timestamp::from(now - SETTLE);
		let rows: Vec<SuspensionRow> = dsl::maintenance_windows
			.select((
				dsl::machine_id,
				dsl::server_group_id,
				dsl::rank,
				dsl::ended_at,
				dsl::expected_end,
			))
			.filter(
				dsl::ended_at
					.is_null()
					.and(dsl::expected_end.gt(cutoff))
					.or(dsl::ended_at.gt(cutoff)),
			)
			.load(db)
			.await
			.map_err(AppError::from)?;
		let mut targets = SuspendedTargets::default();
		let mut environments = HashSet::new();
		let mut holding_environments = HashSet::new();
		for (machine, group, rank, ended_at, expected_end) in rows {
			let holds = ended_at.is_none() && now < Timestamp::from(expected_end);
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
					environments.insert((id, rank));
					if holds {
						holding_environments.insert((id, rank));
					}
				}
				(None, None, _) => {}
			}
		}
		targets
			.machines
			.extend(machines_in_environments(db, &environments).await?);
		targets
			.holding_machines
			.extend(machines_in_environments(db, &holding_environments).await?);
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
		Ok((ended.len(), settled.len()))
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
	match scope {
		Scope::Machine(mid) => {
			let machine = Machine::get_by_id(db, mid).await?;
			let own = machine
				.name
				.clone()
				.unwrap_or_else(|| machine.id.to_string());
			match machine.group_id {
				Some(gid) => {
					let group = ServerGroup::get_by_id(db, gid).await?;
					Ok(format!("{} {own}", group.name))
				}
				None => Ok(own),
			}
		}
		// A window is never declared over one application, so this is only
		// reachable if a caller hands in a scope from elsewhere.
		Scope::Application(aid) => Ok(aid.to_string()),
		Scope::Group(gid) => {
			let group = ServerGroup::get_by_id(db, gid).await?;
			Ok(match rank {
				Some(rank) => environment_name(&group.name, rank),
				None => group.name,
			})
		}
		Scope::Global => Ok("Canopy".to_string()),
	}
}

fn format_when(at: Timestamp) -> String {
	at.strftime("%Y-%m-%d %H:%M UTC").to_string()
}

/// The storage columns for a window's scope. Canopy-wide is not a target a
/// window covers: Canopy's own checks are its self-monitoring, and fleet
/// work never suspends them.
fn fleet_columns(scope: Scope) -> Option<(Option<Uuid>, Option<Uuid>)> {
	match scope {
		Scope::Machine(id) => Some((Some(id), None)),
		Scope::Group(id) => Some((None, Some(id))),
		// A window covers a machine or a group. An application is covered
		// through its machine rather than named, and Canopy's own checks are
		// its self-monitoring, which fleet work never suspends.
		Scope::Application(_) | Scope::Global => None,
	}
}

/// The machines serving any of these environments, each machine taking the
/// rank of the highest-ranked application on it.
// spec: MNT#declaring
async fn machines_in_environments(
	db: &mut AsyncPgConnection,
	environments: &HashSet<(Uuid, ServerRank)>,
) -> Result<HashSet<Uuid>> {
	use crate::schema::applications::dsl;
	use std::collections::HashMap;

	if environments.is_empty() {
		return Ok(HashSet::new());
	}
	let group_ids: Vec<Uuid> = environments.iter().map(|(group, _)| *group).collect();
	let members: Vec<(Uuid, Option<Uuid>, Option<ServerRank>)> = dsl::applications
		.select((dsl::machine_id, dsl::group_id, dsl::rank))
		.filter(dsl::group_id.eq_any(&group_ids))
		.filter(dsl::deleted_at.is_null())
		.load(db)
		.await
		.map_err(AppError::from)?;

	let mut serving: HashMap<Uuid, (Uuid, ServerRank)> = HashMap::new();
	for (machine, group, rank) in members {
		let (Some(group), Some(rank)) = (group, rank) else {
			continue;
		};
		serving
			.entry(machine)
			.and_modify(|held| {
				if rank_priority(Some(rank)) < rank_priority(Some(held.1)) {
					*held = (group, rank);
				}
			})
			.or_insert((group, rank));
	}
	Ok(serving
		.into_iter()
		.filter(|(_, pair)| environments.contains(pair))
		.map(|(machine, _)| machine)
		.collect())
}
