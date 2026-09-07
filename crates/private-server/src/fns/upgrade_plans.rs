use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::server::rank::ServerRank;
use database::{
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
		.routes(routes!(fleet))
		.routes(routes!(history))
		.routes(routes!(targets))
		.routes(routes!(for_group))
		.routes(routes!(record))
		.routes(routes!(amend))
		.routes(routes!(withdraw))
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
	/// Whether this is the group's highest-ranked environment, the one the
	/// group's own version is read from.
	pub headline: bool,
	/// The version the environment runs now, where it has reported one.
	pub current_version: Option<String>,
	/// How far that is behind the newest published version, as majors times a
	/// thousand plus minors. Zero where it is current; `null` where it has
	/// reported no version.
	pub behind: Option<u64>,
	/// The plan, absent for an environment with none.
	pub plan: Option<UpgradePlan>,
	/// The plan's target as semver.
	pub target_version: Option<String>,
	/// Whether the planned date has passed without the upgrade happening.
	/// Presentational: a slipping upgrade is normal operational reality.
	pub late: bool,
	/// Where the environment's data stands against the planned version, rolled
	/// up from its applications: any failure makes the environment a failure,
	/// since one application whose data breaks is enough to stop the upgrade.
	/// `null` without a plan.
	pub verdict: Option<String>,
	/// Whether a restore attempt is under way, carried beside the verdict rather
	/// than folded into it: a restore takes hours, so a group mid-test would
	/// otherwise read as untested for the whole window.
	pub attempt: Option<crate::fns::migration_tests::AttemptState>,
	/// Whether anything is declared to migrate this group's data. A plan on a
	/// group with nothing declared is never dispatched, so its verdict would sit
	/// at "not tested" indefinitely with nothing on its way. `null` without a
	/// plan.
	pub testable: Option<bool>,
}

/// Planned upgrades across the fleet.
///
/// Every environment of every live group, whether or not it has a plan. A
/// group several minors behind with no plan is the thing this view exists to
/// surface, so its environments are listed rather than omitted.
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
	let mut attempts: HashMap<Uuid, Option<crate::fns::migration_tests::AttemptState>> =
		HashMap::new();
	let mut members: HashMap<Uuid, Vec<database::applications::Application>> = HashMap::new();
	let newest = database::versions::Version::get_all(&mut conn)
		.await?
		.into_iter()
		.next()
		.map(|version| version.as_semver());
	let headline = ServerGroup::highest_member_ranks(&mut conn, &ids).await?;
	// Including drafts: a target yanked since the plan was recorded still has to
	// render as the version the environment is going to.
	let versions: HashMap<Uuid, String> =
		database::versions::Version::get_all_including_drafts(&mut conn)
			.await?
			.into_iter()
			.map(|version| (version.id, version.as_semver().to_string()))
			.collect();

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
		let target = plan
			.as_ref()
			.and_then(|plan| versions.get(&plan.target_version_id).cloned());
		let late = plan
			.as_ref()
			.is_some_and(|plan| database::upgrade_plans::is_late(plan, today));

		// The plan says where the environment is going; the verdict says whether
		// its data survives getting there. Pairing them is what makes this view
		// worth reading.
		let verdict = match &plan {
			None => None,
			Some(_) => {
				if !members.contains_key(&env.group_id) {
					let live = database::applications::Application::list_live_in_group(
						&mut conn,
						env.group_id,
					)
					.await?;
					members.insert(env.group_id, live);
				}
				// An unranked member belongs to the group's headline environment.
				let applications: Vec<_> = members[&env.group_id]
					.iter()
					.filter(|application| {
						application
							.rank
							.or_else(|| headline.get(&env.group_id).copied())
							== Some(env.rank)
					})
					.cloned()
					.collect();
				let per_server =
					database::migration_tests::verdicts(&mut conn, applications).await?;
				Some(roll_up(&per_server).to_owned())
			}
		};

		let testable = match &plan {
			None => None,
			Some(_) => Some(
				database::restore::environment_migrates(&mut conn, env.group_id, env.rank).await?,
			),
		};

		// Issuances carry no intent, so another intent's restore traffic would
		// read as a test under way. They carry no rank either, so an attempt is
		// the group's.
		let attempt = match testable {
			Some(true) => match attempts.get(&env.group_id) {
				Some(attempt) => *attempt,
				None => {
					let attempt =
						crate::fns::migration_tests::attempt_state(&mut conn, env.group_id, now_ts)
							.await?;
					attempts.insert(env.group_id, attempt);
					attempt
				}
			},
			_ => None,
		};

		let behind = env
			.version
			.as_ref()
			.zip(newest.as_ref())
			.map(|(current, latest)| database::statuses::version_distance(&current.0, latest));
		out.push(PlannedUpgrade {
			group_id: env.group_id,
			group_name: names.get(&env.group_id).cloned().unwrap_or_default(),
			rank: env.rank,
			headline: env.headline,
			current_version: env.version.map(|v| v.to_string()),
			behind,
			plan,
			target_version: target,
			late,
			verdict,
			attempt,
			testable,
		});
	}

	Ok(Json(out))
}

/// The group's standing against its planned version: the worst of its applications'.
///
/// One server whose data breaks the migrations is enough to stop the upgrade, so
/// a failure anywhere is the group's answer.
fn roll_up(per_server: &[database::migration_tests::GroupVerdict]) -> &'static str {
	use database::migration_tests::Verdict;

	if per_server.is_empty() {
		return "nottested";
	}
	if per_server.iter().any(|v| v.verdict == Verdict::Failed) {
		return "failed";
	}
	if per_server.iter().any(|v| v.verdict == Verdict::NotTested) {
		return "nottested";
	}
	"passed"
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
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<RecordArgs>,
) -> Result<Json<UpgradePlan>> {
	let mut conn = state.db.get().await?;
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
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<AmendArgs>,
) -> Result<Json<UpgradePlan>> {
	let mut conn = state.db.get().await?;
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
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<WithdrawArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	UpgradePlan::withdraw(&mut conn, args.id, &admin.0.login).await?;
	Ok(Json(()))
}
