use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use commons_types::{Uuid, server::rank::ServerRank};
use database::audit::{Action, Record, RecordKind};
use database::diesel_async::AsyncPgConnection;
use database::issues::{Incident, Scope};
use database::machines::Machine;
use database::maintenance_windows::{
	Amendment, Cover, Grain, HeldInPlace, MaintenanceWindow, TargetWindow,
	environment_of_machine_set, line_of_descent_for, window_labels,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use std::collections::HashMap;

use crate::state::AppState;

/// How many of a target's ended windows its history returns.
const HISTORY_LIMIT: i64 = 20;

/// A window and what it covers, as the records an entry names: the one target,
/// or each machine of a window over several. The window is named by its target
/// as it reads to an operator, since a window has no name of its own.
pub async fn window_records(
	conn: &mut AsyncPgConnection,
	window: &MaintenanceWindow,
) -> Result<(Record, Vec<Record>)> {
	let label = database::maintenance_windows::window_label(conn, window).await?;
	let targets = if window.is_machine_set() {
		let mut records = Vec::with_capacity(window.machine_ids.len());
		for machine in &window.machine_ids {
			records.push(Record::machine(conn, *machine).await?);
		}
		records
	} else {
		vec![
			Record::scope(conn, window.scope())
				.await?
				.ok_or_else(|| AppError::custom("a maintenance window covers a target"))?,
		]
	};
	Ok((
		Record::new(RecordKind::MaintenanceWindow, window.id, label),
		targets,
	))
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: list_open))
		.routes(routes!(read_only, silent: for_target))
		.routes(routes!(read_only, silent: targets))
		.routes(routes!(write, audit(MaintenanceDeclared, MaintenanceAmended): declare))
		.routes(routes!(write, audit(MaintenanceAmended): amend))
		.routes(routes!(write, audit(MaintenanceAmended): hold))
		.routes(routes!(write, audit(MaintenanceLifted): lift))
}

/// A target a window can cover, named the way a declaration names it: exactly
/// one of the ids, and with the group, optionally the rank of one of its
/// environments; or several machines of one environment, named alone.
#[derive(Clone, Serialize, Deserialize, ToSchema)]
pub struct MaintenanceTarget {
	/// The application, for a window over one workload.
	pub application_id: Option<Uuid>,
	/// The machine, for a window over one box.
	pub machine_id: Option<Uuid>,
	/// The group, for a window over a whole group or one of its environments.
	pub server_group_id: Option<Uuid>,
	/// With the group, the environment the window covers. Absent for the whole
	/// group.
	pub rank: Option<ServerRank>,
	/// Several machines of one environment, for a window over each of them. Named
	/// alone, without the other fields: the group and environment are the
	/// machines'. One machine is that machine's window. Offered as a choice with
	/// none or the window's own machines until the operator picks.
	#[serde(default, skip_serializing_if = "Option::is_none")]
	pub machine_ids: Option<Vec<Uuid>>,
}

impl MaintenanceTarget {
	/// The application, machine, or group named, whatever the rank.
	fn scope(&self) -> Result<Scope> {
		if self.machine_ids.is_some() {
			return Err(AppError::BadRequest(
				"a window over several machines has no one target".into(),
			));
		}
		match (self.application_id, self.machine_id, self.server_group_id) {
			(Some(id), None, None) => Ok(Scope::Application(id)),
			(None, Some(id), None) => Ok(Scope::Machine(id)),
			(None, None, Some(id)) => Ok(Scope::Group(id)),
			_ => Err(AppError::BadRequest(
				"a maintenance window covers one application, one machine or one group".into(),
			)),
		}
	}

	fn grain(&self) -> Result<Grain> {
		Grain::new(self.scope()?, self.rank)
	}

	/// What this names a window over: a grain, or several machines.
	fn cover(&self) -> Result<Cover> {
		match &self.machine_ids {
			Some(ids) => {
				if self.application_id.is_some()
					|| self.machine_id.is_some()
					|| self.server_group_id.is_some()
					|| self.rank.is_some()
				{
					return Err(AppError::BadRequest(
						"several machines are named alone: their group and environment are theirs"
							.into(),
					));
				}
				Cover::machines(ids.iter().copied())
			}
			None => Ok(Cover::Grain(self.grain()?)),
		}
	}
}

