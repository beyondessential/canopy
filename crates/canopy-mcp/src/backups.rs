//! `list_backup_runs` / `list_maintenance_runs` / `get_backup_defaults` tools.

use commons_types::{
	Uuid,
	backup::{BackupType, MaintenanceKind, RunOutcome},
};
use database::{
	backups::{
		BackupMaintenanceRun, BackupMaintenanceRunFilters, BackupRun, BackupRunFilters,
		BackupTypeDefault, MaintenanceOutcomeFilter, RetentionPolicy,
	},
	machines::Machine,
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
	util::{
		EmptyArgs, group_names, mcp_err, ok_json, parse_opt, parse_opt_uuid, since_from_days,
		unique,
	},
};

/// Default / max rows for `list_backup_runs` and `list_maintenance_runs`.
const DEFAULT_RUN_LIMIT: i64 = 50;
const MAX_RUN_LIMIT: i64 = 200;

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListBackupRunsArgs {
	/// Restrict to one group's id.
	pub group_id: Option<String>,
	/// Restrict to one machine's id. A run is a box's, so this names the box
	/// rather than any workload on it.
	pub machine_id: Option<String>,
	/// Filter by backup type, e.g. `tamanu-postgres`.
	pub r#type: Option<String>,
	/// Filter by outcome: `success` or `failure`.
	pub outcome: Option<String>,
	/// Only runs reported within this many days.
	pub since_days: Option<u32>,
	/// Max runs to return (default 50, capped at 200).
	pub limit: Option<i64>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ListMaintenanceRunsArgs {
	/// Restrict to one group's id.
	pub group_id: Option<String>,
	/// Filter by maintenance kind: `quick` or `full`.
	pub kind: Option<String>,
	/// Filter by outcome: `success`, `failure`, or `running` (still in flight).
	pub outcome: Option<String>,
	/// Only runs started within this many days.
	pub since_days: Option<u32>,
	/// Max runs to return (default 50, capped at 200).
	pub limit: Option<i64>,
}

#[derive(Serialize)]
struct BackupRunOut {
	id: Uuid,
	group_id: Uuid,
	group_name: Option<String>,
	machine_id: Option<Uuid>,
	machine_name: Option<String>,
	device_id: Uuid,
	r#type: String,
	purpose: String,
	outcome: RunOutcome,
	error: Option<String>,
	bytes_uploaded: Option<i64>,
	snapshot_id: Option<String>,
	reported_at: Timestamp,
	/// S3 traffic tallied by bestool's proxy for this run.
	s3_sent_raw_bytes: Option<i64>,
	s3_sent_payload_bytes: Option<i64>,
	s3_received_raw_bytes: Option<i64>,
	s3_received_payload_bytes: Option<i64>,
	/// Logical size of this run's snapshot, as observed by canopy's own repo
	/// inspection (distinct from `bytes_uploaded`, the device's own figure).
	snapshot_logical_bytes: Option<i64>,
}

#[derive(Serialize)]
struct BackupRunsList {
	count: usize,
	truncated: bool,
	runs: Vec<BackupRunOut>,
}

#[derive(Serialize)]
struct MaintenanceRunOut {
	id: i64,
	group_id: Uuid,
	group_name: Option<String>,
	kind: MaintenanceKind,
	started_at: Timestamp,
	finished_at: Option<Timestamp>,
	/// `None` while the run is still in flight.
	outcome: Option<RunOutcome>,
	error: Option<String>,
	bytes_reclaimed: Option<i64>,
}

#[derive(Serialize)]
struct MaintenanceRunsList {
	count: usize,
	truncated: bool,
	runs: Vec<MaintenanceRunOut>,
}

#[derive(Serialize)]
struct BackupDefaultOut {
	r#type: String,
	/// Seconds between scheduled runs; `null` = manual-only.
	default_interval_seconds: Option<i64>,
	default_retention: Option<RetentionPolicyOut>,
	auto_enable: bool,
	/// Whether this default opts out of the org retention floor (dangerous).
	allow_below_floor: bool,
}

#[derive(Serialize)]
struct RetentionPolicyOut {
	keep_latest: i32,
	keep_daily: i32,
	keep_weekly: i32,
	keep_monthly: i32,
	keep_annual: i32,
}

#[derive(Serialize)]
struct BackupDefaultsList {
	defaults: Vec<BackupDefaultOut>,
}

#[tool_router(router = backups_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "List backup-run history across the fleet (or narrowed by group/server/type/outcome), \
		               newest first. Each run carries its outcome, error (if failed), and its size / S3 \
		               traffic figures. Use this to inspect what actually happened; use \
		               find_backup_problems for the current alerting state."
	)]
	async fn list_backup_runs(
		&self,
		Parameters(args): Parameters<ListBackupRunsArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let group_id = parse_opt_uuid(&args.group_id, "group_id")?;
		let machine_id = parse_opt_uuid(&args.machine_id, "machine_id")?;
		let r#type = args.r#type.as_deref().map(BackupType::from);
		let outcome = parse_opt::<RunOutcome>(&args.outcome, "outcome")?;
		let since = args.since_days.map(since_from_days);
		let limit = args
			.limit
			.unwrap_or(DEFAULT_RUN_LIMIT)
			.clamp(1, MAX_RUN_LIMIT);

		let runs = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				group_id,
				machine_id,
				r#type,
				outcome,
				since,
			},
			limit,
		)
		.await
		.map_err(mcp_err)?;

		let group_names = group_names(&mut conn, &unique(runs.iter().map(|r| r.group_id))).await?;
		// A run names the box it captured, so the name comes from the machine.
		let machine_names: std::collections::HashMap<Uuid, Option<String>> =
			Machine::get_many(&mut conn, &unique(runs.iter().filter_map(|r| r.machine_id)))
				.await
				.map_err(mcp_err)?
				.into_iter()
				.map(|m| (m.id, m.name))
				.collect();

		let count = runs.len();
		let truncated = count as i64 == limit;
		let out = runs
			.into_iter()
			.map(|r| BackupRunOut {
				id: r.id,
				group_id: r.group_id,
				group_name: group_names.get(&r.group_id).cloned(),
				machine_id: r.machine_id,
				machine_name: r
					.machine_id
					.and_then(|m| machine_names.get(&m))
					.cloned()
					.flatten(),
				device_id: r.device_id,
				r#type: r.r#type.to_string(),
				purpose: r.purpose.to_string(),
				outcome: r.outcome,
				error: r.error,
				bytes_uploaded: r.bytes_uploaded,
				snapshot_id: r.snapshot_id,
				reported_at: r.reported_at,
				s3_sent_raw_bytes: r.s3_sent_raw_bytes,
				s3_sent_payload_bytes: r.s3_sent_payload_bytes,
				s3_received_raw_bytes: r.s3_received_raw_bytes,
				s3_received_payload_bytes: r.s3_received_payload_bytes,
				snapshot_logical_bytes: r.snapshot_logical_bytes,
			})
			.collect();

		ok_json(&BackupRunsList {
			count,
			truncated,
			runs: out,
		})
	}

	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "List repo-maintenance run history across the fleet (or narrowed by group/kind/outcome), \
		               newest first: kopia maintenance jobs, distinct from backup runs. Use \
		               outcome=\"running\" to find jobs currently in flight."
	)]
	async fn list_maintenance_runs(
		&self,
		Parameters(args): Parameters<ListMaintenanceRunsArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let group_id = parse_opt_uuid(&args.group_id, "group_id")?;
		let kind = parse_opt::<MaintenanceKind>(&args.kind, "kind")?;
		let outcome = match args.outcome.as_deref() {
			Some("running") => Some(MaintenanceOutcomeFilter::Running),
			Some(s) => Some(MaintenanceOutcomeFilter::Outcome(
				s.parse::<RunOutcome>()
					.map_err(|_| McpError::invalid_params(format!("invalid outcome: {s}"), None))?,
			)),
			None => None,
		};
		let since = args.since_days.map(since_from_days);
		let limit = args
			.limit
			.unwrap_or(DEFAULT_RUN_LIMIT)
			.clamp(1, MAX_RUN_LIMIT);

		let runs = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters {
				group_id,
				kind,
				outcome,
				since,
			},
			limit,
		)
		.await
		.map_err(mcp_err)?;

		let group_names = group_names(&mut conn, &unique(runs.iter().map(|r| r.group_id))).await?;

		let count = runs.len();
		let truncated = count as i64 == limit;
		let out = runs
			.into_iter()
			.map(|r| MaintenanceRunOut {
				id: r.id,
				group_id: r.group_id,
				group_name: group_names.get(&r.group_id).cloned(),
				kind: r.kind,
				started_at: r.started_at,
				finished_at: r.finished_at,
				outcome: r.outcome,
				error: r.error,
				bytes_reclaimed: r.bytes_reclaimed,
			})
			.collect();

		ok_json(&MaintenanceRunsList {
			count,
			truncated,
			runs: out,
		})
	}

	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Canopy-wide default schedule/retention per backup type — what a group inherits for a \
		               type unless it sets its own schedule override (see get_group's `backups.schedules`)."
	)]
	async fn get_backup_defaults(
		&self,
		Parameters(_): Parameters<EmptyArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let rows = BackupTypeDefault::list(&mut conn).await.map_err(mcp_err)?;
		let defaults = rows
			.into_iter()
			.map(|d| BackupDefaultOut {
				r#type: d.r#type.to_string(),
				default_interval_seconds: d.default_interval.map(|pg| pg.0.as_secs()),
				default_retention: RetentionPolicy::from_json(&d.default_retention).map(|r| {
					RetentionPolicyOut {
						keep_latest: r.keep_latest,
						keep_daily: r.keep_daily,
						keep_weekly: r.keep_weekly,
						keep_monthly: r.keep_monthly,
						keep_annual: r.keep_annual,
					}
				}),
				auto_enable: d.auto_enable,
				allow_below_floor: d.allow_below_floor,
			})
			.collect();
		ok_json(&BackupDefaultsList { defaults })
	}
}
