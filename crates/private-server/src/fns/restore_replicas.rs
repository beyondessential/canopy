//! Operator-facing managed-restore endpoints (private-server, admin SPA).
//!
//! Thin wrappers over `database::restore`. Operators declare which replicas a
//! restore consumer should maintain, and see each consumer's registered
//! capabilities so the declaration UX can offer only supported intents and flag
//! declarations whose intent is currently unsupported (a *gap*).
//!
//! Reads are open to any tailnet user; mutations require admin.

use std::collections::{HashMap, HashSet};

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::device::DeviceRole;
use commons_types::{
	Uuid,
	backup::{
		BackupPurpose, BackupType, IntentDescriptor, ParamValues, RedactionOutcome, RestoreIntent,
		RunOutcome, display_param_defaults, display_params, normalize_params, redaction_params,
		semantics, validate_params,
	},
	units,
};
use database::diesel_async::AsyncPgConnection;
use database::pg_duration::PgDuration;
use database::restore::{self, RedactionGapReason};
use database::{
	BackupRestoreCheck, NewRestoreReplica, RestoreConsumerCapability, RestoreReplica,
	RestoreReplicaUpdate, backups::BackupCredentialIssuance, devices::Device, machines::Machine,
};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::run_pairing::{self, ReportRef, RunStatus};
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(for_group))
		.routes(routes!(consumers))
		.routes(routes!(checks))
		.routes(routes!(create))
		.routes(routes!(update))
		.routes(routes!(delete))
}

// ── Wire types ──────────────────────────────────────────────────────────────

/// A managed-restore declaration, as shown to operators.
///
/// A declaration instructs a restore consumer to maintain a restored replica
/// of a backup, for a given purpose (intent). It also grants the consumer
/// read access to the covered backups while it is enabled.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RestoreReplicaView {
	/// Unique identifier of the declaration.
	pub id: Uuid,
	/// Identifier of the restore consumer device the declaration is assigned to.
	pub consumer_device_id: Uuid,
	/// Display name of the consumer device, if known.
	pub consumer_name: Option<String>,
	/// Identifier of the server group whose backups the declaration covers.
	pub group_id: Uuid,
	/// Specific machine within the group, or null to cover all current machines
	/// in the group.
	pub machine_id: Option<Uuid>,
	/// The backup type to restore, for example `tamanu-postgres`.
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// How the replica is handled, as defined by the consumer: an arbitrary
	/// identifier from the consumer's advertised intents, e.g. `verify`.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// Operator-chosen display name for the declaration.
	pub name: String,
	/// Overdue bound as a human-friendly duration (e.g. `2h 30m` or `1d`):
	/// how long the replica may go without a healthy restore report (or, for
	/// at-most-once intents, how long the latest snapshot may go unverified)
	/// before it is considered overdue. Null means no bound. The same format
	/// is accepted back by `create` and `update`.
	pub overdue_after: Option<String>,
	/// Operator-supplied parameter values (name → value). Values of
	/// `duration` and `bytes` parameters are formatted as human-friendly
	/// strings (e.g. `2h 30m`, `20Gi`) when the intent's schema is known;
	/// `create` and `update` accept these strings back.
	#[schema(value_type = Object)]
	pub params: serde_json::Value,
	/// Whether the replica is served de-identified.
	pub redacts: bool,
	/// True when the intent carries the `redact` semantic, so the declaration
	/// can be switched to redacting.
	pub can_redact: bool,
	/// Whether this declaration's consumer may publish the group's reporting
	/// schema. Only an operator sets it.
	pub publishes_schemas: bool,
	/// True when the intent carries the `reporting-schema` semantic, so the
	/// declaration can be made the group's publisher.
	pub can_publish_schemas: bool,
	/// Servers this declaration covers that cannot currently be redacted:
	/// either their product publishes no masking manifest, or the version
	/// they report has none published. Each is withheld from the worklist
	/// rather than restored unmasked. Empty unless the declaration redacts.
	pub redaction_gaps: Vec<RedactionGap>,
	/// Whether the declaration is active. Disabled declarations are not
	/// dispatched to the consumer and grant no backup access.
	pub enabled: bool,
	/// True when the consumer does not currently advertise this declaration's
	/// intent, so the declaration is not being dispatched.
	pub gap: bool,
	/// Login of the operator who created the declaration, if recorded.
	pub created_by: Option<String>,
	/// When the declaration was created.
	#[schema(value_type = String)]
	pub created_at: Timestamp,
	/// When the declaration was last modified.
	#[schema(value_type = String)]
	pub updated_at: Timestamp,
}