impl From<Grain> for MaintenanceTarget {
	fn from(grain: Grain) -> Self {
		let (application_id, machine_id, server_group_id, rank) = grain.columns();
		Self {
			application_id,
			machine_id,
			server_group_id,
			rank,
			machine_ids: None,
		}
	}
}

impl From<Cover> for MaintenanceTarget {
	fn from(cover: Cover) -> Self {
		match cover {
			Cover::Grain(grain) => grain.into(),
			Cover::Machines(ids) => Self {
				application_id: None,
				machine_id: None,
				server_group_id: None,
				rank: None,
				machine_ids: Some(ids),
			},
		}
	}
}

/// Declare a window over a target, or amend the one it already has.
#[derive(Deserialize, ToSchema)]
pub struct DeclareArgs {
	/// What the window covers.
	#[serde(flatten)]
	pub target: MaintenanceTarget,
	/// When the work is expected to finish. The window ends itself then.
	#[schema(value_type = String, format = DateTime)]
	pub expected_end: Timestamp,
	/// What is being done.
	pub note: Option<String>,
	/// The upgrade plan this is declared from, over the plan's environment. A
	/// window it opens stays over that environment for as long as it holds.
	pub upgrade_plan_id: Option<Uuid>,
}

/// What to change about an open window. A field left out keeps the window's
/// own, so amending someone else's window changes only what was chosen.
#[derive(Deserialize, ToSchema)]
pub struct AmendWindowArgs {
	/// The window to amend.
	pub id: Uuid,
	/// A new target, moving the window there: any grain on its target's line
	/// of descent that has no window of its own, or machines of its
	/// environment that no other window names. Naming machines adds and removes
	/// the difference from those the window covers now.
	pub target: Option<MaintenanceTarget>,
	/// A new expected end.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub expected_end: Option<Timestamp>,
	/// A new note. Null clears it; leaving it out keeps the window's own.
	#[serde(default, deserialize_with = "super::applications::deserialize_some")]
	#[schema(value_type = Option<String>, nullable)]
	pub note: Option<Option<String>>,
}

/// Where a declaration starts, and what it is read against.
#[derive(Deserialize, ToSchema)]
pub struct MaintenanceTargetsArgs {
	/// The target the declaration is offered over.
	pub start: MaintenanceTarget,
	/// The incident it is offered from, to mark the choices that leave some of
	/// its failing checks contributing.
	pub incident_id: Option<Uuid>,
	/// The window being amended, where the declaration is an amendment.
	pub window_id: Option<Uuid>,
}

/// One machine of an environment, offered as one of several a window covers.
#[derive(Serialize, ToSchema)]
pub struct MaintenanceMachineChoice {
	/// The machine.
	pub machine_id: Uuid,
	/// The machine's name.
	pub name: String,
	/// The open window naming this machine, where one does: it cannot be chosen
	/// into another window, unless that window is the one being amended.
	pub window: Option<MaintenanceWindow>,
	/// Offered from an incident: whether any of its failing checks are on this
	/// machine or on an application running on it, so a choice of machines can
	/// tell whether it covers them.
	pub has_failures: Option<bool>,
}

/// One grain a declaration can cover.
#[derive(Serialize, ToSchema)]
pub struct MaintenanceTargetChoice {
	/// The target a window here would cover.
	pub target: MaintenanceTarget,
	/// The grain's own name: a group's, a machine's, an application's, or an
	/// environment's rank.
	pub label: String,
	/// How many of the listed grains contain this one.
	pub depth: u8,
	/// The grain's own open window, where it has one.
	pub window: Option<MaintenanceWindow>,
	/// Offered from an incident: whether a window here would cover every one
	/// of its failing checks. Absent for a choice of several machines, which
	/// covers what the machines picked do (see `has_failures`).
	pub covers_failures: Option<bool>,
	/// Set for the choice of several machines of the environment, listed between
	/// the environment and its machines: the machines to pick from, each with
	/// the window that names it.
	pub machines: Option<Vec<MaintenanceMachineChoice>>,
}

