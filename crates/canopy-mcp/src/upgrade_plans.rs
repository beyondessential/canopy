//! `list_upgrade_plans` / `get_upgrade_plan_history` tools.

use std::collections::HashMap;

use commons_types::{Uuid, server::rank::ServerRank, version::VersionStr};
use database::{
	server_groups::ServerGroup,
	upgrade_plans::{PlanOutcome, UpgradePlan},
	versions::Version,
};
use jiff::{Timestamp, Zoned, civil::Date, civil::Time};
use rmcp::{
	handler::server::wrapper::Parameters,
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use serde::Serialize;

use crate::{
	CanopyMcp,
	groups::GroupIdArgs,
	util::{EmptyArgs, mcp_err, not_found, ok_json, parse_uuid},
};

#[derive(Serialize)]
struct PlanList {
	plans: Vec<OpenPlan>,
	/// Each group's highest-ranked environment that runs behind the newest
	/// published version with no open plan.
	environments_without_a_plan: Vec<EnvironmentRef>,
}

/// One of a group's environments: its applications at one rank.
#[derive(Serialize)]
struct EnvironmentRef {
	group_id: Uuid,
	group_name: String,
	rank: ServerRank,
	current_version: Option<VersionStr>,
	/// Majors times a thousand plus minors behind the newest published version.
	behind: Option<u64>,
}

#[derive(Serialize)]
struct OpenPlan {
	group_id: Uuid,
	group_name: String,
	/// The environment the plan is for: the group's applications at this rank.
	rank: ServerRank,
	current_version: Option<VersionStr>,
	target_version: String,
	planned_for: Option<Date>,
	/// The hour it starts on the planned day, as a wall clock in
	/// `planned_zone`.
	planned_time: Option<Time>,
	/// The hour the window closes. Earlier than the start means the next
	/// morning.
	planned_end_time: Option<Time>,
	planned_zone: Option<String>,
	/// The planned day has passed and the environment has not moved.
	late: bool,
	note: Option<String>,
	recorded_by: Option<String>,
	recorded_at: Timestamp,
}

#[derive(Serialize)]
struct PlanHistory {
	group_id: Uuid,
	group_name: String,
	/// The group's environments and what each runs now.
	environments: Vec<EnvironmentRef>,
	plans: Vec<HistoricPlan>,
}

#[derive(Serialize)]
struct HistoricPlan {
	/// The environment the plan was for: the group's applications at this rank.
	rank: ServerRank,
	target_version: String,
	outcome: PlanOutcome,
	planned_for: Option<Date>,
	/// The hour it started on the planned day, as a wall clock in
	/// `planned_zone`.
	planned_time: Option<Time>,
	/// The hour the window closed. Earlier than the start means the next
	/// morning.
	planned_end_time: Option<Time>,
	planned_zone: Option<String>,
	note: Option<String>,
	recorded_by: Option<String>,
	recorded_at: Timestamp,
	amended_by: Option<String>,
	amended_at: Option<Timestamp>,
	withdrawn_by: Option<String>,
	/// When the plan closed, absent while it is open.
	ended_at: Option<Timestamp>,
}

#[tool_router(router = upgrade_plans_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		description = "Where every environment is going: each open upgrade plan, per group and \
		               rank, with the version that environment runs now, the version it plans to \
		               move to, the planned date, and whether that date has passed unmet. Each \
		               group's highest-ranked environment that is behind the newest version with \
		               nothing recorded is returned separately."
	)]
	async fn list_upgrade_plans(
		&self,
		Parameters(_): Parameters<EmptyArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let today = Zoned::now().date();
		let versions = version_names(&mut conn).await?;

		let groups = ServerGroup::list_all(&mut conn).await.map_err(mcp_err)?;
		let ids: Vec<Uuid> = groups.iter().map(|group| group.id).collect();
		let names: HashMap<Uuid, String> = groups
			.into_iter()
			.map(|group| (group.id, group.name))
			.collect();

		let newest = Version::get_all(&mut conn)
			.await
			.map_err(mcp_err)?
			.into_iter()
			.next()
			.map(|version| version.as_semver());

		let mut plans = Vec::new();
		let mut unplanned = Vec::new();
		for env in ServerGroup::environments(&mut conn, &ids)
			.await
			.map_err(mcp_err)?
		{
			let headline = env.headline;
			let behind = env
				.version
				.as_ref()
				.zip(newest.as_ref())
				.map(|(current, latest)| database::statuses::version_distance(&current.0, latest));
			let group_name = names.get(&env.group_id).cloned().unwrap_or_default();
			let plan = UpgradePlan::open_for_environment(&mut conn, env.group_id, env.rank)
				.await
				.map_err(mcp_err)?;
			match plan {
				Some(plan) => plans.push(OpenPlan {
					group_id: env.group_id,
					group_name,
					rank: env.rank,
					current_version: env.version,
					target_version: version_name(&versions, plan.target_version_id),
					late: database::upgrade_plans::is_late(&plan, today),
					planned_for: plan.planned_for,
					planned_time: plan.planned_time,
					planned_end_time: plan.planned_end_time,
					planned_zone: plan.planned_zone,
					note: plan.note,
					recorded_by: plan.created_by,
					recorded_at: plan.created_at,
				}),
				None if headline && behind.is_some_and(|behind| behind > 0) => {
					unplanned.push(EnvironmentRef {
						group_id: env.group_id,
						group_name,
						rank: env.rank,
						current_version: env.version,
						behind,
					})
				}
				None => {}
			}
		}

		ok_json(&PlanList {
			plans,
			environments_without_a_plan: unplanned,
		})
	}

	#[tool(
		description = "Every upgrade plan one group's environments have had, newest first, with \
		               the rank each was for and how each stands: open, met (the environment \
		               reached the target), replaced by a later plan, or withdrawn (an operator \
		               said the environment is no longer going there)."
	)]
	async fn get_upgrade_plan_history(
		&self,
		Parameters(args): Parameters<GroupIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let id = parse_uuid(&args.group_id, "group_id")?;
		let Ok(group) = ServerGroup::get_by_id(&mut conn, id).await else {
			return Ok(not_found(format!("no group with id {id}")));
		};

		let versions = version_names(&mut conn).await?;
		let newest = Version::get_all(&mut conn)
			.await
			.map_err(mcp_err)?
			.into_iter()
			.next()
			.map(|version| version.as_semver());
		let plans = UpgradePlan::history_for_group(&mut conn, id)
			.await
			.map_err(mcp_err)?
			.into_iter()
			.map(|plan| HistoricPlan {
				rank: plan.rank,
				target_version: version_name(&versions, plan.target_version_id),
				outcome: database::upgrade_plans::outcome(&plan),
				ended_at: database::upgrade_plans::ended_at(&plan),
				planned_for: plan.planned_for,
				planned_time: plan.planned_time,
				planned_end_time: plan.planned_end_time,
				planned_zone: plan.planned_zone,
				note: plan.note,
				recorded_by: plan.created_by,
				recorded_at: plan.created_at,
				amended_by: plan.amended_by,
				amended_at: plan.amended_at,
				withdrawn_by: plan.withdrawn_by,
			})
			.collect();

		let environments = ServerGroup::environments(&mut conn, &[group.id])
			.await
			.map_err(mcp_err)?
			.into_iter()
			.map(|env| EnvironmentRef {
				group_id: env.group_id,
				group_name: group.name.clone(),
				rank: env.rank,
				behind: env
					.version
					.as_ref()
					.zip(newest.as_ref())
					.map(|(current, latest)| {
						database::statuses::version_distance(&current.0, latest)
					}),
				current_version: env.version,
			})
			.collect();

		ok_json(&PlanHistory {
			group_id: group.id,
			group_name: group.name,
			environments,
			plans,
		})
	}
}

/// Drafts included: a target yanked since the plan was recorded is still where
/// the group was going.
async fn version_names(
	conn: &mut database::diesel_async::AsyncPgConnection,
) -> Result<HashMap<Uuid, String>, McpError> {
	Ok(Version::get_all_including_drafts(conn)
		.await
		.map_err(mcp_err)?
		.into_iter()
		.map(|version| (version.id, version.as_semver().to_string()))
		.collect())
}

fn version_name(versions: &HashMap<Uuid, String>, id: Uuid) -> String {
	versions
		.get(&id)
		.cloned()
		.unwrap_or_else(|| "unknown".into())
}