/// One server a redacting declaration covers but cannot redact.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RedactionGap {
	/// The server that would be restored unmasked, and so isn't restored.
	pub server_id: Uuid,
	/// Its display name, when known.
	pub server_name: Option<String>,
	/// Why it can't be redacted.
	pub reason: RedactionGapReason,
	/// The version it reports, when the reason concerns one.
	pub version: Option<String>,
}

/// A restore consumer and the restore intents it currently advertises.
///
/// A restore consumer is a device with the `backup-restore` role: an agent
/// that restores backups onto standby replicas. Each advertised intent
/// carries a description, semantics flags, and a parameter schema, which
/// together determine what declarations can be created for the consumer and
/// what parameters they accept.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RestoreConsumerView {
	/// Identifier of the consumer device.
	pub device_id: Uuid,
	/// Display name of the consumer device, if known.
	pub name: Option<String>,
	/// The intents the consumer currently advertises support for.
	pub intents: Vec<IntentDescriptor>,
}

/// Scopes a request to one server group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestoreReplicasGroupArgs {
	/// Identifier of the server group.
	pub server_group_id: Uuid,
}

/// Request to declare a new managed restore replica.
///
/// The consumer, group, server, backup type, and intent define the
/// declaration's scope; all of it can be changed later via `update`.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestoreReplicasCreateArgs {
	/// Identifier of the restore consumer device to assign the declaration to.
	pub consumer_device_id: Uuid,
	/// Identifier of the server group whose backups to restore.
	pub group_id: Uuid,
	/// Specific machine within the group; omit or null to cover all current
	/// machines in the group.
	pub machine_id: Option<Uuid>,
	/// The backup type to restore, for example `tamanu-postgres`.
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// How the replica is handled, as defined by the consumer: an arbitrary
	/// identifier from the consumer's advertised intents, e.g. `verify`.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// Display name for the declaration, unique among the consumer's
	/// declarations.
	pub name: String,
	/// Overdue bound as a human-friendly duration (jiff's "friendly" format,
	/// e.g. `2h 30m`, `36h`, `1d 12h`); omit, null, or blank for no bound.
	pub overdue_after: Option<String>,
	/// Parameter values for the intent (name → value), validated against the
	/// consumer's advertised parameter schema. `duration` and `bytes`
	/// parameters accept human-unit strings (e.g. `2h 30m`, `20Gi`) as well
	/// as raw integer seconds/bytes. Defaults to empty.
	#[serde(default)]
	#[schema(value_type = Object)]
	pub params: ParamValues,
	/// Whether the replica is served de-identified. Accepted only for an
	/// intent carrying the `redact` semantic; Canopy resolves the masking
	/// manifest itself from the server's product, so there is nothing else
	/// to set. Defaults to false.
	#[serde(default)]
	pub redacts: bool,
	/// Whether this consumer may publish the group's reporting schema.
	/// Accepted only for a group-wide, non-redacting declaration whose intent
	/// carries the `reporting-schema` semantic. Defaults to false, so a
	/// consumer publishes only where an operator has said it may.
	#[serde(default)]
	pub publishes_schemas: bool,
}