/// The grains a declaration can cover, nested in the order they contain one
/// another.
#[derive(Serialize, ToSchema)]
pub struct MaintenanceTargets {
	/// Whatever contains the starting grain and whatever it contains, nested
	/// in the order they contain one another.
	pub choices: Vec<MaintenanceTargetChoice>,
	/// The window the declaration amends from the start: the one named, or
	/// else the starting target's own.
	pub amends: Option<MaintenanceWindow>,
	/// Why that window cannot move, where it cannot.
	pub held_in_place: Option<HeldInPlace>,
}

/// The window to lift.
#[derive(Deserialize, ToSchema)]
pub struct LiftArgs {
	/// The window, as returned when it was declared or listed.
	pub id: Uuid,
}

/// The window a run declared, and the end to give it.
#[derive(Deserialize, ToSchema)]
pub struct HoldArgs {
	/// The window, as returned when it was listed.
	pub id: Uuid,
	/// When the work is expected to finish. The window ends itself then, whatever
	/// becomes of the run's lease.
	#[schema(value_type = String, format = DateTime)]
	pub expected_end: Timestamp,
}

/// An open window with the target it covers, named so a fleet-wide view
/// reads without a lookup per row.
#[derive(Serialize, ToSchema)]
pub struct OpenWindow {
	/// The window itself.
	pub window: MaintenanceWindow,
	/// The target as it reads to an operator.
	pub target: String,
}

/// Every maintenance window currently holding, across the fleet.
///
/// Most recently declared first. A window that has ended is history and
/// belongs to its target, so it is not listed here even while its settle
/// period runs.
#[utoipa::path(
	post,
	path = "/list_open",
	tag = "maintenance",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Vec<OpenWindow>),
	),
)]
pub async fn list_open(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(_args): Json<serde_json::Value>,
) -> Result<Json<Vec<OpenWindow>>> {
	let mut conn = state.db.get().await?;
	let windows = MaintenanceWindow::list_open(&mut conn).await?;
	let mut labels = window_labels(&mut conn, &windows).await?;
	let out = windows
		.into_iter()
		.map(|window| {
			let target = labels.remove(&window.id).unwrap_or_default();
			OpenWindow { window, target }
		})
		.collect();
	Ok(Json(out))
}

