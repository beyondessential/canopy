//! `find_restore_replicas` / `get_restore_replica` tools.

use std::collections::HashMap;

use commons_types::{
	Uuid,
	backup::{IntentDescriptor, RestoreIntent, RunOutcome},
};
use database::{
	devices::Device,
	diesel_async::AsyncPgConnection,
	restore::{BackupRestoreCheck, RestoreConsumerCapability, RestoreReplica},
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
	util::{group_names, mcp_err, not_found, ok_json, parse_opt_uuid, parse_uuid, unique},
};

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FindRestoreReplicasArgs {
	/// Restrict to one group's id.
	pub group_id: Option<String>,
	/// Restrict to one restore consumer device's id.
	pub consumer_device_id: Option<String>,
	/// Only enabled declarations. Defaults to false (all).
	pub enabled_only: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RestoreReplicaIdArgs {
	/// The replica declaration's id.
	pub replica_id: String,
}

#[derive(Serialize)]
struct RestoreReplicaOut {
	id: Uuid,
	consumer_device_id: Uuid,
	consumer_name: Option<String>,
	group_id: Uuid,
	group_name: Option<String>,
	/// `None` = declared against every current machine in the group.
	machine_id: Option<Uuid>,
	machine_name: Option<String>,
	r#type: String,
	intent: String,
	name: String,
	overdue_after_seconds: Option<i64>,
	enabled: bool,
	/// The consumer no longer advertises this intent, so Canopy is not
	/// dispatching this declaration. Mirrors the `gap` flag the operator UI
	/// shows for the same reason (see `restore_replicas::to_views`).
	gap: bool,
	/// Timestamp of the latest healthy restore-verification report for this
	/// exact `(machine, type, intent)`. Only populated for machine-scoped
	/// declarations; a group-wide declaration (`machine_id: null`) covers many
	/// machines so has no single answer here — use `get_restore_replica` or
	/// `find_backup_problems` for a specific machine.
	last_healthy_at: Option<Timestamp>,
	created_at: Timestamp,
	updated_at: Timestamp,
}

#[derive(Serialize)]
struct RestoreReplicaList {
	count: usize,
	replicas: Vec<RestoreReplicaOut>,
}

#[derive(Serialize)]
struct RestoreCheckOut {
	id: i64,
	machine_id: Option<Uuid>,
	machine_name: Option<String>,
	snapshot_id: Option<String>,
	outcome: RunOutcome,
	replica_healthy: bool,
	error: Option<String>,
	postgres_version: Option<String>,
	observed_at: Timestamp,
	reported_at: Timestamp,
	health_details: Option<serde_json::Value>,
}

#[derive(Serialize)]
struct RestoreReplicaDetail {
	#[serde(flatten)]
	replica: RestoreReplicaOut,
	/// The consumer's advertised descriptor for this intent (description,
	/// semantics, parameter schema): `None` when `gap` is true, since the
	/// consumer doesn't currently advertise it.
	intent_descriptor: Option<IntentDescriptor>,
	/// Recent health reports for this replica, newest first.
	recent_checks: Vec<RestoreCheckOut>,
}

#[tool_router(router = restore_router, vis = "pub(crate)")]
impl CanopyMcp {
	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "List managed-restore replica declarations (fleet-wide, or narrowed by group/consumer), \
		               with the consumer's display name and whether the consumer currently advertises the \
		               declared intent (`gap: true` means Canopy is not dispatching it — the declaration is \
		               unsatisfiable until the consumer registers that intent again). Application-scoped \
		               declarations also carry the latest healthy restore-verification timestamp for that \
		               exact (server, type, intent); use get_restore_replica for the recent-checks history \
		               and the consumer's full intent descriptor."
	)]
	async fn find_restore_replicas(
		&self,
		Parameters(args): Parameters<FindRestoreReplicasArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let group = parse_opt_uuid(&args.group_id, "group_id")?;
		let consumer = parse_opt_uuid(&args.consumer_device_id, "consumer_device_id")?;
		let enabled_only = args.enabled_only.unwrap_or(false);

		let mut replicas = match group {
			Some(g) => RestoreReplica::list_for_group(&mut conn, g)
				.await
				.map_err(mcp_err)?,
			None => RestoreReplica::list_all(&mut conn).await.map_err(mcp_err)?,
		};
		replicas.retain(|r| {
			consumer.is_none_or(|c| r.consumer_device_id == c) && (!enabled_only || r.enabled)
		});

		let replicas = self.restore_replica_outs(&mut conn, replicas).await?;
		ok_json(&RestoreReplicaList {
			count: replicas.len(),
			replicas,
		})
	}

	#[tool(
		// Every tool reads; none changes anything (see the SAFE spec).
		annotations(read_only_hint = true),
		description = "Full detail for one managed-restore replica declaration: its config, the consumer's \
		               full descriptor for the intent (parameters, semantics — `None` when the declaration \
		               is a gap), and its recent restore-verification health reports."
	)]
	async fn get_restore_replica(
		&self,
		Parameters(args): Parameters<RestoreReplicaIdArgs>,
	) -> Result<CallToolResult, McpError> {
		let mut conn = self.conn().await?;
		let id = parse_uuid(&args.replica_id, "replica_id")?;
		let Ok(replica) = RestoreReplica::get(&mut conn, id).await else {
			return Ok(not_found(format!("no restore replica with id {id}")));
		};

		let intent_descriptor =
			RestoreConsumerCapability::list_for_consumer(&mut conn, replica.consumer_device_id)
				.await
				.map_err(mcp_err)?
				.into_iter()
				.find(|d| d.intent == replica.intent);

		let relevant: Vec<BackupRestoreCheck> =
			BackupRestoreCheck::list_recent_for_replica(&mut conn, replica.id, 50)
				.await
				.map_err(mcp_err)?;
		let check_machine_names = database::machines::Machine::names_by_ids(
			&mut conn,
			&unique(relevant.iter().filter_map(|c| c.machine_id)),
		)
		.await
		.map_err(mcp_err)?;
		let recent_checks = relevant
			.into_iter()
			.map(|c| RestoreCheckOut {
				id: c.id,
				machine_id: c.machine_id,
				machine_name: c
					.machine_id
					.and_then(|m| check_machine_names.get(&m))
					.cloned()
					.flatten(),
				snapshot_id: c.snapshot_id,
				outcome: c.outcome,
				replica_healthy: c.replica_healthy,
				error: c.error,
				postgres_version: c.postgres_version,
				observed_at: c.observed_at,
				reported_at: c.reported_at,
				health_details: c.health_details,
			})
			.collect();

		let replica = self
			.restore_replica_outs(&mut conn, vec![replica])
			.await?
			.into_iter()
			.next()
			.expect("exactly one replica");

		ok_json(&RestoreReplicaDetail {
			replica,
			intent_descriptor,
			recent_checks,
		})
	}
}