/// Request to update an existing declaration.
///
/// Replaces every field, including scope: the consumer, group, server,
/// backup type, and intent can all be changed in the same call as the name,
/// overdue bound, parameter values, and enabled flag. Only a name already used
/// by another of the consumer's declarations maps to `409`; the scope may
/// match another declaration's.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestoreReplicasUpdateArgs {
	/// Identifier of the declaration to update.
	pub id: Uuid,
	/// Identifier of the restore consumer device to assign the declaration to.
	pub consumer_device_id: Uuid,
	/// Identifier of the server group whose backups to restore.
	pub group_id: Uuid,
	/// Specific machine within the group; omit or null to cover all current
	/// machines in the group.
	pub machine_id: Option<Uuid>,
	/// The backup type to restore, for example `tamanu-postgres`.
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// How the replica is handled, as defined by the consumer: an arbitrary
	/// identifier from the consumer's advertised intents, e.g. `verify`.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// New display name for the declaration, unique among the consumer's
	/// declarations.
	pub name: String,
	/// New overdue bound as a human-friendly duration (jiff's "friendly"
	/// format, e.g. `2h 30m`, `36h`, `1d 12h`); null or blank removes the
	/// bound.
	pub overdue_after: Option<String>,
	/// New parameter values (name → value), validated against the intent's
	/// advertised parameter schema. `duration` and `bytes` parameters accept
	/// human-unit strings (e.g. `2h 30m`, `20Gi`) as well as raw integer
	/// seconds/bytes. Defaults to empty.
	#[serde(default)]
	#[schema(value_type = Object)]
	pub params: ParamValues,
	/// Whether the replica is served de-identified. Accepted only for an
	/// intent carrying the `redact` semantic. Defaults to false.
	#[serde(default)]
	pub redacts: bool,
	/// Whether this consumer may publish the group's reporting schema.
	/// Accepted only for a group-wide, non-redacting declaration whose intent
	/// carries the `reporting-schema` semantic. Defaults to false.
	#[serde(default)]
	pub publishes_schemas: bool,
	/// Whether the declaration should be active.
	pub enabled: bool,
}

/// Identifies the declaration to operate on.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IdArgs {
	/// Identifier of the declaration.
	pub id: Uuid,
}

// ── Helpers ───────────────────────────────────────────────────────────────

/// Parse an operator-supplied overdue bound (jiff's "friendly" duration
/// format, e.g. `2h 30m`) into the stored form; null or blank means no bound.
/// A string that doesn't parse maps to 400.
fn overdue_after_to_pg(overdue_after: Option<&str>) -> Result<Option<PgDuration>> {
	let Some(text) = overdue_after.map(str::trim).filter(|t| !t.is_empty()) else {
		return Ok(None);
	};
	let seconds = units::parse_duration_seconds(text)
		.map_err(|e| AppError::BadRequest(format!("overdue bound: {e}")))?;
	Ok(Some(PgDuration(SignedDuration::from_secs(seconds))))
}

/// Resolve human-unit strings in operator-supplied parameter values to their
/// raw stored form, validate them against the consumer's advertised schema for
/// `intent`, and refuse a flag the declaration cannot carry. If the intent is
/// not advertised (a gap) there is no schema to resolve or check against, so
/// the values are accepted as-is.
async fn normalized_params_for_intent(
	conn: &mut AsyncPgConnection,
	consumer_device_id: Uuid,
	intent: &RestoreIntent,
	params: &ParamValues,
	redacts: bool,
	publishes_schemas: bool,
	machine_id: Option<Uuid>,
) -> Result<ParamValues> {
	let descriptors =
		RestoreConsumerCapability::list_for_consumer(conn, consumer_device_id).await?;
	let Some(desc) = descriptors.iter().find(|d| &d.intent == intent) else {
		return Ok(params.clone());
	};

	// The masking parameters are Canopy's for any intent carrying `redact`,
	// whether or not this declaration redacts. An operator value for one is
	// dropped rather than stored: were it kept, a declaration could redact
	// with its flag off and the flag would stop answering on its own whether
	// an unmasked replica is a finding.
	let owns_masking = desc.has_semantic(semantics::REDACT);
	if redacts && !owns_masking {
		return Err(AppError::BadRequest(format!(
			"intent {intent} cannot redact: it does not carry the `redact` semantic"
		)));
	}

	// A schema is built per group from its canonical central, from data the
	// masking manifest has not altered, so a declaration Canopy would never
	// dispatch a build to cannot be the group's publisher either.
	// spec: RPT#the-build-contract
	if publishes_schemas {
		if !desc.has_semantic(semantics::REPORTING_SCHEMA) {
			return Err(AppError::BadRequest(format!(
				"intent {intent} cannot publish a reporting schema: it does not carry the `reporting-schema` semantic"
			)));
		}
		if redacts {
			return Err(AppError::BadRequest(
				"a redacting declaration cannot publish a reporting schema".into(),
			));
		}
		if machine_id.is_some() {
			return Err(AppError::BadRequest(
				"a machine-scoped declaration cannot publish a reporting schema: a build is per group".into(),
			));
		}
	}
	let params = if owns_masking {
		&params
			.iter()
			.filter(|(name, _)| !redaction_params::ALL.contains(&name.as_str()))
			.map(|(name, value)| (name.clone(), value.clone()))
			.collect()
	} else {
		params
	};

	let normalized =
		normalize_params(&desc.params, params).map_err(|e| AppError::BadRequest(e.to_string()))?;
	validate_params(&desc.params, &normalized).map_err(|e| AppError::BadRequest(e.to_string()))?;
	Ok(normalized)
}