/// A target's maintenance windows.
///
/// Those still covering it first, then the rest by when they stopped, so what
/// was being done the last time the target went quiet is readable against it.
/// A group's include the windows over its environments, so the history is read
/// per application, machine, or group, and a rank is refused.
#[utoipa::path(
	post,
	path = "/for_target",
	tag = "maintenance",
	security(("tailscale-user" = [])),
	request_body = MaintenanceTarget,
	responses(
		(status = 200, body = Vec<TargetWindow>),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn for_target(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<MaintenanceTarget>,
) -> Result<Json<Vec<TargetWindow>>> {
	if args.rank.is_some() {
		return Err(AppError::BadRequest(
			"an environment's windows are read through its group's history".into(),
		));
	}
	let mut conn = state.db.get().await?;
	let rows = MaintenanceWindow::list_for_scope(&mut conn, args.scope()?, HISTORY_LIMIT).await?;
	Ok(Json(rows))
}

/// Declare that an application, one or several machines, a group, or one of a
/// group's environments is being worked on.
///
/// Every check on the target grades to skipped while the window holds and
/// for a settle period after it ends, so nothing on it opens or joins an
/// incident. A window over one application leaves the rest of the box watched;
/// one over the machine covers everything on it. Issues already in an open
/// incident leave it, closing the incident where nothing else holds it open. A
/// target that already has an open window has that window amended rather than
/// a second opened. Several machines of one environment are declared together
/// as one window covering each of them; a machine another window names cannot
/// be named in a second.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/declare",
	tag = "maintenance",
	security(("tailscale-admin" = [])),
	request_body = DeclareArgs,
	responses(
		(status = 200, body = MaintenanceWindow),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn declare(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<DeclareArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = audit.begin().await?;
	let window = match args.target.cover()? {
		Cover::Machines(machines) => {
			if args.upgrade_plan_id.is_some() {
				return Err(AppError::BadRequest(
					"a window declared from an upgrade plan is over the plan's environment".into(),
				));
			}
			MaintenanceWindow::declare_machines(
				&mut conn,
				&machines,
				args.expected_end,
				args.note.as_deref(),
				Some(&admin.0.login),
			)
			.await?
		}
		Cover::Grain(grain) => {
			let (scope, rank) = (grain.scope(), grain.rank());
			match args.upgrade_plan_id {
				Some(plan) => {
					MaintenanceWindow::declare_from_plan(
						&mut conn,
						plan,
						scope,
						rank,
						args.expected_end,
						args.note.as_deref(),
						Some(&admin.0.login),
					)
					.await?
				}
				None => {
					MaintenanceWindow::declare(
						&mut conn,
						scope,
						rank,
						args.expected_end,
						args.note.as_deref(),
						Some(&admin.0.login),
					)
					.await?
				}
			}
		}
	};
	let (record, target) = window_records(&mut conn, &window).await?;
	// Declaring over a target that has a window amends it rather than opening a
	// second, and only an amended window carries an amendment.
	let action = if window.amended_at.is_some() {
		Action::MaintenanceAmended {
			window: record,
			target,
			moved_from: Vec::new(),
			rank: window.rank,
			expected_end: window.expected_end,
			note: window.note.clone(),
		}
	} else {
		let upgrade_plan = match args.upgrade_plan_id {
			Some(plan) => {
				let plan = database::upgrade_plans::UpgradePlan::get_open(&mut conn, plan).await?;
				Some(Record::upgrade_plan(&mut conn, &plan).await?)
			}
			None => None,
		};
		Action::MaintenanceDeclared {
			window: record,
			target,
			upgrade_plan,
			lease: None,
			rank: window.rank,
			expected_end: window.expected_end,
			note: window.note.clone(),
		}
	};
	conn.record(action).await?;
	Ok(Json(window))
}

/// Amend an open window: its end, its note, or what it covers.
///
/// Only what the request names changes. A new target moves the window there:
/// it stays the same window, the target it left settles as though the window
/// had ended over it, and what it newly covers is suspended from now. A window
/// over several machines is moved by naming the machines it should cover:
/// those it stops covering settle as though the window had ended over them. A
/// window declared from an upgrade plan, or one a configuration run's lease is
/// being served against, cannot move.
/// Requires admin access.
// spec: MNT#moving-a-window
#[utoipa::path(
	post,
	path = "/amend",
	tag = "maintenance",
	security(("tailscale-admin" = [])),
	request_body = AmendWindowArgs,
	responses(
		(status = 200, body = MaintenanceWindow),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "The target has a window of its own, or the window is held where it is by an upgrade plan or a configuration run", body = ProblemDetailsSchema),
	),
)]
pub async fn amend(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<AmendWindowArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = audit.begin().await?;
	let before = MaintenanceWindow::get(&mut conn, args.id).await?;
	let (target, machines) = match args
		.target
		.as_ref()
		.map(MaintenanceTarget::cover)
		.transpose()?
	{
		Some(Cover::Grain(grain)) => (Some(grain), None),
		Some(Cover::Machines(machines)) => (None, Some(machines)),
		None => (None, None),
	};
	let window = MaintenanceWindow::amend(
		&mut conn,
		args.id,
		Amendment {
			target,
			machines,
			expected_end: args.expected_end,
			note: args
				.note
				.map(|note| note.filter(|note| !note.trim().is_empty())),
		},
		Some(&admin.0.login),
	)
	.await?;
	let (record, target) = window_records(&mut conn, &window).await?;
	let moved_from = if window.cover() != before.cover() {
		window_records(&mut conn, &before).await?.1
	} else {
		Vec::new()
	};
	conn.record(Action::MaintenanceAmended {
		window: record,
		target,
		moved_from,
		rank: window.rank,
		expected_end: window.expected_end,
		note: window.note.clone(),
	})
	.await?;
	Ok(Json(window))
}

