use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::{Uuid, server::rank::ServerRank};
use database::issues::{Incident, Scope};
use database::maintenance_windows::{
	Amendment, Grain, HeldInPlace, MaintenanceWindow, TargetWindow, line_of_descent, target_labels,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

/// How many of a target's ended windows its history returns.
const HISTORY_LIMIT: i64 = 20;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list_open))
		.routes(routes!(read_only: for_target))
		.routes(routes!(read_only: targets))
		.routes(routes!(write: declare))
		.routes(routes!(write: amend))
		.routes(routes!(write: lift))
}

/// A target a window can cover, named the way a declaration names it: exactly
/// one of the ids, and with the group, optionally the rank of one of its
/// environments.
#[derive(Clone, Copy, Serialize, Deserialize, ToSchema)]
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
}

impl MaintenanceTarget {
	/// The application, machine, or group named, whatever the rank.
	fn scope(self) -> Result<Scope> {
		match (self.application_id, self.machine_id, self.server_group_id) {
			(Some(id), None, None) => Ok(Scope::Application(id)),
			(None, Some(id), None) => Ok(Scope::Machine(id)),
			(None, None, Some(id)) => Ok(Scope::Group(id)),
			_ => Err(AppError::BadRequest(
				"a maintenance window covers one application, one machine or one group".into(),
			)),
		}
	}

	fn grain(self) -> Result<Grain> {
		Grain::new(self.scope()?, self.rank)
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
	/// of descent that has no window of its own.
	pub target: Option<MaintenanceTarget>,
	/// A new expected end.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub expected_end: Option<Timestamp>,
	/// A new note. Null clears it; leaving it out keeps the window's own.
	#[serde(default, deserialize_with = "present")]
	#[schema(value_type = Option<String>, nullable)]
	pub note: Option<Option<String>>,
}

/// Tell a field sent as null apart from one left out.
fn present<'de, D, T>(deserializer: D) -> std::result::Result<Option<Option<T>>, D::Error>
where
	D: serde::Deserializer<'de>,
	T: Deserialize<'de>,
{
	Option::<T>::deserialize(deserializer).map(Some)
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
	/// of its failing checks.
	pub covers_failures: Option<bool>,
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
	let grains: Vec<Grain> = windows
		.iter()
		.filter_map(MaintenanceWindow::grain)
		.collect();
	let labels = target_labels(&mut conn, &grains).await?;
	let out = windows
		.into_iter()
		.map(|window| {
			let target = window
				.grain()
				.and_then(|grain| labels.get(&grain).cloned())
				.unwrap_or_default();
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

/// Declare that an application, a machine, a group, or one of a group's
/// environments is being worked on.
///
/// Every check on the target grades to skipped while the window holds and
/// for a settle period after it ends, so nothing on it opens or joins an
/// incident. A window over one application leaves the rest of the box watched;
/// one over the machine covers everything on it. Issues already in an open
/// incident leave it, closing the incident where nothing else holds it open. A
/// target that already has an open window has that window amended rather than
/// a second opened.
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
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<DeclareArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = state.db.get().await?;
	let grain = args.target.grain()?;
	let (scope, rank) = (grain.scope(), grain.rank());
	let window = match args.upgrade_plan_id {
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
	};
	Ok(Json(window))
}

/// Amend an open window: its end, its note, or what it covers.
///
/// Only what the request names changes. A new target moves the window there:
/// it stays the same window, the target it left settles as though the window
/// had ended over it, and what it newly covers is suspended from now. A window
/// declared from an upgrade plan, or one a configuration run's lease is being
/// served against, cannot move.
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
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<AmendWindowArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = state.db.get().await?;
	let window = MaintenanceWindow::amend(
		&mut conn,
		args.id,
		Amendment {
			target: args.target.map(MaintenanceTarget::grain).transpose()?,
			expected_end: args.expected_end,
			note: args
				.note
				.map(|note| note.filter(|note| !note.trim().is_empty())),
		},
		Some(&admin.0.login),
	)
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
	let start = args.start.grain()?;
	let descent = line_of_descent(&mut conn, start).await?;
	let open: std::collections::HashMap<Grain, MaintenanceWindow> =
		MaintenanceWindow::list_open(&mut conn)
			.await?
			.into_iter()
			.filter_map(|window| Some((window.grain()?, window)))
			.collect();

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

	let choices = descent
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
		})
		.collect();

	// The window the declaration amends: the one named, or else the starting
	// grain's own, since a declaration offered over a target with a window of
	// its own amends it from the start.
	// spec: MNT#moving-a-window
	// A window that has ended is amended by no one, so nothing holds it.
	let own = match args.window_id {
		Some(id) => {
			let window = MaintenanceWindow::get(&mut conn, id).await?;
			// The declaration amends the window it starts over, and no other.
			if window.grain() != Some(start) {
				return Err(AppError::BadRequest(
					"the window named is not the starting target's".into(),
				));
			}
			Some(window).filter(|window| window.ended_at.is_none())
		}
		None => open.get(&start).cloned(),
	};
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
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<LiftArgs>,
) -> Result<Json<MaintenanceWindow>> {
	let mut conn = state.db.get().await?;
	let window = MaintenanceWindow::lift(&mut conn, args.id, Some(&admin.0.login)).await?;
	Ok(Json(window))
}