/// The applications a redacting declaration covers that can't be redacted, so an
/// operator sees which of its replicas are being withheld and why.
// spec: RST#the-masking-manifest
async fn redaction_gaps_for(
	conn: &mut AsyncPgConnection,
	replica: &RestoreReplica,
) -> Result<Vec<RedactionGap>> {
	// The declaration covers machines; a gap is a property of the product in the
	// snapshot, so it is reported per application on those machines.
	let machines = match replica.machine_id {
		Some(mid) => database::machines::Machine::get_by_id(conn, mid)
			.await
			.ok()
			.into_iter()
			.collect(),
		None => database::machines::Machine::list_for_group(conn, replica.group_id).await?,
	};
	let mut applications = Vec::new();
	for machine in machines {
		applications.extend(machine.applications(conn).await?);
	}

	let mut gaps = Vec::new();
	for server in applications {
		if let Some((reason, version)) = restore::redaction_gap_for(conn, &server).await? {
			gaps.push(RedactionGap {
				server_id: server.id,
				server_name: Some(server.display_name()),
				reason,
				version,
			});
		}
	}
	Ok(gaps)
}

/// Build views from declarations, resolving consumer display names and the
/// per-consumer capabilities so `gap` can be computed and stored `duration`/
/// `bytes` parameter values can be shown as human-unit strings.
async fn to_views(
	conn: &mut AsyncPgConnection,
	replicas: Vec<RestoreReplica>,
) -> Result<Vec<RestoreReplicaView>> {
	let consumer_ids: HashSet<Uuid> = replicas.iter().map(|r| r.consumer_device_id).collect();

	// Consumer display names come from the set of restore-consumer devices.
	let names: HashMap<Uuid, Option<String>> =
		Device::list_by_role(conn, DeviceRole::BackupRestore)
			.await?
			.into_iter()
			.map(|d| (d.id, d.tailscale_node_name))
			.collect();

	let mut caps: HashMap<Uuid, Vec<IntentDescriptor>> = HashMap::new();
	for id in consumer_ids {
		let descriptors = RestoreConsumerCapability::list_for_consumer(conn, id).await?;
		caps.insert(id, descriptors);
	}

	// Only a redacting declaration can have a redaction gap, and resolving
	// one walks the declaration's applications, so this is keyed by declaration
	// and computed only for those that redact.
	let mut gaps: HashMap<Uuid, Vec<RedactionGap>> = HashMap::new();
	for r in replicas.iter().filter(|r| r.redacts) {
		gaps.insert(r.id, redaction_gaps_for(conn, r).await?);
	}

	Ok(replicas
		.into_iter()
		.map(|r| {
			let schema = caps
				.get(&r.consumer_device_id)
				.and_then(|descs| descs.iter().find(|d| d.intent == r.intent))
				.map(|d| &d.params);
			// With no schema (a gap) the stored values pass through raw.
			let params = match (schema, r.params) {
				(Some(schema), serde_json::Value::Object(map)) => {
					let values: ParamValues = map.into_iter().collect();
					serde_json::to_value(display_params(schema, &values)).expect("params serialize")
				}
				(_, params) => params,
			};
			RestoreReplicaView {
				gap: schema.is_none(),
				can_redact: caps
					.get(&r.consumer_device_id)
					.and_then(|descs| descs.iter().find(|d| d.intent == r.intent))
					.is_some_and(|d| d.has_semantic(semantics::REDACT)),
				can_publish_schemas: caps
					.get(&r.consumer_device_id)
					.and_then(|descs| descs.iter().find(|d| d.intent == r.intent))
					.is_some_and(|d| d.has_semantic(semantics::REPORTING_SCHEMA)),
				publishes_schemas: r.publishes_schemas,
				redacts: r.redacts,
				redaction_gaps: gaps.remove(&r.id).unwrap_or_default(),
				consumer_name: names.get(&r.consumer_device_id).cloned().flatten(),
				overdue_after: r
					.overdue_after
					.map(|f| units::format_duration_seconds(f.0.as_secs())),
				params,
				id: r.id,
				consumer_device_id: r.consumer_device_id,
				group_id: r.group_id,
				machine_id: r.machine_id,
				r#type: r.r#type,
				intent: r.intent,
				name: r.name,
				enabled: r.enabled,
				created_by: r.created_by,
				created_at: r.created_at,
				updated_at: r.updated_at,
			}
		})
		.collect())
}

