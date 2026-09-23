//! `find_groups` / `get_group` tools.

use std::collections::HashMap;

use commons_types::{
	Uuid,
	backup::{BackupConfigStatus, BackupPlacement, BackupRepoMode},
	server::rank::ServerRank,
	version::VersionStr,
};
use database::{
	backups::{
		BackupMaintenanceRun, BackupRepoSnapshot, BackupRepoStats, BackupRun,
		ServerGroupBackupConfig, ServerGroupBackupSchedule,
	},
	reported_detail::ReportedDetail,
	server_groups::ServerGroup,
	statuses::Status,
};
use jiff::Timestamp;
use rmcp::{
	handler::server::wrapper::Parameters,
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
	CanopyMcp,
	applications::{Retained, ServerSummary, summarize},
	util::{mcp_err, not_found, ok_json, parse_uuid},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindGroupsArgs {
	/// Free-text term matched against group name or id. Omit to list all.
	pub query: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct GroupIdArgs {
	/// The group's id.
	pub group_id: String,
}

#[derive(Serialize)]
struct GroupList {
	groups: Vec<GroupSummary>,
}

#[derive(Serialize)]
struct GroupSummary {
	id: Uuid,
	name: String,
	member_count: i64,
	effective_version: Option<VersionStr>,
	highest_rank: Option<ServerRank>,
	backup_config: Option<BackupConfigStatus>,
	last_backup_at: Option<Timestamp>,
}

#[derive(Serialize)]
struct BackupConfigOut {
	status: BackupConfigStatus,
	bucket: String,
	prefix: String,
	region: Option<String>,
	mode: BackupRepoMode,
	placement: BackupPlacement,
	last_init_error: Option<String>,
	created_at: Timestamp,
	updated_at: Timestamp,
}

#[derive(Serialize)]
struct GroupBackups {
	config: Option<BackupConfigOut>,
	schedules: Vec<ServerGroupBackupSchedule>,
	repo_stats: Option<BackupRepoStats>,
	recent_runs: Vec<BackupRun>,
	maintenance_runs: Vec<BackupMaintenanceRun>,
	repo_snapshots: Vec<BackupRepoSnapshot>,
	last_inspected_at: Option<Timestamp>,
}

#[derive(Serialize)]
struct GroupDetail {
	id: Uuid,
	name: String,
	notes: String,
	tags: commons_types::server::TagMap,
	effective_version: Option<VersionStr>,
	created_at: Timestamp,
	updated_at: Timestamp,
	members: Vec<ServerSummary>,
	backups: GroupBackups,
}

#[tool_router(router = groups_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Find server groups by name/id, with live member count, effective version, \
		               highest member rank, backup config state, and last-backup time."
	)]
	async fn find_groups(
		&self,
		Parameters(args): Parameters<FindGroupsArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let groups = match args.query.as_deref() {
			Some(q) if !q.is_empty() => ServerGroup::search(&mut conn, q).await.map_err(mcp_err)?,
			_ => ServerGroup::list_all(&mut conn).await.map_err(mcp_err)?,
		};
		let ids: Vec<Uuid> = groups.iter().map(|g| g.id).collect();
		let counts = ServerGroup::live_server_counts(&mut conn)
			.await
			.map_err(mcp_err)?;
		let ranks = ServerGroup::highest_member_ranks(&mut conn, &ids)
			.await
			.map_err(mcp_err)?;
		let configs = ServerGroupBackupConfig::list(&mut conn)
			.await
			.map_err(mcp_err)?;
		let cfg_status: HashMap<Uuid, BackupConfigStatus> =
			configs.iter().map(|c| (c.group_id, c.status)).collect();

		let mut out = Vec::with_capacity(groups.len());
		for g in &groups {
			let last_backup_at = BackupRun::latest_backup_at_for_group(&mut conn, g.id)
				.await
				.map_err(mcp_err)?;
			out.push(GroupSummary {
				id: g.id,
				name: g.name.clone(),
				member_count: counts.get(&g.id).copied().unwrap_or(0),
				effective_version: g.effective_version.clone(),
				highest_rank: ranks.get(&g.id).cloned(),
				backup_config: cfg_status.get(&g.id).copied(),
				last_backup_at,
			});
		}
		ok_json(&GroupList { groups: out })
	}

	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Full detail for one group: members (with version/health), backup config, \
		               schedules, repo stats, and recent backup/maintenance activity."
	)]
	async fn get_group(
		&self,
		Parameters(args): Parameters<GroupIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let id = parse_uuid(&args.group_id, "group_id")?;
		let Ok(group) = ServerGroup::get_by_id(&mut conn, id).await else {
			return Ok(not_found(format!("no group with id {id}")));
		};

		let members = group.list_servers(&mut conn).await.map_err(mcp_err)?;
		let mids: Vec<Uuid> = members.iter().map(|s| s.id).collect();
		let statuses = Status::latest_for_servers(&mut conn, &mids)
			.await
			.map_err(mcp_err)?;
		let st_by: HashMap<Uuid, &Status> = statuses
			.iter()
			.filter_map(|s| Some((s.server_id?, s)))
			.collect();
		// Same retained-version resolution as `find_servers`: a member quiet
		// for more than the status window still ran something last time
		// anyone heard from it.
		let missed: Vec<Uuid> = mids
			.iter()
			.copied()
			.filter(|id| !st_by.contains_key(id))
			.collect();
		let last_versions = ReportedDetail::last_versions(&mut conn, &missed)
			.await
			.map_err(mcp_err)?;
		let last_reported = ReportedDetail::last_reported_ats(&mut conn, &mids)
			.await
			.map_err(mcp_err)?;
		let member_groups: Vec<(Uuid, Option<Uuid>)> =
			members.iter().map(|s| (s.id, s.group_id)).collect();
		let health = database::issues::health_from_check_state(&mut conn, &member_groups)
			.await
			.map_err(mcp_err)?;
		let member_summaries = members
			.iter()
			.map(|s| {
				summarize(
					s,
					st_by.get(&s.id).copied(),
					Retained {
						version: last_versions.get(&s.id).cloned(),
						last_reported_at: last_reported.get(&s.id).copied(),
					},
					Some(group.name.clone()),
					health.get(&s.id).copied().unwrap_or_default(),
				)
			})
			.collect();

		let config = ServerGroupBackupConfig::get(&mut conn, id)
			.await
			.map_err(mcp_err)?
			.map(|c| BackupConfigOut {
				status: c.status,
				bucket: c.bucket,
				prefix: c.prefix,
				region: c.region,
				mode: c.mode,
				placement: c.placement,
				last_init_error: c.last_init_error,
				created_at: c.created_at,
				updated_at: c.updated_at,
			});
		let schedules = ServerGroupBackupSchedule::list_for_group(&mut conn, id)
			.await
			.map_err(mcp_err)?;
		let repo_stats = BackupRepoStats::get(&mut conn, id).await.map_err(mcp_err)?;
		let recent_runs = BackupRun::list_for_group(&mut conn, id, 10)
			.await
			.map_err(mcp_err)?;
		let maintenance_runs = BackupMaintenanceRun::list_for_group(&mut conn, id, 5)
			.await
			.map_err(mcp_err)?;
		let repo_snapshots = BackupRepoSnapshot::list_for_group(&mut conn, id)
			.await
			.map_err(mcp_err)?;
		let last_inspected_at = BackupRepoSnapshot::last_inspected_at_for_group(&mut conn, id)
			.await
			.map_err(mcp_err)?;

		ok_json(&GroupDetail {
			id: group.id,
			name: group.name.clone(),
			notes: group.notes.clone(),
			tags: group.tags.clone(),
			effective_version: group.effective_version.clone(),
			created_at: group.created_at,
			updated_at: group.updated_at,
			members: member_summaries,
			backups: GroupBackups {
				config,
				schedules,
				repo_stats,
				recent_runs,
				maintenance_runs,
				repo_snapshots,
				last_inspected_at,
			},
		})
	}
}
