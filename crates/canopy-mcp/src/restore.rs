//! `find_restore_replicas` / `get_restore_replica` tools, and the two that
//! declare and change a replica.

use std::collections::HashMap;

use commons_errors::AppError;
use commons_types::{
	Uuid,
	backup::{BackupType, IntentDescriptor, ParamValues, RestoreIntent, RunOutcome},
};
use database::{
	audit::{self, Action},
	devices::Device,
	diesel_async::{AsyncConnection, AsyncPgConnection},
	restore::{
		self, BackupRestoreCheck, NewRestoreReplica, RestoreConsumerCapability, RestoreReplica,
		RestoreReplicaUpdate,
	},
};
use jiff::Timestamp;
use rmcp::{
	handler::server::{tool::Extension, wrapper::Parameters},
	model::{CallToolResult, ErrorData as McpError},
	tool, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::{
	CanopyMcp,
	caller::{self, Caller},
	util::{
		group_names, is_not_found, mcp_err, not_found, ok_json, parse_opt_uuid, parse_uuid, unique,
	},
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct DeclareRestoreReplicaArgs {
	/// The restore consumer device that will maintain the replica.
	pub consumer_device_id: String,
	/// The group whose backups are restored.
	pub group_id: String,
	/// One machine of the group to restore. Omit to cover every machine.
	pub machine_id: Option<String>,
	/// The backup type restored, e.g. `tamanu-postgres`.
	pub r#type: String,
	/// The intent, one the consumer advertises, e.g. `analytics`.
	pub intent: String,
	/// A name unique among the consumer's declarations.
	pub name: String,
	/// How long without a healthy restore before it is overdue, e.g. `36h`.
	pub overdue_after: Option<String>,
	/// Values for the intent's parameters, by parameter name.
	pub params: Option<ParamValues>,
	/// Serve the replica de-identified. Defaults to false.
	pub redacts: Option<bool>,
	/// Let the consumer publish the reporting schema it builds. Defaults to
	/// false.
	pub publishes_schemas: Option<bool>,
	/// Test migrations only when an operator asks. Defaults to false.
	pub migrates_on_request: Option<bool>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct UpdateRestoreReplicaArgs {
	/// The replica declaration's id.
	pub replica_id: String,
	/// A new name.
	pub name: Option<String>,
	/// Whether the declaration is active.
	pub enabled: Option<bool>,
	/// Scope the declaration to this machine of its group.
	pub machine_id: Option<String>,
	/// Set true to cover every machine of the group. Not with `machine_id`.
	pub whole_group: Option<bool>,
	/// A new overdue bound, e.g. `36h`; an empty string clears it.
	pub overdue_after: Option<String>,
	/// The full set of parameter values, replacing those held.
	pub params: Option<ParamValues>,
	pub redacts: Option<bool>,
	pub publishes_schemas: Option<bool>,
	pub migrates_on_request: Option<bool>,
}

/// An administrator on the operator surface, or a refusal saying so.
async fn require_declaring(parts: Extension<http::request::Parts>) -> Result<Caller, McpError> {
	let caller = caller::of(parts)?;
	if !caller.may_declare_replicas().await.map_err(mcp_err)? {
		return Err(McpError::invalid_request(
			"declaring or changing a restore replica needs an administrator on the operator surface",
			None,
		));
	}
	Ok(caller)
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
					.cloned(),
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

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = false
		),
		description = "Declare a restore replica: ask a restore consumer to maintain a replica of a \
		               group's backups, or of one machine's, for an intent it advertises. Validated \
		               and audited as a declaration made in the operator interface is. Returns the \
		               declaration. Refused, with no effect, unless the caller is an administrator \
		               on the operator surface; no access token may declare one."
	)]
	async fn declare_restore_replica(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<DeclareRestoreReplicaArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_declaring(parts).await?;
		let consumer_device_id = parse_uuid(&args.consumer_device_id, "consumer_device_id")?;
		let group_id = parse_uuid(&args.group_id, "group_id")?;
		let machine_id = parse_opt_uuid(&args.machine_id, "machine_id")?;
		let new = RestoreReplicaUpdate {
			consumer_device_id,
			group_id,
			machine_id,
			r#type: BackupType::from(args.r#type.as_str()),
			intent: RestoreIntent::from(args.intent.as_str()),
			name: args.name,
			overdue_after: None,
			params: serde_json::Value::Null,
			redacts: args.redacts.unwrap_or(false),
			publishes_schemas: args.publishes_schemas.unwrap_or(false),
			migrates_on_request: args.migrates_on_request.unwrap_or(false),
			enabled: true,
		};
		self.write_replica(
			&caller,
			None,
			new,
			args.overdue_after,
			args.params.unwrap_or_default(),
		)
		.await
	}

	#[tool(
		annotations(
			read_only_hint = false,
			destructive_hint = false,
			idempotent_hint = true
		),
		description = "Change a restore replica declaration: its name, whether it is enabled, its \
		               scope within its group (one machine or the whole group), its overdue bound, \
		               its parameter values and its flags. Anything left out keeps its value. \
		               Validated and audited as a change made in the operator interface is. Returns \
		               the declaration. Refused, with no effect, unless the caller is an \
		               administrator on the operator surface; no access token may change one."
	)]
	async fn update_restore_replica(
		&self,
		parts: Extension<http::request::Parts>,
		Parameters(args): Parameters<UpdateRestoreReplicaArgs>,
	) -> Result<CallToolResult, McpError> {
		let caller = require_declaring(parts).await?;
		let id = parse_uuid(&args.replica_id, "replica_id")?;
		let machine_id = parse_opt_uuid(&args.machine_id, "machine_id")?;
		let whole_group = args.whole_group.unwrap_or(false);
		if whole_group && machine_id.is_some() {
			return Err(McpError::invalid_params(
				"give machine_id or whole_group, not both",
				None,
			));
		}

		let mut conn = self.conn().await?;
		let held = match RestoreReplica::get(&mut conn, id).await {
			Ok(held) => held,
			Err(error) if is_not_found(&error) => {
				return Ok(not_found(format!("no restore replica with id {id}")));
			}
			Err(error) => return Err(mcp_err(error)),
		};
		drop(conn);

		let params = match args.params {
			Some(params) => params,
			None => serde_json::from_value(held.params.clone()).unwrap_or_default(),
		};
		let overdue_after = match args.overdue_after {
			Some(text) => Some(text),
			None => held
				.overdue_after
				.map(|bound| format!("{}s", bound.0.as_secs())),
		};
		let new = RestoreReplicaUpdate {
			consumer_device_id: held.consumer_device_id,
			group_id: held.group_id,
			machine_id: if whole_group {
				None
			} else {
				machine_id.or(held.machine_id)
			},
			r#type: held.r#type,
			intent: held.intent,
			name: args.name.unwrap_or(held.name),
			overdue_after: None,
			params: serde_json::Value::Null,
			redacts: args.redacts.unwrap_or(held.redacts),
			publishes_schemas: args.publishes_schemas.unwrap_or(held.publishes_schemas),
			migrates_on_request: args.migrates_on_request.unwrap_or(held.migrates_on_request),
			enabled: args.enabled.unwrap_or(held.enabled),
		};
		self.write_replica(&caller, Some(id), new, overdue_after, params)
			.await
	}
}

