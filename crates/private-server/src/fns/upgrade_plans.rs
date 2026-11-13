use std::collections::{HashMap, hash_map::Entry};

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::{audit::Audit, tailscale_auth::TailscaleAdmin};
use commons_types::server::rank::ServerRank;
use database::{
	audit::{Action, Record},
	server_groups::ServerGroup,
	upgrade_plans::{PlanOutcome, PlannedWhen, UpgradePlan},
};
use jiff::{Timestamp, Zoned, civil::Date, civil::Time};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

/// The history is for reading, not paging: enough to cover what the fleet has
/// planned recently without shipping every plan ever recorded.
const HISTORY_LIMIT: i64 = 100;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: fleet))
		.routes(routes!(read_only, silent: history))
		.routes(routes!(read_only, silent: targets))
		.routes(routes!(read_only, silent: for_group))
		.routes(routes!(write, audit(UpgradePlanRecorded): record))
		.routes(routes!(write, audit(UpgradePlanAmended): amend))
		.routes(routes!(write, audit(UpgradePlanWithdrawn): withdraw))
}

/// The hours a plan says its work runs, resolved to instants.
#[derive(Serialize, ToSchema)]
pub struct PlannedWindow {
	/// When the work is planned to start.
	#[schema(value_type = String)]
	pub starts_at: jiff::Timestamp,
	/// When it is planned to be over. A window closing earlier in the day than
	/// it opened runs into the next morning.
	#[schema(value_type = String)]
	pub ends_at: jiff::Timestamp,
}

/// One row of the planned-upgrades view: one of a group's environments.
#[derive(Serialize, ToSchema)]
pub struct PlannedUpgrade {
	/// The group this concerns.
	pub group_id: Uuid,
	/// Its name, so the view reads without a second lookup.
	pub group_name: String,
	/// The rank of the environment this concerns: the group's applications at
	/// that rank.
	pub rank: ServerRank,
	/// The version the environment runs now, where it has reported one.
	pub current_version: Option<String>,
	/// The plan, absent for an environment with none.
	pub plan: Option<UpgradePlan>,
	/// The plan's target as semver.
	pub target_version: Option<String>,
	/// Whether the planned date has passed without the upgrade happening.
	/// Presentational: a slipping upgrade is normal operational reality.
	pub late: bool,
	/// Whether a restore attempt is under way, carried beside the verdicts rather
	/// than folded into them: a restore takes hours, so a group mid-test would
	/// otherwise read as untested for the whole window.
	pub attempt: Option<crate::fns::migration_tests::AttemptState>,
	/// The environment's applications a testing declaration restores, each with
	/// where it stands against the planned version. Empty without a plan, and
	/// where nothing tests the environment.
	// spec: RST#verdicts
	pub tests: Vec<TestedServer>,
	/// The most recent ask for this environment to be tested against its plan
	/// that has yet to be answered for every machine it covers. `null` where
	/// none is pending.
	// spec: RST#dispatching-a-migration-test
	pub test_request: Option<TestRequest>,
	/// How the environment is tested: `scheduled` where a declaration covering
	/// it tests weekly and in the day before the upgrade, `on_request` where
	/// every one covering it waits to be asked. `null` where nothing tests it.
	// spec: RST#dispatching-a-migration-test
	pub testing: Option<database::restore::Testing>,
	/// When the plan's own window opens and closes, where it recorded one. The
	/// hours the operator said the work runs, so declaring over it can offer
	/// exactly those rather than a guess from now.
	// spec: UPG#when-a-plan-is-met
	pub planned_window: Option<PlannedWindow>,
	/// The window holding over this environment or its group, where one is.
	/// This is what holds an open plan open, so the view can both say why one
	/// has not closed and amend the work from there.
	// spec: UPG#when-a-plan-is-met
	pub maintenance_window: Option<database::maintenance_windows::MaintenanceWindow>,
}