// ── Handlers ──────────────────────────────────────────────────────────────

/// List restore replica declarations for a group.
///
/// Returns every declaration scoped to the given server group, with each
/// consumer's display name resolved and the `gap` flag computed against the
/// intents the consumer currently advertises.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "restore_replicas_for_group",
	tag = "restore_replicas",
	security(("tailscale-user" = [])),
	request_body = RestoreReplicasGroupArgs,
	responses((status = 200, body = Vec<RestoreReplicaView>)),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<RestoreReplicasGroupArgs>,
) -> Result<Json<Vec<RestoreReplicaView>>> {
	let mut conn = state.db_read.get().await?;
	let replicas = RestoreReplica::list_for_group(&mut conn, args.server_group_id).await?;
	Ok(Json(to_views(&mut conn, replicas).await?))
}

/// List restore consumers and their advertised intents.
///
/// Returns every device with the backup-restore role, together with the
/// restore intents it currently advertises (each with its description,
/// semantics flags, and parameter schema). Use this to discover which
/// consumers and intents a declaration can target and which parameters each
/// intent accepts. Defaults of `duration` and `bytes` parameters are
/// formatted as human-unit strings (e.g. `2h`, `20Gi`).
#[utoipa::path(
	post,
	path = "/consumers",
	operation_id = "restore_replicas_consumers",
	tag = "restore_replicas",
	security(("tailscale-user" = [])),
	responses((status = 200, body = Vec<RestoreConsumerView>)),
)]
pub async fn consumers(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<Vec<RestoreConsumerView>>> {
	let mut conn = state.db_read.get().await?;
	let devices = Device::list_by_role(&mut conn, DeviceRole::BackupRestore).await?;
	let mut out = Vec::with_capacity(devices.len());
	for d in devices {
		let intents = RestoreConsumerCapability::list_for_consumer(&mut conn, d.id)
			.await?
			.into_iter()
			// `duration`/`bytes` defaults are shown to operators, so format
			// them as human-unit strings like the declaration values.
			.map(|mut desc| {
				desc.params = display_param_defaults(&desc.params);
				desc
			})
			.collect();
		out.push(RestoreConsumerView {
			device_id: d.id,
			name: d.tailscale_node_name,
			intents,
		});
	}
	Ok(Json(out))
}

/// The cap on the recent restore-activity list.
const RECENT_CHECKS_LIMIT: i64 = 50;

/// One row of the restore-activity view: either a consumer-reported restore
/// health check, or a restore inferred from a credential issuance that never
/// reported (in flight, or terminated without a report). Mirrors the backup
/// recent-runs view on the restore side.
#[derive(Debug, Serialize, ToSchema)]
pub struct RestoreActivity {
	/// Stable identity for UI keying: `check-<id>` for a reported check, or
	/// `issuance-<id>` for an inferred restore.
	pub key: String,
	/// Reported, in-flight, or an unreported terminated restore.
	pub status: RunStatus,
	/// The machine the restore is for, when reported. Absent for inferred rows
	/// (the issuance is minted per group+type, not per machine).
	pub machine_id: Option<Uuid>,
	/// The backup type restored.
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// The restore intent, when reported. Absent for inferred rows.
	#[schema(value_type = Option<String>)]
	pub intent: Option<RestoreIntent>,
	/// The restore outcome; only present for a reported check.
	#[schema(value_type = Option<String>)]
	pub outcome: Option<RunOutcome>,
	/// Whether the restored replica came up healthy; only for a reported check.
	pub replica_healthy: Option<bool>,
	/// Error detail for a failed reported restore, if any.
	pub error: Option<String>,
	/// PostgreSQL version of the restored database, if reported.
	pub postgres_version: Option<String>,
	/// The snapshot that was restored, if reported.
	pub snapshot_id: Option<String>,
	/// Consumer-supplied health data, if any (reported checks only).
	pub health_details: Option<serde_json::Value>,
	/// How far the replica's masking manifest got, for a replica that
	/// redacts: `complete`, `partial`, or `failed`.
	#[schema(value_type = Option<String>)]
	pub redaction_outcome: Option<RedactionOutcome>,
	/// The version whose manifest was fetched.
	pub redaction_manifest_version: Option<String>,
	/// How many columns the manifest masked.
	pub redaction_columns_masked: Option<i64>,
	/// How many columns it named but could not mask.
	pub redaction_columns_skipped: Option<i64>,
	/// Why the redaction failed, when it did.
	pub redaction_error: Option<String>,
	/// Raw bytes sent to S3 during the restore, if reported.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Payload bytes sent to S3 during the restore, if reported.
	pub s3_sent_payload_bytes: Option<i64>,
	/// Raw bytes received from S3 during the restore, if reported.
	pub s3_received_raw_bytes: Option<i64>,
	/// Payload bytes received from S3 during the restore, if reported.
	pub s3_received_payload_bytes: Option<i64>,
	/// When the consumer observed the restore result (client-reported); reported
	/// checks only.
	pub observed_at: Option<Timestamp>,
	/// When the restore started, taken from its matching credential issuance.
	/// `None` when no issuance could be matched.
	pub started_at: Option<Timestamp>,
	/// When the consumer's report was received. `None` for an inferred row.
	pub reported_at: Option<Timestamp>,
	/// Effective time for sorting/display: the report's observed time when
	/// reported, otherwise the issuance start.
	pub at: Timestamp,
	/// Canopy-measured restore duration in seconds (report received time minus
	/// first-issuance time). `None` for an in-flight/unreported row or a check
	/// with no matching issuance.
	#[schema(value_type = Option<i64>, format = "int64")]
	pub duration_seconds: Option<i64>,
}

/// List recent restore activity for a group.
///
/// Returns up to the 50 most recent restore-health reports submitted by
/// consumers for the given server group, plus restores inferred from credential
/// issuances that never reported (in flight, or terminated without a report).
/// Each reported check records whether a backup snapshot restored successfully
/// and whether the resulting replica was healthy — the strongest available
/// signal that the group's backups are actually restorable. Reported checks
/// carry a Canopy-measured duration (issuance → report).
#[utoipa::path(
	post,
	path = "/checks",
	operation_id = "restore_replicas_checks",
	tag = "restore_replicas",
	security(("tailscale-user" = [])),
	request_body = RestoreReplicasGroupArgs,
	responses((status = 200, body = Vec<RestoreActivity>)),
)]
pub async fn checks(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<RestoreReplicasGroupArgs>,
) -> Result<Json<Vec<RestoreActivity>>> {
	let mut conn = state.db_read.get().await?;
	let group_id = args.server_group_id;
	let now = Timestamp::now();

	let checks =
		BackupRestoreCheck::list_recent_for_group(&mut conn, group_id, RECENT_CHECKS_LIMIT).await?;

	// Member-box devices run *manual* restores, tracked in the backup panel;
	// this table is for restore *consumers*, so their issuances are the ones we
	// pair here (the complement of the backup panel's member-device filter). An
	// identity belongs to a box, so the members are the group's machines.
	let member_devices: HashSet<Uuid> = Machine::list_for_group(&mut conn, group_id)
		.await?
		.into_iter()
		.filter_map(|m| m.device_id)
		.collect();

	let issuance_since =
		run_pairing::issuance_since(now, checks.iter().map(|c| c.reported_at).min());
	let issuances: Vec<_> = BackupCredentialIssuance::list_for_group_since(
		&mut conn,
		group_id,
		issuance_since,
		RECENT_CHECKS_LIMIT * 4,
	)
	.await?
	.into_iter()
	.filter(|i| i.purpose == BackupPurpose::Restore && !member_devices.contains(&i.device_id))
	.collect();

	// `list_recent_for_group` is newest-first, which the guesstimate fallback
	// relies on.
	let reports: Vec<ReportRef> = checks
		.iter()
		.map(|c| ReportRef {
			run_id: c.run_id,
			key: run_pairing::run_key(c.consumer_device_id, &c.r#type, BackupPurpose::Restore),
			reported_at: c.reported_at,
		})
		.collect();
	let (starts, attempts) = run_pairing::pair_issuances(issuances, &reports);

	let mut rows: Vec<RestoreActivity> = checks
		.into_iter()
		.zip(starts)
		.map(|(c, started_at)| {
			let duration_seconds = started_at.map(|s| c.reported_at.as_second() - s.as_second());
			RestoreActivity {
				key: format!("check-{}", c.id),
				status: RunStatus::Reported,
				machine_id: c.machine_id,
				r#type: c.r#type,
				intent: Some(c.intent),
				outcome: Some(c.outcome),
				replica_healthy: Some(c.replica_healthy),
				error: c.error,
				postgres_version: c.postgres_version,
				snapshot_id: c.snapshot_id,
				health_details: c.health_details,
				redaction_outcome: c.redaction_outcome,
				redaction_manifest_version: c.redaction_manifest_version,
				redaction_columns_masked: c.redaction_columns_masked,
				redaction_columns_skipped: c.redaction_columns_skipped,
				redaction_error: c.redaction_error,
				s3_sent_raw_bytes: c.s3_sent_raw_bytes,
				s3_sent_payload_bytes: c.s3_sent_payload_bytes,
				s3_received_raw_bytes: c.s3_received_raw_bytes,
				s3_received_payload_bytes: c.s3_received_payload_bytes,
				observed_at: Some(c.observed_at),
				started_at,
				reported_at: Some(c.reported_at),
				at: c.observed_at,
				duration_seconds,
			}
		})
		.collect();

	for attempt in attempts {
		let status = attempt.status(now);
		let first = attempt.first;
		rows.push(RestoreActivity {
			key: format!("issuance-{}", first.id),
			status,
			machine_id: None,
			r#type: first.r#type,
			intent: None,
			outcome: None,
			replica_healthy: None,
			error: None,
			postgres_version: None,
			snapshot_id: None,
			health_details: None,
			redaction_outcome: None,
			redaction_manifest_version: None,
			redaction_columns_masked: None,
			redaction_columns_skipped: None,
			redaction_error: None,
			s3_sent_raw_bytes: None,
			s3_sent_payload_bytes: None,
			s3_received_raw_bytes: None,
			s3_received_payload_bytes: None,
			observed_at: None,
			started_at: Some(first.issued_at),
			reported_at: None,
			at: first.issued_at,
			duration_seconds: None,
		});
	}

	rows.sort_by(|a, b| b.at.cmp(&a.at));
	rows.truncate(RECENT_CHECKS_LIMIT as usize);
	Ok(Json(rows))
}

/// Declare a managed restore replica.
///
/// Creates a declaration instructing the chosen consumer to maintain a
/// restored replica of the given backup type for the given intent, and
/// records the calling operator as its creator. The overdue bound is a
/// human-friendly duration string (e.g. `2h 30m`); `duration` and `bytes`
/// parameter values likewise accept human-unit strings (e.g. `20Gi`), which
/// are resolved to raw seconds/bytes before validation against the consumer's
/// advertised schema for the intent and stored raw. If the intent is not
/// currently advertised, the values are accepted as-is and the declaration is
/// created with a gap. The name must be unique among the consumer's
/// declarations, and is the only thing that must be: several replicas of one
/// group, type, intent, and server are allowed, told apart by name. Requires
/// the caller to be on the admin allow-list. Responds 400 if the name is blank
/// or the overdue bound or a parameter value fails to parse or validate, and
/// 409 if the consumer already has a declaration with that name.
#[utoipa::path(
	post,
	path = "/create",
	operation_id = "restore_replicas_create",
	tag = "restore_replicas",
	security(("tailscale-admin" = [])),
	request_body = RestoreReplicasCreateArgs,
	responses(
		(status = 200, body = RestoreReplicaView),
		(status = 409, description = "The consumer already has a declaration with that name.", body = ProblemDetailsSchema),
	),
)]
pub async fn create(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<RestoreReplicasCreateArgs>,
) -> Result<Json<RestoreReplicaView>> {
	let mut conn = state.db.get().await?;
	let params = normalized_params_for_intent(
		&mut conn,
		args.consumer_device_id,
		&args.intent,
		&args.params,
		args.redacts,
		args.publishes_schemas,
		args.machine_id,
	)
	.await?;
	let replica = RestoreReplica::create(
		&mut conn,
		NewRestoreReplica {
			consumer_device_id: args.consumer_device_id,
			group_id: args.group_id,
			machine_id: args.machine_id,
			r#type: args.r#type,
			intent: args.intent,
			name: args.name,
			overdue_after: overdue_after_to_pg(args.overdue_after.as_deref())?,
			params: serde_json::to_value(&params).expect("params serialize"),
			redacts: args.redacts,
			publishes_schemas: args.publishes_schemas,
			created_by: Some(admin.login),
		},
	)
	.await?;
	let views = to_views(&mut conn, vec![replica]).await?;
	Ok(Json(views.into_iter().next().expect("one view")))
}