impl CanopyMcp {
	/// Validate and store a declaration, new where `id` is `None`, and record
	/// it as the caller's.
	async fn write_replica(
		&self,
		caller: &Caller,
		id: Option<Uuid>,
		mut new: RestoreReplicaUpdate,
		overdue_after: Option<String>,
		params: ParamValues,
	) -> Result<CallToolResult, McpError> {
		let actor = caller.actor();
		let created_by = caller.name().to_owned();
		let mut conn = self.db_write.get().await.map_err(mcp_err)?;
		let written = conn
			.transaction::<_, AppError, _>(async |conn| {
				let params = restore::declared_params(
					conn,
					new.consumer_device_id,
					&new.intent,
					&params,
					new.redacts,
					new.publishes_schemas,
					new.group_id,
					new.machine_id,
				)
				.await?;
				new.params = serde_json::to_value(&params).expect("params serialize");
				new.overdue_after = restore::parse_overdue_after(overdue_after.as_deref())?;

				let replica = match id {
					Some(id) => RestoreReplica::update(conn, id, new).await?,
					None => {
						RestoreReplica::create(
							conn,
							NewRestoreReplica {
								consumer_device_id: new.consumer_device_id,
								group_id: new.group_id,
								machine_id: new.machine_id,
								r#type: new.r#type,
								intent: new.intent,
								name: new.name,
								overdue_after: new.overdue_after,
								params: new.params,
								redacts: new.redacts,
								publishes_schemas: new.publishes_schemas,
								migrates_on_request: new.migrates_on_request,
								created_by: Some(created_by),
							},
						)
						.await?
					}
				};

				let (record, group, machine, consumer) =
					restore::audit_records(conn, &replica).await?;
				let backup_type = replica.r#type.to_string();
				let intent = replica.intent.to_string();
				let action = match id {
					Some(_) => Action::RestoreReplicaUpdated {
						replica: record,
						group,
						machine,
						consumer,
						backup_type,
						intent,
						enabled: replica.enabled,
						redacts: replica.redacts,
						publishes_schemas: replica.publishes_schemas,
					},
					None => Action::RestoreReplicaDeclared {
						replica: record,
						group,
						machine,
						consumer,
						backup_type,
						intent,
						enabled: replica.enabled,
						redacts: replica.redacts,
						publishes_schemas: replica.publishes_schemas,
					},
				};
				audit::record(conn, &actor, &action).await?;
				Ok(replica)
			})
			.await;

		match written {
			Ok(replica) => {
				let mut conn = self.conn().await?;
				let out = self.restore_replica_outs(&mut conn, vec![replica]).await?;
				ok_json(&out.into_iter().next().expect("one declaration"))
			}
			Err(AppError::BadRequest(why) | AppError::Conflict(why)) => Ok(not_found(why)),
			Err(error) if is_not_found(&error) => Ok(not_found(
				"no such declaration, group, machine or consumer".into(),
			)),
			Err(error) => Err(mcp_err(error)),
		}
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
					machine_name: r.machine_id.and_then(|m| machine_names.get(&m)).cloned(),
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