/// Planned upgrades across the fleet.
///
/// Every environment of every live group, whether or not it has a plan, since
/// recording a plan starts from an environment listed here.
// spec: UPG#the-dashboard
#[utoipa::path(
	post,
	path = "/fleet",
	operation_id = "upgrade_plans_fleet",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "One row per environment of each live group.", body = Vec<PlannedUpgrade>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn fleet(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<PlannedUpgrade>>> {
	let mut conn = state.db.get().await?;
	// Late is judged against the server's own day; nothing depends on it beyond
	// presentation.
	let today = Zoned::now().date();
	let now_ts = jiff::Timestamp::now();

	let groups = ServerGroup::list_all(&mut conn).await?;
	let ids: Vec<Uuid> = groups.iter().map(|group| group.id).collect();
	let names: HashMap<Uuid, String> = groups
		.into_iter()
		.map(|group| (group.id, group.name))
		.collect();
	let mut open: HashMap<(Uuid, ServerRank), UpgradePlan> = UpgradePlan::all_open(&mut conn)
		.await?
		.into_iter()
		.map(|plan| ((plan.group_id, plan.rank), plan))
		.collect();
	let plan_ids: Vec<Uuid> = open.values().map(|plan| plan.id).collect();
	let mut requests: HashMap<Uuid, Vec<database::migration_tests::MigrationTestRequest>> =
		HashMap::new();
	for request in
		database::migration_tests::MigrationTestRequest::pending_for_plans(&mut conn, &plan_ids)
			.await?
	{
		requests.entry(request.plan_id).or_default().push(request);
	}
	let mut attempts: HashMap<Uuid, Option<crate::fns::migration_tests::AttemptState>> =
		HashMap::new();
	let mut members: HashMap<Uuid, Vec<database::applications::Application>> = HashMap::new();
	let mut migrating: HashMap<Uuid, database::restore::MigratingEnvironments> = HashMap::new();
	// Including drafts: a target yanked since the plan was recorded still has to
	// render as the version the environment is going to.
	let versions: HashMap<Uuid, database::versions::Version> =
		database::versions::Version::get_all_including_drafts(&mut conn)
			.await?
			.into_iter()
			.map(|version| (version.id, version))
			.collect();

	// The windows themselves, so the view can amend the work rather than only
	// report it. Everything holding needs is on these rows, so this is the only
	// read of the table the view makes.
	let open_windows =
		database::maintenance_windows::MaintenanceWindow::list_open(&mut conn).await?;
	// A window over the group covers its environments, and the environment's own
	// is the more specific of the two, so index both and prefer the specific. A
	// window over some of an environment's machines is neither.
	let mut holding: HashMap<(Uuid, Option<ServerRank>), &_> = HashMap::new();
	for window in &open_windows {
		if let Some(group) = window.server_group_id
			&& !window.is_machine_set()
			&& window.ended_at.is_none()
			&& window.holds_at(now_ts)
		{
			holding.insert((group, window.rank), window);
		}
	}
	let mut environments = ServerGroup::environments(&mut conn, &ids).await?;
	// A plan whose environment has no live application any more still says
	// where the group was going, and this view is the only place it can be
	// withdrawn.
	for ((group_id, rank), _) in open.iter() {
		if names.contains_key(group_id)
			&& !environments
				.iter()
				.any(|env| env.group_id == *group_id && env.rank == *rank)
		{
			environments.push(database::server_groups::Environment {
				group_id: *group_id,
				rank: *rank,
				headline: false,
				version: None,
			});
		}
	}

	let mut out = Vec::new();
	for env in environments {
		let plan = open.remove(&(env.group_id, env.rank));
		let planned_window = plan.as_ref().and_then(|plan| {
			database::upgrade_plans::planned_window(plan)
				.map(|(starts_at, ends_at)| PlannedWindow { starts_at, ends_at })
		});
		let planned = plan
			.as_ref()
			.and_then(|plan| versions.get(&plan.target_version_id));
		let target = planned.map(|version| version.as_semver().to_string());
		let late = plan
			.as_ref()
			.is_some_and(|plan| database::upgrade_plans::is_late(plan, today));

		// The plan says where the environment is going; the verdicts say whether
		// its data survives getting there, from each application a declaration
		// tests. One nothing tests has nothing to say, so it is left out.
		let mut testing = None;
		let mut tests = Vec::new();
		let mut test_request = None;
		if let Some(plan) = &plan {
			let declared = match migrating.entry(env.group_id) {
				Entry::Occupied(held) => held.into_mut(),
				Entry::Vacant(slot) => slot.insert(
					database::restore::migrating_environments(&mut conn, env.group_id).await?,
				),
			};
			testing = declared.testing(env.rank);
			if !members.contains_key(&env.group_id) {
				let live = database::applications::Application::list_live_in_group(
					&mut conn,
					env.group_id,
				)
				.await?;
				members.insert(env.group_id, live);
			}
			let applications: Vec<_> = members[&env.group_id]
				.iter()
				.filter(|application| {
					application.rank == Some(env.rank)
						&& application
							.machine_id
							.is_some_and(|machine_id| declared.tests_machine(machine_id))
				})
				.cloned()
				.collect();
			tests = database::migration_tests::verdicts_against(
				&mut conn,
				applications.clone(),
				planned,
			)
			.await?
			.into_iter()
			.map(|verdict| TestedServer {
				name: applications
					.iter()
					.find(|application| application.id == verdict.server_id)
					.map(database::applications::Application::display_name)
					.unwrap_or_else(|| "Unknown application".to_string()),
				verdict,
			})
			.collect();
			test_request = requests
				.get(&plan.id)
				.into_iter()
				.flatten()
				.filter(|request| {
					applications
						.iter()
						.any(|application| application.machine_id == Some(request.machine_id))
				})
				.max_by_key(|request| request.requested_at)
				.map(|request| TestRequest {
					requested_at: request.requested_at,
					requested_by: request.requested_by.clone(),
				});
		}

		// Issuances carry no intent, so another intent's restore traffic would
		// read as a test under way. They carry no rank either, so an attempt is
		// the group's.
		let dispatchable = match testing {
			Some(database::restore::Testing::Scheduled) => true,
			Some(database::restore::Testing::OnRequest) => test_request.is_some(),
			None => false,
		};
		let attempt = if dispatchable {
			match attempts.get(&env.group_id) {
				Some(attempt) => *attempt,
				None => {
					let attempt =
						crate::fns::migration_tests::attempt_state(&mut conn, env.group_id, now_ts)
							.await?;
					attempts.insert(env.group_id, attempt);
					attempt
				}
			}
		} else {
			None
		};

		out.push(PlannedUpgrade {
			group_id: env.group_id,
			group_name: names.get(&env.group_id).cloned().unwrap_or_default(),
			rank: env.rank,
			current_version: env.version.map(|v| v.to_string()),
			plan,
			target_version: target,
			late,
			attempt,
			tests,
			test_request,
			testing,
			planned_window,
			maintenance_window: holding
				.get(&(env.group_id, Some(env.rank)))
				.or_else(|| holding.get(&(env.group_id, None)))
				.map(|window| (*window).clone()),
		});
	}

	Ok(Json(out))
}

/// One of an environment's applications that a declaration tests, and where it
/// stands against the environment's planned version.
#[derive(Serialize, ToSchema)]
pub struct TestedServer {
	/// The application's name, so the view reads without a second lookup.
	pub name: String,
	/// Where it stands.
	#[serde(flatten)]
	pub verdict: database::migration_tests::GroupVerdict,
}

/// An ask for an environment to be migration-tested, still waiting on a verdict.
#[derive(Serialize, ToSchema)]
pub struct TestRequest {
	/// When it was asked for.
	#[schema(value_type = String, format = DateTime)]
	pub requested_at: jiff::Timestamp,
	/// Who asked.
	pub requested_by: Option<String>,
}

/// One plan that has closed, in the fleet's plan history.
#[derive(Serialize, ToSchema)]
pub struct PastPlan {
	/// The plan itself.
	pub plan: UpgradePlan,
	/// The group it was for.
	pub group_id: Uuid,
	/// Its name, so the view reads without a second lookup.
	pub group_name: String,
	/// Where the plan was going, as semver.
	pub target_version: String,
	/// How it closed.
	pub outcome: PlanOutcome,
	/// When it closed.
	#[schema(value_type = String)]
	pub ended_at: Timestamp,
}

/// The plans that have closed, across the fleet.
///
/// A group that stopped going somewhere leaves no other mark on the fleet,
/// so a withdrawn plan is readable here or nowhere.
// spec: UPG#the-dashboard
#[utoipa::path(
	post,
	path = "/history",
	operation_id = "upgrade_plans_history",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "Closed plans, most recently closed first.", body = Vec<PastPlan>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn history(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<PastPlan>>> {
	let mut conn = state.db.get().await?;
	let plans = UpgradePlan::closed_recent(&mut conn, HISTORY_LIMIT).await?;

	let group_ids: Vec<Uuid> = plans.iter().map(|plan| plan.group_id).collect();
	let group_names: HashMap<Uuid, String> =
		database::server_groups::ServerGroup::list_by_ids(&mut conn, &group_ids)
			.await?
			.into_iter()
			.map(|group| (group.id, group.name))
			.collect();
	// Including drafts: a target yanked since the plan was recorded still has to
	// render as the version the group was going to.
	let versions: HashMap<Uuid, String> =
		database::versions::Version::get_all_including_drafts(&mut conn)
			.await?
			.into_iter()
			.map(|version| (version.id, version.as_semver().to_string()))
			.collect();

	Ok(Json(
		plans
			.into_iter()
			.filter_map(|plan| {
				Some(PastPlan {
					group_id: plan.group_id,
					group_name: group_names.get(&plan.group_id).cloned()?,
					target_version: versions.get(&plan.target_version_id).cloned()?,
					outcome: database::upgrade_plans::outcome(&plan),
					ended_at: database::upgrade_plans::ended_at(&plan)?,
					plan,
				})
			})
			.collect(),
	))
}

/// Request body for reading one group's plans. Named apart from the
/// migration-test one because utoipa keys component schemas by short name.
#[derive(Deserialize, ToSchema)]
pub struct PlansForGroupArgs {
	/// The group to read.
	pub group_id: Uuid,
}

/// A group's plan and the plans it has had before.
///
/// The history is the record of what a group planned, when for, and when
/// it landed.
// spec: UPG#when-a-plan-is-met
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "upgrade_plans_for_group",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	request_body = PlansForGroupArgs,
	responses(
		(status = 200, description = "Every plan the group has had, newest first.", body = Vec<UpgradePlan>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<PlansForGroupArgs>,
) -> Result<Json<Vec<UpgradePlan>>> {
	let mut conn = state.db.get().await?;
	Ok(Json(
		UpgradePlan::history_for_group(&mut conn, args.group_id).await?,
	))
}

/// A version a group could be planned onto.
#[derive(Serialize, ToSchema)]
pub struct PlannableVersion {
	/// The version's identifier, for `record`.
	pub id: Uuid,
	/// Its semver.
	pub version: String,
	/// Whether it is clear of unresolved known issues, the same gate that keeps
	/// a version off the public listings. A flagged version is still offered:
	/// the issue may be resolved well before the planned date.
	pub ready: bool,
}

/// Request body for the versions an environment could be planned onto.
#[derive(Deserialize, ToSchema)]
pub struct TargetsArgs {
	/// The group.
	pub group_id: Uuid,
	/// The rank of the environment within it.
	pub rank: ServerRank,
}

/// The versions an environment could be planned onto: published, and ahead of
/// what it runs.
///
/// Offering only valid targets is what keeps the operator from picking one
/// `record` would refuse.
// spec: UPG#a-plan
#[utoipa::path(
	post,
	path = "/targets",
	operation_id = "upgrade_plans_targets",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	request_body = TargetsArgs,
	responses(
		(status = 200, description = "Plannable versions, newest first.", body = Vec<PlannableVersion>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn targets(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<TargetsArgs>,
) -> Result<Json<Vec<PlannableVersion>>> {
	let mut conn = state.db.get().await?;
	let group = ServerGroup::get_by_id(&mut conn, args.group_id).await?;
	let Some(environment) = ServerGroup::environment(&mut conn, args.group_id, args.rank).await?
	else {
		return Err(commons_errors::AppError::BadRequest(format!(
			"{} has no {} environment",
			group.name, args.rank
		)));
	};
	let running = environment.version;

	// get_all is already newest-first; keep that for the picker.
	let ahead: Vec<database::versions::Version> = database::versions::Version::get_all(&mut conn)
		.await?
		.into_iter()
		.filter(|version| {
			running
				.as_ref()
				.is_none_or(|running| version.as_semver() > running.0)
		})
		.collect();

	let ids: Vec<Uuid> = ahead.iter().map(|version| version.id).collect();
	let flagged =
		database::version_known_issues::VersionKnownIssue::affected_versions(&mut conn, &ids)
			.await?;

	Ok(Json(
		ahead
			.into_iter()
			.map(|version| PlannableVersion {
				ready: !flagged.contains(&version.id),
				id: version.id,
				version: version.as_semver().to_string(),
			})
			.collect(),
	))
}

/// Request body for recording where a group is going.
#[derive(Deserialize, ToSchema)]
pub struct RecordArgs {
	/// The group whose environment intends to move.
	pub group_id: Uuid,
	/// The rank of the environment within it that intends to move.
	pub rank: ServerRank,
	/// The published version it intends to move to.
	pub target_version_id: Uuid,
	/// The day it is expected to happen, as `YYYY-MM-DD`. Optional.
	#[schema(value_type = Option<String>)]
	pub planned_for: Option<Date>,
	/// The hour it starts on that day, as `HH:MM`. Optional, and needs a day
	/// and a zone.
	#[schema(value_type = Option<String>)]
	pub planned_time: Option<Time>,
	/// The hour the window closes, as `HH:MM`. Optional, and needs a start.
	/// Earlier than the start means the following morning.
	#[schema(value_type = Option<String>)]
	pub planned_end_time: Option<Time>,
	/// The IANA zone the planned time is a wall clock in, such as
	/// `Pacific/Fiji`. Required alongside a time.
	pub planned_zone: Option<String>,
	/// Anything the next reader needs to know. Optional.
	pub note: Option<String>,
}

/// Record where an environment is going, retiring any plan it already had.
///
/// An environment goes one place next, so this replaces rather than queues. The
/// target must be published and ahead of what the environment runs.
// spec: UPG#a-plan
#[utoipa::path(
	post,
	path = "/record",
	operation_id = "upgrade_plans_record",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	request_body = RecordArgs,
	responses(
		(status = 200, description = "The recorded plan.", body = UpgradePlan),
		(status = 400, description = "The target is unpublished, or not ahead of the environment.", body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn record(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<RecordArgs>,
) -> Result<Json<UpgradePlan>> {
	let mut conn = audit.begin().await?;
	let note = args
		.note
		.as_deref()
		.map(str::trim)
		.filter(|n| !n.is_empty());
	let plan = UpgradePlan::record(
		&mut conn,
		args.group_id,
		args.rank,
		args.target_version_id,
		PlannedWhen {
			date: args.planned_for,
			time: args.planned_time,
			end: args.planned_end_time,
			zone: args.planned_zone,
		},
		note,
		&admin.0.login,
	)
	.await?;
	let plan_record = Record::upgrade_plan(&mut conn, &plan).await?;
	let group_record = Record::group(&mut conn, plan.group_id).await?;
	conn.record(Action::UpgradePlanRecorded {
		plan: plan_record,
		group: group_record,
		rank: plan.rank,
		planned_for: plan.planned_for.map(|date| date.to_string()),
		planned_time: plan.planned_time.map(|time| time.to_string()),
		note: plan.note.clone(),
	})
	.await?;
	Ok(Json(plan))
}

/// Request body for amending an open plan.
#[derive(Deserialize, ToSchema)]
pub struct AmendArgs {
	/// The plan to amend.
	pub id: Uuid,
	/// The day it is expected to happen, as `YYYY-MM-DD`. Cleared when absent.
	#[schema(value_type = Option<String>)]
	pub planned_for: Option<Date>,
	/// The hour it starts on that day, as `HH:MM`. Cleared when absent, and
	/// needs a day and a zone.
	#[schema(value_type = Option<String>)]
	pub planned_time: Option<Time>,
	/// The hour the window closes, as `HH:MM`. Cleared when absent, and needs a
	/// start. Earlier than the start means the following morning.
	#[schema(value_type = Option<String>)]
	pub planned_end_time: Option<Time>,
	/// The IANA zone the planned time is a wall clock in, such as
	/// `Pacific/Fiji`. Required alongside a time.
	pub planned_zone: Option<String>,
	/// Anything the next reader needs to know. Cleared when absent.
	pub note: Option<String>,
}

/// Amend an open plan's date and note.
///
/// The same plan better described, so it keeps its place in the history rather
/// than being replaced. Moving a group to a different version is a new plan:
/// record that instead.
// spec: UPG#a-plan
#[utoipa::path(
	post,
	path = "/amend",
	operation_id = "upgrade_plans_amend",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	request_body = AmendArgs,
	responses(
		(status = 200, description = "The amended plan.", body = UpgradePlan),
		(status = 400, description = "The plan has been met or replaced, so it is history.", body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn amend(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<AmendArgs>,
) -> Result<Json<UpgradePlan>> {
	let mut conn = audit.begin().await?;
	let note = args
		.note
		.as_deref()
		.map(str::trim)
		.filter(|n| !n.is_empty());
	let plan = UpgradePlan::amend(
		&mut conn,
		args.id,
		PlannedWhen {
			date: args.planned_for,
			time: args.planned_time,
			end: args.planned_end_time,
			zone: args.planned_zone,
		},
		note,
		&admin.0.login,
	)
	.await?;
	let plan_record = Record::upgrade_plan(&mut conn, &plan).await?;
	let group_record = Record::group(&mut conn, plan.group_id).await?;
	conn.record(Action::UpgradePlanAmended {
		plan: plan_record,
		group: group_record,
		rank: plan.rank,
		planned_for: plan.planned_for.map(|date| date.to_string()),
		planned_time: plan.planned_time.map(|time| time.to_string()),
		note: plan.note.clone(),
	})
	.await?;
	Ok(Json(plan))
}

/// Request body for withdrawing a plan.
#[derive(Deserialize, ToSchema)]
pub struct WithdrawArgs {
	/// The plan to withdraw.
	pub id: Uuid,
}

/// Withdraw a plan: the group is no longer going there.
///
/// This does not say the upgrade happened. Canopy closes a met plan on its own
/// once the group reports the target.
// spec: UPG#a-plan
#[utoipa::path(
	post,
	path = "/withdraw",
	operation_id = "upgrade_plans_withdraw",
	tag = "upgrade_plans",
	security(("tailscale-admin" = [])),
	request_body = WithdrawArgs,
	responses(
		(status = 200, description = "Withdrawn (idempotent)."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn withdraw(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<WithdrawArgs>,
) -> Result<Json<()>> {
	let mut conn = audit.begin().await?;
	// A plan already closed has nothing to withdraw, which is no change.
	let Some(plan) = UpgradePlan::withdraw(&mut conn, args.id, &admin.0.login).await? else {
		audit.unchanged();
		return Ok(Json(()));
	};
	let plan_record = Record::upgrade_plan(&mut conn, &plan).await?;
	let group_record = Record::group(&mut conn, plan.group_id).await?;
	conn.record(Action::UpgradePlanWithdrawn {
		plan: plan_record,
		group: group_record,
		rank: plan.rank,
	})
	.await?;
	Ok(Json(()))
}