impl CanopyMcp {
	/// Enrich replica rows with consumer/group/server display names, the `gap`
	/// flag (the consumer no longer advertises the declared intent — mirrors
	/// the operator UI's `restore_replicas::to_views`), and, for server-scoped
	/// declarations, the latest healthy restore-verification timestamp for that
	/// exact `(server, type, intent)` (from
	/// `BackupRestoreCheck::latest_healthy_by_key_for_group`). Does not compute
	/// an overdue verdict — that logic lives solely in
	/// `database::restore::sweep_restore_checks`, which alone owns the once-vs-check
	/// semantics distinction.
	async fn restore_replica_outs(
		&self,
		conn: &mut AsyncPgConnection,
		replicas: Vec<RestoreReplica>,
	) -> Result<Vec<RestoreReplicaOut>, McpError> {
		let consumer_ids = unique(replicas.iter().map(|r| r.consumer_device_id));
		let consumer_names = Device::tailscale_names_by_ids(conn, &consumer_ids)
			.await
			.map_err(mcp_err)?;
		let group_ids = unique(replicas.iter().map(|r| r.group_id));
		let g_names = group_names(conn, &group_ids).await?;
		let machine_names = database::machines::Machine::names_by_ids(
			conn,
			&unique(replicas.iter().filter_map(|r| r.machine_id)),
		)
		.await
		.map_err(mcp_err)?;

		let mut caps: HashMap<Uuid, std::collections::HashSet<RestoreIntent>> = HashMap::new();
		for id in &consumer_ids {
			let set = RestoreConsumerCapability::list_for_consumer(conn, *id)
				.await
				.map_err(mcp_err)?
				.into_iter()
				.map(|d| d.intent)
				.collect();
			caps.insert(*id, set);
		}

		let mut healthy_by_group: HashMap<Uuid, HashMap<database::restore::ReplicaKey, Timestamp>> =
			HashMap::new();
		for gid in &group_ids {
			let map = BackupRestoreCheck::latest_healthy_by_key_for_group(conn, *gid)
				.await
				.map_err(mcp_err)?;
			healthy_by_group.insert(*gid, map);
		}

		Ok(replicas
			.into_iter()
			.map(|r| {
				let gap = !caps
					.get(&r.consumer_device_id)
					.is_some_and(|s| s.contains(&r.intent));
				let last_healthy_at = r.machine_id.and_then(|mid| {
					healthy_by_group
						.get(&r.group_id)
						.and_then(|m| {
							m.get(&(
								mid,
								r.r#type.clone(),
								r.intent.clone(),
								Some(r.name.clone()),
							))
						})
						.copied()
				});
				RestoreReplicaOut {
					id: r.id,
					consumer_device_id: r.consumer_device_id,
					consumer_name: consumer_names.get(&r.consumer_device_id).cloned(),
					group_id: r.group_id,
					group_name: g_names.get(&r.group_id).cloned(),
					machine_id: r.machine_id,
					machine_name: r
						.machine_id
						.and_then(|m| machine_names.get(&m))
						.cloned()
						.flatten(),
					r#type: r.r#type.to_string(),
					intent: r.intent.to_string(),
					name: r.name,
					overdue_after_seconds: r.overdue_after.map(|f| f.0.as_secs()),
					enabled: r.enabled,
					gap,
					last_healthy_at,
					created_at: r.created_at,
					updated_at: r.updated_at,
				}
			})
			.collect())
	}
}