/// Hold the window a run declared, giving it an expected end.
///
/// A window a run declared lasts the run and ends when the run's lease does.
/// Holding it gives it an expected end instead, and it then ends as any window
/// does, whatever becomes of the lease. Only the operator whose run it is can
/// hold it. Recorded as an amendment.
/// Requires admin access.
// spec: MNT#ending
#[utoipa::path(
	post,
	path = "/hold",
	tag = "maintenance",
	security(("tailscale-admin" = [])),
	request_body = HoldArgs,
	responses(
		(status = 200, body = MaintenanceWindow),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "The window is not the caller's, so they cannot hold it", body = ProblemDetailsSchema),
	),
)]
pub async fn hold(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<HoldArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = audit.begin().await?;
	let window =
		MaintenanceWindow::hold(&mut conn, args.id, args.expected_end, Some(&admin.0.login))
			.await?;
	let (record, target) = window_records(&mut conn, &window).await?;
	conn.record(Action::MaintenanceAmended {
		window: record,
		target,
		moved_from: Vec::new(),
		rank: window.rank,
		expected_end: window.expected_end,
		note: window.note.clone(),
	})
	.await?;
	Ok(Json(window))
}

/// The grains a declaration offered over `start` can cover.
///
/// Whatever contains the starting grain and whatever it contains, nested
/// group over environment over machine over application, each with its own
/// open window. Offered from an incident, each choice says whether a window
/// there would cover every failing check in it.
// spec: MNT#choosing-what-to-cover
#[utoipa::path(
	post,
	path = "/targets",
	tag = "maintenance",
	security(("tailscale-user" = [])),
	request_body = MaintenanceTargetsArgs,
	responses(
		(status = 200, body = MaintenanceTargets),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn targets(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<MaintenanceTargetsArgs>,
) -> Result<Json<MaintenanceTargets>> {
	let mut conn = state.db.get().await?;
	let start = args.start.cover()?;
	let descent = line_of_descent_for(&mut conn, &start).await?;
	let open_windows = MaintenanceWindow::list_open(&mut conn).await?;
	// A machine's window is the one naming it, whether it names it alone or
	// among others.
	let mut open: HashMap<Grain, MaintenanceWindow> = HashMap::new();
	for window in &open_windows {
		match window.grain() {
			Some(grain) => {
				open.insert(grain, window.clone());
			}
			None => {
				for machine in &window.machine_ids {
					open.insert(Grain::machine(*machine), window.clone());
				}
			}
		}
	}

	// Coverage is reckoned against the failures alone: an incident whose
	// failures have all left closes whatever warnings remain in it.
	// A failure on something no window can be declared over, such as a
	// cluster, is left contributing by every choice, so it is kept as `None`
	// rather than dropped.
	let failing: Option<Vec<Option<Grain>>> = match args.incident_id {
		Some(incident) => {
			let (_, rows) = Incident::get_with_issues(&mut conn, incident).await?;
			Some(
				rows.into_iter()
					.filter(|(link, issue)| link.left_at.is_none() && issue.opens_incident())
					.map(|(_, issue)| Grain::of_issue(&issue))
					.collect(),
			)
		}
		None => None,
	};

	let mut choices: Vec<MaintenanceTargetChoice> = descent
		.entries
		.iter()
		.map(|entry| MaintenanceTargetChoice {
			target: entry.grain.into(),
			label: entry.label.clone(),
			depth: entry.depth,
			window: open.get(&entry.grain).cloned(),
			covers_failures: failing.as_ref().map(|failing| {
				failing
					.iter()
					.all(|target| target.is_some_and(|target| descent.covers(entry.grain, target)))
			}),
			machines: None,
		})
		.collect();

	// The window the declaration amends: the one named, or else the starting
	// target's own, since a declaration offered over a target with a window of
	// its own amends it from the start. A machine's own is the window naming it.
	// spec: MNT#moving-a-window
	// A window that has ended is amended by no one, so nothing holds it.
	let own = match args.window_id {
		Some(id) => {
			let window = MaintenanceWindow::get(&mut conn, id).await?;
			// The declaration amends the window it starts over, and no other.
			if !window.is_window_of(&start) {
				return Err(AppError::BadRequest(
					"the window named is not the starting target's".into(),
				));
			}
			Some(window).filter(|window| window.ended_at.is_none())
		}
		None => open_windows
			.iter()
			.find(|window| window.is_window_of(&start))
			.cloned(),
	};

	// From an environment, or one of its machines, several of the environment's
	// machines can be chosen too, listed between the environment and its
	// machines.
	// spec: MNT#choosing-what-to-cover
	let environment: Option<(Uuid, ServerRank)> = match &start {
		Cover::Grain(grain) => match (grain.scope(), grain.rank()) {
			(Scope::Group(group), Some(rank)) => Some((group, rank)),
			(Scope::Machine(machine), _) => {
				let machine = Machine::get_by_id(&mut conn, machine).await?;
				machine.group_id.zip(machine.environment_rank())
			}
			_ => None,
		},
		Cover::Machines(machines) => Some(environment_of_machine_set(&mut conn, machines).await?),
	};
	if let Some((group, rank)) = environment {
		let members: Vec<Machine> = Machine::list_for_group(&mut conn, group)
			.await?
			.into_iter()
			.filter(|machine| machine.environment_rank() == Some(rank))
			.collect();
		let at = choices.iter().position(|choice| {
			choice.target.rank == Some(rank) && choice.target.server_group_id == Some(group)
		});
		if let (Some(at), true) = (at, members.len() >= 2) {
			let machines = members
				.iter()
				.map(|machine| MaintenanceMachineChoice {
					machine_id: machine.id,
					name: machine.name.clone(),
					window: open.get(&Grain::machine(machine.id)).cloned(),
					has_failures: failing.as_ref().map(|failing| {
						failing.iter().any(|target| {
							target.is_some_and(|target| {
								descent.covers(Grain::machine(machine.id), target)
							})
						})
					}),
				})
				.collect();
			let own_set = own.as_ref().filter(|window| window.is_machine_set());
			let named = own_set
				.map(|window| window.machine_ids.clone())
				.unwrap_or_default();
			let depth = choices[at].depth + 1;
			choices.insert(
				at + 1,
				MaintenanceTargetChoice {
					target: MaintenanceTarget::from(Cover::Machines(named)),
					label: "Several machines".into(),
					depth,
					window: own_set.cloned(),
					covers_failures: None,
					machines: Some(machines),
				},
			);
		}
	}

	let held_in_place = match &own {
		Some(window) => window.held_in_place(&mut conn).await?,
		None => None,
	};
	Ok(Json(MaintenanceTargets {
		choices,
		amends: own,
		held_in_place,
	}))
}

/// Lift a window before its expected end.
///
/// Suspension runs on for the settle period, after which the target is
/// watched again. Lifting a window that has already ended changes nothing.
/// Requires admin access.
#[utoipa::path(
	post,
	path = "/lift",
	tag = "maintenance",
	security(("tailscale-admin" = [])),
	request_body = LiftArgs,
	responses(
		(status = 200, body = MaintenanceWindow),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn lift(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<LiftArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = audit.begin().await?;
	let already_ended = MaintenanceWindow::get(&mut conn, args.id)
		.await?
		.ended_at
		.is_some();
	let window = MaintenanceWindow::lift(&mut conn, args.id, Some(&admin.0.login)).await?;
	let (record, target) = window_records(&mut conn, &window).await?;
	conn.record(Action::MaintenanceLifted {
		window: record,
		target,
		rank: window.rank,
		already_ended,
	})
	.await?;
	Ok(Json(window))
}