/// Update a restore replica declaration.
///
/// Replaces every field, including scope: the consumer, group, server,
/// backup type, and intent can be retargeted in the same call as the name,
/// overdue bound, parameter values, and enabled flag. The overdue bound and
/// `duration`/`bytes` parameter values accept human-unit strings (e.g.
/// `2h 30m`, `20Gi`), resolved to raw seconds/bytes and validated against the
/// *new* consumer+intent's advertised parameter schema; as with `create`, an
/// intent the new consumer doesn't currently advertise is accepted and the
/// values pass through unvalidated, leaving the declaration with a gap. If the
/// scope changes, the replica at the old scope stops being one Canopy derives
/// checks for, so any active restore-verification finding against it recovers on
/// the next periodic sweep. The name must be unique among the consumer's
/// declarations; the new scope is free to match another declaration's, since
/// the name is what tells two replicas of one scope apart. Requires the caller
/// to be on the admin allow-list. Responds 400 if the name is blank or the
/// overdue bound or a parameter value fails to parse or validate, 404 if the
/// declaration does not exist, and 409 if the name collides with another of
/// the consumer's declarations.
#[utoipa::path(
	post,
	path = "/update",
	operation_id = "restore_replicas_update",
	tag = "restore_replicas",
	security(("tailscale-admin" = [])),
	request_body = RestoreReplicasUpdateArgs,
	responses(
		(status = 200, body = RestoreReplicaView),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "The name collides with another of the consumer's declarations.", body = ProblemDetailsSchema),
	),
)]
pub async fn update(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<RestoreReplicasUpdateArgs>,
) -> Result<Json<RestoreReplicaView>> {
	let mut conn = state.db.get().await?;
	let params = normalized_params_for_intent(
		&mut conn,
		args.consumer_device_id,
		&args.intent,
		&args.params,
		args.redacts,
		args.publishes_schemas,
		args.machine_id,
	)
	.await?;
	let replica = RestoreReplica::update(
		&mut conn,
		args.id,
		RestoreReplicaUpdate {
			consumer_device_id: args.consumer_device_id,
			group_id: args.group_id,
			machine_id: args.machine_id,
			r#type: args.r#type,
			intent: args.intent,
			name: args.name,
			overdue_after: overdue_after_to_pg(args.overdue_after.as_deref())?,
			params: serde_json::to_value(&params).expect("params serialize"),
			redacts: args.redacts,
			publishes_schemas: args.publishes_schemas,
			enabled: args.enabled,
		},
	)
	.await?;
	let views = to_views(&mut conn, vec![replica]).await?;
	Ok(Json(views.into_iter().next().expect("one view")))
}

/// Delete a restore replica declaration.
///
/// Removes the declaration: the consumer stops being asked to maintain the
/// replica and loses the backup access the declaration granted. Nothing asks for
/// that replica any more, so it stops being one Canopy derives checks for and
/// any active restore-verification finding against it recovers on the next
/// periodic sweep. The restore-health reports it collected are retained,
/// detached from the deleted declaration. Requires the caller to
/// be on the admin allow-list. Responds 404 if the declaration does not exist.
#[utoipa::path(
	post,
	path = "/delete",
	operation_id = "restore_replicas_delete",
	tag = "restore_replicas",
	security(("tailscale-admin" = [])),
	request_body = IdArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn delete(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<IdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	RestoreReplica::delete(&mut conn, args.id).await?;
	Ok(Json(()))
}
