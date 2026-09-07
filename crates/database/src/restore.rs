//! Managed restore replicas (RST): the control-plane state for driving an
//! external restore consumer. Operators declare which replicas should exist
//! ([`RestoreReplica`]); consumers register the intents they can satisfy
//! ([`RestoreConsumerCapability`]). The worklist expansion, credential issuance,
//! and restore-health ingest live in the public-server and `jobs` components.

use std::collections::{HashMap, HashSet, hash_map::Entry};

use commons_errors::{AppError, Result};
use commons_types::backup::{
	BackupType, IntentDescriptor, RedactionOutcome, RestoreIntent, RunOutcome, semantics,
};
use commons_types::status::CheckResult;
use diesel::{
	prelude::*,
	result::{DatabaseErrorKind, Error as DieselError},
};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::Serialize;
use uuid::Uuid;

use crate::backup::refs;
use crate::backups::BackupRun;
use crate::issues::{
	CheckInstance, GradedInstance, InstancedCheckFiling, Scope, file_check_instances,
};
use crate::pg_duration::PgDuration;

/// An operator's declaration that a restore consumer should maintain a
/// restorable replica for a server (or every server in a group) and backup
/// type, satisfying a given restore intent. This both queues the work for
/// the consumer and grants that consumer access to the matching backups.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::restore_replicas)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct RestoreReplica {
	/// Unique identifier for this declaration.
	pub id: Uuid,
	/// The device (restore consumer) responsible for maintaining this
	/// replica.
	pub consumer_device_id: Uuid,
	/// The server group this declaration applies to.
	pub group_id: Uuid,
	/// The specific machine this declaration covers. `None` means every
	/// machine currently in the group.
	///
	/// The machine rather than an application on it: what gets restored is a
	/// snapshot, and a snapshot is what a machine backed up.
	// spec: RST#declared-replicas
	pub machine_id: Option<Uuid>,
	/// The backup type this declaration covers (e.g. a database snapshot
	/// type). Any type name the consumer advertises support for is
	/// accepted.
	#[diesel(column_name = type_)]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// The restore intent this declaration should satisfy — what kind of
	/// restore behaviour the consumer should perform against the backups
	/// (for example, a periodic verification restore vs. one held ready for
	/// disaster recovery). Any intent name the consumer advertises support
	/// for is accepted.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// An operator-assigned label for this declaration, shown in the UI.
	pub name: String,
	/// How long, in seconds, this replica may go without a healthy restore
	/// report before it's considered overdue and an alert is raised. For an
	/// intent that only needs to verify the latest snapshot once, this
	/// instead bounds how long that snapshot may go unverified. `None`
	/// means no overdue bound is enforced.
	#[schema(value_type = Option<i64>)]
	pub overdue_after: Option<PgDuration>,
	/// Operator-supplied parameter values for this declaration, keyed by
	/// parameter name, passed through to the consumer. Only the values the
	/// operator explicitly set are included.
	#[schema(value_type = Object)]
	pub params: serde_json::Value,
	/// Whether this replica is served de-identified. Canopy resolves the
	/// masking manifest itself from the server's product, so this flag is
	/// the whole of the operator's say in it, and it answers on its own
	/// whether a replica that came up unmasked is a finding.
	pub redacts: bool,
	/// Whether this declaration is currently active. When disabled, it
	/// produces no work and grants no access, but is kept for reference.
	pub enabled: bool,
	/// The operator who created this declaration. `None` if not recorded.
	pub created_by: Option<String>,
	/// When this declaration was created.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When this declaration was last modified.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::restore_replicas)]
pub struct NewRestoreReplica {
	pub consumer_device_id: Uuid,
	pub group_id: Uuid,
	pub machine_id: Option<Uuid>,
	#[diesel(column_name = type_)]
	pub r#type: BackupType,
	pub intent: RestoreIntent,
	pub name: String,
	pub overdue_after: Option<PgDuration>,
	pub params: serde_json::Value,
	pub redacts: bool,
	pub created_by: Option<String>,
}

/// The full new state of a declaration, including its scope. Every field is
/// always set (there is no partial-update shorthand); `machine_id: None` means
/// "every machine in the group".
#[derive(Debug, Clone)]
pub struct RestoreReplicaUpdate {
	pub consumer_device_id: Uuid,
	pub group_id: Uuid,
	pub machine_id: Option<Uuid>,
	pub r#type: BackupType,
	pub intent: RestoreIntent,
	pub name: String,
	pub overdue_after: Option<PgDuration>,
	pub params: serde_json::Value,
	pub redacts: bool,
	pub enabled: bool,
}

/// Normalise an operator-supplied declaration name: surrounding whitespace is
/// insignificant, and a name that is blank once trimmed is rejected. Names are
/// unique per consumer, so leaving the whitespace on would let a near-identical
/// name slip past the index.
fn normalize_name(name: String) -> Result<String> {
	let trimmed = name.trim();
	if trimmed.is_empty() {
		return Err(AppError::BadRequest(
			"restore replica name cannot be empty".into(),
		));
	}
	Ok(trimmed.to_owned())
}

/// Turn a unique violation into a `409` naming what the operator has to change.
///
/// The name is the only thing a declaration is unique on: an operator may
/// declare as many replicas of one `(group, type, intent, server)` as they have
/// uses for, and tells them apart by name.
fn unique_violation(info: &dyn diesel::result::DatabaseErrorInformation) -> AppError {
	match info.constraint_name() {
		Some("restore_replicas_consumer_name") | None => {
			AppError::Conflict("this consumer already has a restore replica with that name".into())
		}
		Some(other) => {
			AppError::Conflict(format!("this declaration collides with another ({other})"))
		}
	}
}

impl RestoreReplica {
	/// Create a declaration. A name already used by another of the consumer's
	/// declarations maps to `409`. The scope is deliberately not unique: several
	/// replicas of one `(group, type, intent, server)` are allowed, told apart
	/// by name.
	pub async fn create(db: &mut AsyncPgConnection, new: NewRestoreReplica) -> Result<Self> {
		use crate::schema::restore_replicas::dsl;
		let new = NewRestoreReplica {
			name: normalize_name(new.name)?,
			..new
		};
		match diesel::insert_into(dsl::restore_replicas)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
		{
			Ok(row) => Ok(row),
			Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, info)) => {
				Err(unique_violation(&*info))
			}
			Err(e) => Err(AppError::from(e)),
		}
	}

	/// Every declaration, newest first — the operator overview.
	pub async fn list_all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::restore_replicas::dsl;
		dsl::restore_replicas
			.select(Self::as_select())
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Declarations scoped to a group.
	pub async fn list_for_group(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::restore_replicas::dsl;
		dsl::restore_replicas
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Enabled declarations for a consumer — the basis of its worklist (before
	/// per-server expansion and capability filtering).
	pub async fn list_enabled_for_consumer(
		db: &mut AsyncPgConnection,
		consumer_device_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::restore_replicas::dsl;
		dsl::restore_replicas
			.select(Self::as_select())
			.filter(dsl::consumer_device_id.eq(consumer_device_id))
			.filter(dsl::enabled.eq(true))
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::restore_replicas::dsl;
		dsl::restore_replicas
			.select(Self::as_select())
			.filter(dsl::id.eq(id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.ok_or(AppError::DatabaseQuery(DieselError::NotFound))
	}

	/// Edit every field of a declaration, including its scope. A name already
	/// used by another of the consumer's declarations maps to `409`, same as
	/// [`Self::create`]; the scope itself is free to collide. If the scope
	/// (group, server, or type/intent) moves, or the declaration is disabled,
	/// any active restore-verification alert keyed to the *old* scope is
	/// recovered — the overdue sweep only walks current *enabled* declarations,
	/// so a stale key would otherwise never clear.
	pub async fn update(
		db: &mut AsyncPgConnection,
		id: Uuid,
		update: RestoreReplicaUpdate,
	) -> Result<Self> {
		use crate::schema::restore_replicas::dsl;

		let name = normalize_name(update.name)?;

		let result = match diesel::update(dsl::restore_replicas.filter(dsl::id.eq(id)))
			.set((
				dsl::consumer_device_id.eq(update.consumer_device_id),
				dsl::group_id.eq(update.group_id),
				dsl::machine_id.eq(update.machine_id),
				dsl::type_.eq(update.r#type),
				dsl::intent.eq(update.intent),
				dsl::name.eq(name),
				dsl::overdue_after.eq(update.overdue_after),
				dsl::params.eq(update.params),
				dsl::redacts.eq(update.redacts),
				dsl::enabled.eq(update.enabled),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
		{
			Ok(row) => row,
			Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, info)) => {
				return Err(unique_violation(&*info));
			}
			Err(DieselError::NotFound) => {
				return Err(AppError::DatabaseQuery(DieselError::NotFound));
			}
			Err(e) => return Err(AppError::from(e)),
		};

		// Nothing to clear by hand when the scope moves or the declaration is
		// disabled: `sweep_restore_checks` re-derives each server's restore checks
		// every pass, and a key stops being derivable once neither a
		// declaration nor a still-coverable report yields it, so the check
		// re-files without it. That is what recomputing buys over accumulating.
		Ok(result)
	}

	/// Delete a declaration. Its restore checks need no explicit recovery: the
	/// next [`sweep_restore_checks`] pass re-derives each server's checks, and with the
	/// declaration gone its key is derivable only while another declaration
	/// still covers the `(group, type)` a report could arrive on — so a replica
	/// nothing tracks any more drops out and the check re-files without it.
	///
	/// Restore-health reports the declaration collected are retained: the FK
	/// from `backup_restore_checks` is `ON DELETE SET NULL`, so each report
	/// survives with its group, server, type, and intent, no longer attached to
	/// a declaration.
	pub async fn delete(db: &mut AsyncPgConnection, id: Uuid) -> Result<()> {
		db.transaction::<_, AppError, _>(async |conn| {
			use crate::schema::restore_replicas::dsl;
			let n = diesel::delete(dsl::restore_replicas.filter(dsl::id.eq(id)))
				.execute(conn)
				.await?;
			if n == 0 {
				return Err(AppError::DatabaseQuery(DieselError::NotFound));
			}
			// See `update`: the next sweep re-derives the server's restore
			// checks, so a deleted declaration drops out on its own.
			Ok(())
		})
		.await
	}

	/// Whether an enabled declaration covers `(consumer, group, type)` — the
	/// authorization check for issuing restore credentials. A server-scoped or
	/// a group-wide declaration both satisfy it.
	pub async fn authorizes(
		db: &mut AsyncPgConnection,
		consumer_device_id: Uuid,
		group_id: Uuid,
		r#type: &BackupType,
	) -> Result<bool> {
		use crate::schema::restore_replicas::dsl;
		let n: i64 = dsl::restore_replicas
			.filter(dsl::consumer_device_id.eq(consumer_device_id))
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::type_.eq(r#type.as_str()))
			.filter(dsl::enabled.eq(true))
			.count()
			.get_result(db)
			.await?;
		Ok(n > 0)
	}
}

/// A consumer's advertised capability row: one intent it can satisfy, with the
/// description, semantics, and parameter schema it advertises. Registered by the
/// consumer on start and whenever it changes; Canopy dispatches only matching
/// worklist entries, acts on the semantics it recognises, and constrains the
/// declaration UX to this set.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::restore_consumer_capabilities)]
#[diesel(check_for_backend(diesel::pg::Pg))]
struct CapabilityRow {
	#[allow(dead_code)]
	consumer_device_id: Uuid,
	intent: RestoreIntent,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	#[allow(dead_code)]
	registered_at: Timestamp,
	description: Option<String>,
	semantics: serde_json::Value,
	params: serde_json::Value,
}

impl CapabilityRow {
	fn into_descriptor(self) -> IntentDescriptor {
		IntentDescriptor {
			intent: self.intent,
			description: self.description,
			semantics: serde_json::from_value(self.semantics).unwrap_or_default(),
			params: serde_json::from_value(self.params).unwrap_or_default(),
		}
	}
}

#[derive(Debug, Insertable)]
#[diesel(table_name = crate::schema::restore_consumer_capabilities)]
struct NewCapability {
	consumer_device_id: Uuid,
	intent: RestoreIntent,
	description: Option<String>,
	semantics: serde_json::Value,
	params: serde_json::Value,
}

/// Registration and lookup of a consumer's advertised capabilities.
pub struct RestoreConsumerCapability;

impl RestoreConsumerCapability {
	/// Replace a consumer's advertised set with `descriptors`. Implemented as
	/// upsert-then-prune (not a transaction) so there is never a window where a
	/// still-valid intent is absent: advertised intents are upserted first, then
	/// any no longer present are removed.
	pub async fn register(
		db: &mut AsyncPgConnection,
		consumer_device_id: Uuid,
		descriptors: &[IntentDescriptor],
	) -> Result<()> {
		use crate::schema::restore_consumer_capabilities::dsl;
		use diesel::upsert::excluded;

		let names: Vec<String> = descriptors
			.iter()
			.map(|d| d.intent.as_str().to_owned())
			.collect();

		let rows: Vec<NewCapability> = descriptors
			.iter()
			.map(|d| NewCapability {
				consumer_device_id,
				intent: d.intent.clone(),
				description: d.description.clone(),
				semantics: serde_json::to_value(&d.semantics).expect("semantics serialize"),
				params: serde_json::to_value(&d.params).expect("params serialize"),
			})
			.collect();

		if !rows.is_empty() {
			diesel::insert_into(dsl::restore_consumer_capabilities)
				.values(rows)
				.on_conflict((dsl::consumer_device_id, dsl::intent))
				.do_update()
				.set((
					dsl::description.eq(excluded(dsl::description)),
					dsl::semantics.eq(excluded(dsl::semantics)),
					dsl::params.eq(excluded(dsl::params)),
				))
				.execute(db)
				.await?;
		}

		diesel::delete(
			dsl::restore_consumer_capabilities
				.filter(dsl::consumer_device_id.eq(consumer_device_id))
				.filter(dsl::intent.ne_all(names)),
		)
		.execute(db)
		.await?;
		Ok(())
	}

	/// The descriptors a consumer currently advertises, ordered by intent.
	pub async fn list_for_consumer(
		db: &mut AsyncPgConnection,
		consumer_device_id: Uuid,
	) -> Result<Vec<IntentDescriptor>> {
		use crate::schema::restore_consumer_capabilities::dsl;
		let rows: Vec<CapabilityRow> = dsl::restore_consumer_capabilities
			.filter(dsl::consumer_device_id.eq(consumer_device_id))
			.select(CapabilityRow::as_select())
			.order(dsl::intent.asc())
			.load(db)
			.await?;
		Ok(rows
			.into_iter()
			.map(CapabilityRow::into_descriptor)
			.collect())
	}
}

/// Whether any enabled declaration on `group_id` migrates what it restores.
///
/// The consumer's advertised `migrate` semantic decides, whatever the intent is
/// named: only a migrating intent is dispatched a target version.
// spec: RST#verdicts
pub async fn group_migrates(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<bool> {
	for replica in RestoreReplica::list_for_group(db, group_id).await? {
		if !replica.enabled {
			continue;
		}
		let advertised =
			RestoreConsumerCapability::list_for_consumer(db, replica.consumer_device_id).await?;
		if advertised
			.iter()
			.any(|d| d.intent == replica.intent && d.has_semantic(semantics::MIGRATE))
		{
			return Ok(true);
		}
	}
	Ok(false)
}

/// Whether any enabled declaration on `group_id` migrates what it restores of
/// the environment at `rank`: a group-wide declaration covers every
/// environment, one over a single machine covers the environment that machine
/// serves.
// spec: RST#verdicts
pub async fn environment_migrates(
	db: &mut AsyncPgConnection,
	group_id: Uuid,
	rank: commons_types::server::rank::ServerRank,
) -> Result<bool> {
	for replica in RestoreReplica::list_for_group(db, group_id).await? {
		if !replica.enabled {
			continue;
		}
		if let Some(machine_id) = replica.machine_id
			&& crate::machines::Machine::rank(db, machine_id).await? != Some(rank)
		{
			continue;
		}
		let advertised =
			RestoreConsumerCapability::list_for_consumer(db, replica.consumer_device_id).await?;
		if advertised
			.iter()
			.any(|d| d.intent == replica.intent && d.has_semantic(semantics::MIGRATE))
		{
			return Ok(true);
		}
	}
	Ok(false)
}

/// Why a server a redacting declaration covers can't be redacted.
///
/// Each of these withholds the server's worklist entry: a replica that
/// cannot be redacted is not restored at all, since an unredacted replica
/// standing in for a redacted one is worse than no replica.
// spec: RST#the-masking-manifest
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum RedactionGapReason {
	/// The server's product publishes no masking manifests, so the replica is
	/// withheld from the worklist rather than restored unmasked.
	ProductHasNoManifest,
	/// The product publishes them, but not for the version this server
	/// reports, so the consumer would fetch a URL that 404s. The replica is
	/// still dispatched — the consumer resolves the manifest against the
	/// version in the data it restored, which may not be this one, and holds
	/// the switchover if it can't.
	VersionHasNoManifest,
}

/// Whether a server can be redacted, and why not when it can't.
///
/// Corroborates the product's manifest template against the artefacts the
/// version actually published: a template that resolves to nothing is
/// caught here, at declaration time, rather than when a restore fails.
// spec: RST#the-masking-manifest
pub async fn redaction_gap_for(
	db: &mut AsyncPgConnection,
	server: &crate::applications::Application,
) -> Result<Option<(RedactionGapReason, Option<String>)>> {
	let Some(manifest) = server.r#type.caps().redaction else {
		return Ok(Some((RedactionGapReason::ProductHasNoManifest, None)));
	};

	// The consumer resolves the manifest against the version in the data it
	// restored, not against what the server last reported, so a server Canopy
	// holds no version for isn't one that can't be redacted — it's one Canopy
	// can't corroborate, which is not a finding.
	let Some(reported) =
		crate::reported_detail::ReportedDetail::last_version(db, server.id).await?
	else {
		return Ok(None);
	};
	let shown = reported.0.to_string();

	// An unpublished version has no artefacts at all, which reads the same
	// way as a published one that didn't upload a manifest: either way the
	// URL the consumer would fetch isn't there.
	let Ok(version) = crate::versions::Version::get_by_version(db, reported).await else {
		return Ok(Some((
			RedactionGapReason::VersionHasNoManifest,
			Some(shown),
		)));
	};

	let published = crate::artifacts::Artifact::get_for_version(db, version.id)
		.await?
		.into_iter()
		.any(|a| a.artifact_type == manifest.artifact_type);

	Ok((!published).then_some((RedactionGapReason::VersionHasNoManifest, Some(shown))))
}

/// A restore-health report submitted by a restore consumer: proof (or
/// disproof) that a backup snapshot actually restores into a healthy
/// database — the strongest available signal of backup health. `snapshot_id`
/// identifies which snapshot was restored, if the report is snapshot-scoped.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::backup_restore_checks)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRestoreCheck {
	/// Unique identifier for this report.
	pub id: i64,
	/// The replica declaration this report is for, if it was made against
	/// a declared replica. `None` for reports not tied to a declaration.
	pub replica_id: Option<Uuid>,
	/// The declaration's name when the report was recorded. Several replicas can
	/// share one `(server, type, intent)`, so this is what tells them apart —
	/// held here rather than read through `replica_id` so a report keeps naming
	/// its replica once the declaration is retired. `None` for a report that
	/// named no declaration.
	pub replica_name: Option<String>,
	/// The device (restore consumer) that submitted this report.
	pub consumer_device_id: Uuid,
	/// The server group this report belongs to.
	pub group_id: Uuid,
	/// The machine whose snapshot this report is about, if any.
	// spec: RST#restore-health-reporting
	pub machine_id: Option<Uuid>,
	/// The backup type this report covers.
	#[diesel(column_name = type_)]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// The restore intent this report was performed for.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// The id of the snapshot that was restored, if the report is scoped to
	/// a specific snapshot.
	pub snapshot_id: Option<String>,
	/// Whether the restore attempt itself succeeded or failed.
	#[schema(value_type = String)]
	pub outcome: RunOutcome,
	/// Error message reported for a failed restore, if any.
	pub error: Option<String>,
	/// Whether the restored database came up and passed its health checks.
	/// A report only counts as fully healthy when `outcome` is success and
	/// this is also `true`.
	pub replica_healthy: bool,
	/// The Postgres version of the restored database, if reported.
	pub postgres_version: Option<String>,
	/// When the restore attempt this report describes actually took place.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub observed_at: Timestamp,
	/// Bytes sent to storage for this restore, counting the full HTTP
	/// request including signing/chunking overhead.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Bytes sent to storage for this restore, counting only the decoded
	/// object data (excludes request/signing overhead).
	pub s3_sent_payload_bytes: Option<i64>,
	/// Bytes received from storage for this restore, counting the full HTTP
	/// response including framing overhead.
	pub s3_received_raw_bytes: Option<i64>,
	/// Bytes received from storage for this restore, counting only the
	/// decoded object data (excludes response framing overhead).
	pub s3_received_payload_bytes: Option<i64>,
	/// When this report was received.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub reported_at: Timestamp,
	/// Additional health data supplied by the consumer (for example,
	/// database cluster statistics or whether indexes needed repair).
	/// Passed through and displayed as-is. `None` if none was supplied.
	pub health_details: Option<serde_json::Value>,
	/// The run this report belongs to, when the consumer stamped its run-uuid on
	/// the restore-credentials request. Ties the report to its issuance exactly
	/// (for Canopy-measured duration). `None` for older consumers, which fall
	/// back to time-window matching.
	pub run_id: Option<Uuid>,
	/// How far the replica's masking manifest got. `None` for a replica that
	/// doesn't redact.
	pub redaction_outcome: Option<RedactionOutcome>,
	/// The version whose manifest was fetched, when the manifest URL named one.
	pub redaction_manifest_version: Option<String>,
	/// How many columns the manifest masked.
	pub redaction_columns_masked: Option<i64>,
	/// How many columns the manifest named but could not mask. Non-zero is
	/// what makes a redaction partial.
	pub redaction_columns_skipped: Option<i64>,
	/// Why the redaction failed, when it did. Distinct from `error`, which
	/// describes the restore: the restore can succeed and the redaction that
	/// follows it fail.
	pub redaction_error: Option<String>,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::backup_restore_checks)]
pub struct NewBackupRestoreCheck {
	pub replica_id: Option<Uuid>,
	/// Left unset by callers: [`BackupRestoreCheck::record_report`] resolves it
	/// from `replica_id` so a report always carries the name the declaration had
	/// when it was made.
	pub replica_name: Option<String>,
	pub consumer_device_id: Uuid,
	pub group_id: Uuid,
	pub machine_id: Option<Uuid>,
	#[diesel(column_name = type_)]
	pub r#type: BackupType,
	pub intent: RestoreIntent,
	pub snapshot_id: Option<String>,
	pub outcome: RunOutcome,
	pub error: Option<String>,
	pub replica_healthy: bool,
	pub postgres_version: Option<String>,
	#[diesel(serialize_as = jiff_diesel::Timestamp)]
	pub observed_at: Timestamp,
	pub s3_sent_raw_bytes: Option<i64>,
	pub s3_sent_payload_bytes: Option<i64>,
	pub s3_received_raw_bytes: Option<i64>,
	pub s3_received_payload_bytes: Option<i64>,
	pub health_details: Option<serde_json::Value>,
	pub run_id: Option<Uuid>,
	pub redaction_outcome: Option<RedactionOutcome>,
	pub redaction_manifest_version: Option<String>,
	pub redaction_columns_masked: Option<i64>,
	pub redaction_columns_skipped: Option<i64>,
	pub redaction_error: Option<String>,
}

/// Fill in a report's `replica_name` from the declaration it names, so the
/// report identifies its replica on its own. Several replicas can share one
/// `(server, type, intent)` and are told apart by name, so a report that
/// reached the declaration but not its name would be indistinguishable from
/// its siblings' once the declaration is retired.
///
/// The ingest requires a report to name a declaration that exists, so the name
/// is always found on that path. A report recorded without one keeps a `None`
/// name: the DB layer is also where the reports predating the requirement sit,
/// and they are still facts about a restore.
pub(crate) async fn stamp_replica_name(
	db: &mut AsyncPgConnection,
	new: NewBackupRestoreCheck,
) -> Result<NewBackupRestoreCheck> {
	use crate::schema::restore_replicas::dsl;
	let Some(replica_id) = new.replica_id else {
		return Ok(new);
	};
	let name: Option<String> = dsl::restore_replicas
		.select(dsl::name)
		.filter(dsl::id.eq(replica_id))
		.first(db)
		.await
		.optional()
		.map_err(AppError::from)?;
	Ok(NewBackupRestoreCheck {
		replica_name: name,
		..new
	})
}

impl BackupRestoreCheck {
	/// Record a restore-health report.
	///
	/// Recording is all this does: the checks the report feeds —
	/// `restore-verification` and `redaction` — are filed by [`sweep_restore_checks`],
	/// which is their sole filer. A replica's state is the worse of what its
	/// latest report said and whether it has gone overdue, and only the sweep
	/// holds every one of a server's replicas at once; this path holds one. Two
	/// filers on the same check would race and drift apart.
	///
	/// The sweep runs on the same minute cadence as the reachability tick, and
	/// these are non-paging warnings (see `BKJ#alerting`), so the delay between
	/// a report landing and its check reflecting it does not matter.
	pub async fn record_report(
		db: &mut AsyncPgConnection,
		new: NewBackupRestoreCheck,
	) -> Result<i64> {
		use crate::schema::backup_restore_checks::dsl;
		let new = stamp_replica_name(db, new).await?;
		diesel::insert_into(dsl::backup_restore_checks)
			.values(new)
			.returning(dsl::id)
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Recent reports for a group, newest first — the operator restore-health
	/// view.
	pub async fn list_recent_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_restore_checks::dsl;
		dsl::backup_restore_checks
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::observed_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Recent reports for one replica, newest first.
	///
	/// Not [`Self::list_recent_for_group`] filtered afterwards: the limit has
	/// to bite the replica's own checks, or a low-frequency replica sharing a
	/// group with a chatty one has every one of its reports pushed out of the
	/// window and reads as never checked.
	pub async fn list_recent_for_replica(
		db: &mut AsyncPgConnection,
		replica_id: Uuid,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_restore_checks::dsl;
		dsl::backup_restore_checks
			.select(Self::as_select())
			.filter(dsl::replica_id.eq(Some(replica_id)))
			.order(dsl::observed_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Recent reports across all groups, newest first.
	pub async fn list_recent(db: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Self>> {
		use crate::schema::backup_restore_checks::dsl;
		dsl::backup_restore_checks
			.select(Self::as_select())
			.order(dsl::observed_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The latest report per `(server, type, intent, name)` across the fleet,
	/// whatever its outcome — what the sweep needs to know each replica's
	/// *current* state, as opposed to when it was last healthy.
	///
	/// Fleet-wide rather than per group because the sweep derives its instances
	/// from these keys as well as from declarations, so it cannot know which
	/// groups to ask about until it has them.
	///
	/// Carries the whole row so the redaction fields come along: a report says
	/// both whether the restore came up healthy and whether its masking
	/// applied, and those are two checks off one report.
	pub async fn latest_by_key(db: &mut AsyncPgConnection) -> Result<HashMap<ReplicaKey, Self>> {
		use crate::schema::backup_restore_checks::dsl;
		let rows: Vec<Self> = dsl::backup_restore_checks
			.select(Self::as_select())
			.filter(dsl::machine_id.is_not_null())
			.distinct_on((dsl::machine_id, dsl::type_, dsl::intent, dsl::replica_name))
			// The recency tiebreak is one raw fragment because diesel only proves
			// `DISTINCT ON`/`ORDER BY` agreement for tuples up to five elements,
			// and the four key columns plus these two would be six.
			.order_by((
				dsl::machine_id,
				dsl::type_,
				dsl::intent,
				dsl::replica_name,
				diesel::dsl::sql::<diesel::sql_types::Untyped>("observed_at DESC, id DESC"),
			))
			.load(db)
			.await?;
		Ok(rows
			.into_iter()
			.filter_map(|r| {
				r.machine_id.map(|mid| {
					(
						(
							mid,
							r.r#type.clone(),
							r.intent.clone(),
							r.replica_name.clone(),
						),
						r,
					)
				})
			})
			.collect())
	}

	/// Latest *healthy* report timestamp per `(server, type, intent, name)` in a
	/// group — the freshness anchor the overdue sweep compares against.
	pub async fn latest_healthy_by_key_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<HashMap<ReplicaKey, Timestamp>> {
		use crate::schema::backup_restore_checks::dsl;
		let rows: Vec<Self> = dsl::backup_restore_checks
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::machine_id.is_not_null())
			.filter(dsl::outcome.eq(RunOutcome::Success))
			.filter(dsl::replica_healthy.eq(true))
			.distinct_on((dsl::machine_id, dsl::type_, dsl::intent, dsl::replica_name))
			.order_by((
				dsl::machine_id,
				dsl::type_,
				dsl::intent,
				dsl::replica_name,
				dsl::observed_at.desc(),
			))
			.load(db)
			.await?;
		Ok(rows
			.into_iter()
			.filter_map(|r| {
				r.machine_id.map(|mid| {
					(
						(
							mid,
							r.r#type.clone(),
							r.intent.clone(),
							r.replica_name.clone(),
						),
						r.observed_at,
					)
				})
			})
			.collect())
	}

	/// The snapshot id of the most recent *healthy* report per
	/// `(server, type, intent, name)` in a group — the anchor for `once`
	/// suppression (a snapshot is already verified when this equals the latest
	/// snapshot) and snapshot-driven overdue. Reports without a snapshot id are
	/// ignored.
	pub async fn latest_healthy_snapshot_by_key_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<HashMap<ReplicaKey, String>> {
		use crate::schema::backup_restore_checks::dsl;
		let rows: Vec<Self> = dsl::backup_restore_checks
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::machine_id.is_not_null())
			.filter(dsl::snapshot_id.is_not_null())
			.filter(dsl::outcome.eq(RunOutcome::Success))
			.filter(dsl::replica_healthy.eq(true))
			.distinct_on((dsl::machine_id, dsl::type_, dsl::intent, dsl::replica_name))
			.order_by((
				dsl::machine_id,
				dsl::type_,
				dsl::intent,
				dsl::replica_name,
				dsl::observed_at.desc(),
			))
			.load(db)
			.await?;
		Ok(rows
			.into_iter()
			.filter_map(|r| match (r.machine_id, r.snapshot_id) {
				(Some(mid), Some(snap)) => Some((
					(mid, r.r#type.clone(), r.intent.clone(), r.replica_name),
					snap,
				)),
				_ => None,
			})
			.collect())
	}
}

/// One replica key: a named `(type, intent)` replica on one machine. Every
/// dimension bar the machine is an open-ended string, so the set of keys is
/// discovered, never enumerated.
///
/// The machine, because what a replica restores is a snapshot and a snapshot is
/// what a machine backed up. A box running two workloads has one replica per
/// declaration, not one per workload.
///
/// The name is part of the key because an operator may declare several replicas
/// of one `(group, type, intent, machine)` and tell them apart by name; without
/// it two of them would grade as one instance and their reports would overwrite
/// each other. It is `None` only for a report that named no declaration.
pub type ReplicaKey = (Uuid, BackupType, RestoreIntent, Option<String>);

/// What the sweep has gathered about one replica key before it grades it.
#[derive(Default)]
struct KeyWork {
	/// An enabled declaration covers this key on its server. The operator's name
	/// for it is in the key itself; this is only whether one is currently asking
	/// for the replica, which a key derived from a report alone is not.
	declared: bool,
	/// A `migrate` declaration covers it, so restore health is not what it is
	/// for — whether its candidate version applies is.
	migrates: bool,
	/// Whether the key's latest report is allowed to drive its checks: either a
	/// declaration covers the key, or its `(group, type)` is still covered, so
	/// another report could yet arrive to change the answer.
	reports_count: bool,
	/// Past its declaration's overdue bound.
	overdue: bool,
	/// That bound was measured against the latest snapshot (a `once` intent).
	once: bool,
	/// A `migrate` declaration's candidate has gone untried past the bound:
	/// `(target version, snapshot)`.
	untried: Option<(String, String)>,
}

/// The instances one server's three restore checks are filed from.
/// A machine's instances of the two checks that are the machine's own.
#[derive(Default)]
struct MachineInstances {
	verification: Vec<CheckInstance>,
	redaction: Vec<CheckInstance>,
}

/// The restore checks' sole filer: re-derive every server's
/// `restore-verification`, `redaction` and `migration-test` checks from what
/// Canopy currently holds, and file the ones that have something to say.
///
/// Each of a server's replicas is one instance of each check rather than a
/// check of its own (see the Names section of the CHK spec), and an instance's
/// result is the worse of what its latest report said and whether it has gone
/// past its declaration's overdue bound: one judgement about the replica, not
/// two writers racing on one name.
///
/// A replica key is derived from recorded facts as much as from live
/// declarations, because a finding has to survive the thing that produced it
/// going quiet. A key `(server, type, intent)` yields instances when:
///
/// - an enabled declaration covers it on that server, whatever its consumer
///   currently advertises. A capability that stops being advertised is a gap,
///   surfaced as one; it is not grounds for a standing finding to disappear.
///   Only the overdue judgement needs the intent's semantics, so that is the
///   only thing gated on them. Or,
/// - Canopy holds a report for it and an enabled declaration still asks for that
///   replica somewhere in the server's group, so a report about a server the
///   declaration does not currently name still counts. Once no declaration asks
///   for the replica at all, nothing can report on it again
///   ([`RestoreReplica::authorizes`] is what a consumer has to satisfy to
///   report), so a finding held on it could never recover: it stops being
///   derived rather than being pinned open with no way out. Or,
/// - Canopy holds a migration verdict for it, which yields a `migration-test`
///   instance whatever declares the replica now. A failed migration is a fact
///   about a candidate version measured against this group's data, not a
///   deadline on a declaration, and what supersedes it is a later verdict.
///
/// Returns the number of checks this pass left degraded.
// spec: RST#alerting
pub async fn sweep_restore_checks(db: &mut AsyncPgConnection) -> Result<usize> {
	use crate::schema::restore_replicas::dsl;
	let now = Timestamp::now();

	// Every enabled declaration, not just the ones with an overdue bound: this
	// sweep is the sole filer of the restore checks, so a declaration without a
	// bound still needs its latest report's health reflected. A missing bound
	// means "never overdue", not "never checked". Ordered so that when two
	// declarations cover one key, which of them names the instance is stable.
	let declarations: Vec<RestoreReplica> = dsl::restore_replicas
		.select(RestoreReplica::as_select())
		.filter(dsl::enabled.eq(true))
		.order_by((dsl::name, dsl::id))
		.load(db)
		.await?;

	// The recorded facts, fleet-wide and one query each.
	let latest_reports = BackupRestoreCheck::latest_by_key(db).await?;
	let latest_verdicts = crate::migration_tests::latest_verdict_by_key(db).await?;

	// The replicas an enabled declaration still asks for, wherever in their group
	// they sit. A report is derivable into an instance while its replica is still
	// declared somewhere in the group, so a report about a server the declaration
	// does not currently name is not lost — but a replica nothing declares any
	// more stops being derived.
	let declared: HashSet<(Uuid, BackupType, RestoreIntent)> = declarations
		.iter()
		.map(|d| (d.group_id, d.r#type.clone(), d.intent.clone()))
		.collect();

	// Per-consumer descriptors (to read semantics) and per-group anchors.
	let mut capability_cache: HashMap<Uuid, HashMap<RestoreIntent, IntentDescriptor>> =
		HashMap::new();
	let mut healthy_cache: HashMap<Uuid, HashMap<ReplicaKey, Timestamp>> = HashMap::new();
	let mut verified_snapshot_cache: HashMap<Uuid, HashMap<ReplicaKey, String>> = HashMap::new();
	let mut latest_snapshot_cache: HashMap<Uuid, HashMap<(Uuid, BackupType), BackupRun>> =
		HashMap::new();

	let mut work: HashMap<ReplicaKey, KeyWork> = HashMap::new();
	let mut machines: HashMap<Uuid, crate::machines::Machine> = HashMap::new();

	for d in &declarations {
		let capabilities = match capability_cache.entry(d.consumer_device_id) {
			Entry::Occupied(e) => e.into_mut(),
			Entry::Vacant(e) => e.insert(
				RestoreConsumerCapability::list_for_consumer(db, d.consumer_device_id)
					.await?
					.into_iter()
					.map(|desc| (desc.intent.clone(), desc))
					.collect(),
			),
		};
		let descriptor = capabilities.get(&d.intent);
		let checks = descriptor.is_some_and(|desc| desc.has_semantic(semantics::CHECK));
		let once = descriptor.is_some_and(|desc| desc.has_semantic(semantics::ONCE));
		let migrates = descriptor.is_some_and(|desc| desc.has_semantic(semantics::MIGRATE));

		// A declaration covers machines: a whole-group one expands over every
		// machine in the group, a targeted one over the single machine it
		// names. Expanding over applications gave a two-workload box two
		// replicas of the one snapshot.
		// spec: RST#declared-replicas
		let covered_machines: Vec<crate::machines::Machine> = match d.machine_id {
			Some(mid) => match crate::machines::Machine::get_by_id(db, mid).await.ok() {
				Some(m) if m.group_id == Some(d.group_id) && m.deleted_at.is_none() => vec![m],
				_ => vec![],
			},
			None => crate::machines::Machine::list_for_group(db, d.group_id).await?,
		};
		if covered_machines.is_empty() {
			continue;
		}

		if let Entry::Vacant(e) = healthy_cache.entry(d.group_id) {
			e.insert(BackupRestoreCheck::latest_healthy_by_key_for_group(db, d.group_id).await?);
			verified_snapshot_cache.insert(
				d.group_id,
				BackupRestoreCheck::latest_healthy_snapshot_by_key_for_group(db, d.group_id)
					.await?,
			);
			latest_snapshot_cache.insert(
				d.group_id,
				BackupRun::latest_success_by_machine_type_for_group(db, d.group_id).await?,
			);
		}

		for machine in covered_machines {
			let mid = machine.id;
			let key = (
				mid,
				d.r#type.clone(),
				d.intent.clone(),
				Some(d.name.clone()),
			);

			// A `migrate` intent is overdue on its own terms: the question is
			// whether the candidate version has been tried against the latest
			// snapshot, not whether the replica restored.
			let untried = if migrates && checks {
				untried_candidate(db, d, &machine, &latest_snapshot_cache[&d.group_id], now).await?
			} else {
				None
			};
			// Overdue is a property of the bound, so a declaration without one
			// is never overdue — but its latest report's health still counts.
			let overdue = match (migrates, checks, d.overdue_after) {
				(false, true, Some(bound)) => is_overdue(
					&key,
					bound,
					once,
					now,
					&healthy_cache[&d.group_id],
					&verified_snapshot_cache[&d.group_id],
					&latest_snapshot_cache[&d.group_id],
				),
				_ => false,
			};

			machines.entry(mid).or_insert(machine);
			let w = work.entry(key).or_default();
			w.declared = true;
			w.reports_count = true;
			w.migrates |= migrates;
			w.overdue |= overdue;
			w.once |= overdue && once;
			if untried.is_some() {
				w.untried = untried;
			}
		}
	}

	// Keys Canopy holds a report for on a server no declaration currently names,
	// which stay derivable while the replica itself is still declared.
	for (key, report) in &latest_reports {
		let declaration = (
			report.group_id,
			report.r#type.clone(),
			report.intent.clone(),
		);
		if work.contains_key(key) || !declared.contains(&declaration) {
			continue;
		}
		if live_machine(db, &mut machines, key.0).await?.is_none() {
			continue;
		}
		work.entry(key.clone()).or_default().reports_count = true;
	}

	// Keys with a recorded verdict, which stand whatever covers them now.
	for key in latest_verdicts.keys() {
		if work.contains_key(key) {
			continue;
		}
		if live_machine(db, &mut machines, key.0).await?.is_none() {
			continue;
		}
		work.entry(key.clone()).or_default();
	}

	// Grade every key into its server's instances, in a stable order so a
	// check's message and detail don't reshuffle between passes.
	let mut keys: Vec<ReplicaKey> = work.keys().cloned().collect();
	keys.sort_by_key(|(sid, r#type, intent, name)| {
		(
			*sid,
			r#type.to_string(),
			intent.to_string(),
			name.clone().unwrap_or_default(),
		)
	});

	// The two grains part here. Restore-verification and redaction are the
	// machine's: what failed to restore is the box's backup. Migration-test is
	// the application's: the version under test is that workload's, so a box
	// hosting two carries the finding against whichever of them it was for.
	// spec: RST#alerting
	let mut per_machine: HashMap<Uuid, MachineInstances> = HashMap::new();
	let mut machine_order: Vec<Uuid> = Vec::new();
	let mut per_application: HashMap<Uuid, Vec<CheckInstance>> = HashMap::new();
	let mut application_order: Vec<Uuid> = Vec::new();
	for key in keys {
		let w = &work[&key];
		let (mid, r#type, intent, declared_as) = (key.0, &key.1, &key.2, &key.3);
		let label = match declared_as {
			Some(name) => format!("{name} ({type} / {intent})"),
			None => format!("{type} / {intent}"),
		};
		let latest = if w.reports_count {
			latest_reports.get(&key)
		} else {
			None
		};
		let instances = per_machine.entry(mid).or_insert_with(|| {
			machine_order.push(mid);
			MachineInstances::default()
		});

		// A declaration for something other than migrating is a replica whose
		// restore health is expected; so is any key with a report to read.
		if (w.declared && !w.migrates) || latest.is_some() {
			instances
				.verification
				.push(verification_instance(&key, &label, w, latest));
		}
		if let Some(instance) = redaction_instance(&key, &label, latest) {
			instances.redaction.push(instance);
		}

		let verdict = latest_verdicts.get(&key);
		if let Some(instance) = migration_instance(&key, &label, w, verdict) {
			// The application the version belongs to: the verdict's own where
			// there is one, else the sole application on the box. A machine
			// running several with nothing recorded has no application the
			// finding can be attributed to, so it is not filed rather than
			// filed against a guess.
			let owner = match verdict.and_then(|v| v.application_id) {
				Some(aid) => Some(aid),
				None => match live_machine(db, &mut machines, mid).await? {
					Some(machine) => {
						let on_box = machine.applications(db).await?;
						match on_box.as_slice() {
							[only] => Some(only.id),
							_ => None,
						}
					}
					None => None,
				},
			};
			if let Some(aid) = owner {
				per_application.entry(aid).or_insert_with(|| {
					application_order.push(aid);
					Vec::new()
				});
				per_application
					.get_mut(&aid)
					.expect("just entered")
					.push(instance);
			}
		}
	}

	// Targets whose checks are open but which derived nothing this pass: their
	// last replica is gone, and a target nobody visits is a check left open with
	// nothing that could ever clear it. Each grain is revisited on its own
	// columns, since a machine check's issue is on `machine_id`.
	for mid in crate::backup::staleness::machines_with_open_checks(
		db,
		&[refs::RESTORE_VERIFICATION, refs::REDACTION],
	)
	.await?
	{
		if per_machine.contains_key(&mid) || live_machine(db, &mut machines, mid).await?.is_none() {
			continue;
		}
		per_machine.insert(mid, MachineInstances::default());
		machine_order.push(mid);
	}
	for aid in
		crate::backup::staleness::servers_with_open_checks(db, &[refs::MIGRATION_TEST]).await?
	{
		if per_application.contains_key(&aid) {
			continue;
		}
		let live = crate::applications::Application::get_by_id(db, aid)
			.await
			.ok()
			.filter(|a| a.deleted_at.is_none())
			.is_some();
		if !live {
			continue;
		}
		per_application.insert(aid, Vec::new());
		application_order.push(aid);
	}

	let mut degraded = 0usize;
	for mid in machine_order {
		let found = per_machine.remove(&mid).expect("entered with its machine");
		let label = match live_machine(db, &mut machines, mid).await? {
			Some(machine) => crate::backup::staleness::machine_label(machine),
			None => continue,
		};
		degraded += file_verification(db, mid, &label, found.verification).await?;
		degraded += file_redaction(db, mid, &label, found.redaction).await?;
	}
	for aid in application_order {
		let instances = per_application.remove(&aid).expect("entered with its app");
		let Ok(application) = crate::applications::Application::get_by_id(db, aid).await else {
			continue;
		};
		let label = crate::backup::staleness::server_label(&application);
		degraded += file_migration(db, aid, &label, instances).await?;
	}

	Ok(degraded)
}

/// A live machine by id, cached across the sweep. `None` when it is gone or
/// soft-deleted, in which case its keys are not worth deriving: nothing holds a
/// check against a machine that isn't there.
async fn live_machine<'a>(
	db: &mut AsyncPgConnection,
	cache: &'a mut HashMap<Uuid, crate::machines::Machine>,
	machine_id: Uuid,
) -> Result<Option<&'a crate::machines::Machine>> {
	if let Entry::Vacant(e) = cache.entry(machine_id) {
		match crate::machines::Machine::get_by_id(db, machine_id).await {
			Ok(machine) if machine.deleted_at.is_none() => {
				e.insert(machine);
			}
			_ => return Ok(None),
		}
	}
	Ok(cache.get(&machine_id))
}

/// Whether one replica key has gone past its bound, per its intent's semantics.
fn is_overdue(
	key: &ReplicaKey,
	bound: PgDuration,
	once: bool,
	now: Timestamp,
	healthy: &HashMap<ReplicaKey, Timestamp>,
	verified: &HashMap<ReplicaKey, String>,
	snapshots: &HashMap<(Uuid, BackupType), BackupRun>,
) -> bool {
	if !once {
		return match healthy.get(key) {
			Some(last) => now.duration_since(*last) > bound.0,
			None => true,
		};
	}

	// A `once` intent is overdue only when a snapshot exists to verify, it is
	// not the last one verified, and it has stood past the bound.
	match snapshots.get(&(key.0, key.1.clone())) {
		None => false,
		Some(run) => {
			let already = matches!(
				(verified.get(key), run.snapshot_id.as_ref()),
				(Some(v), Some(s)) if v == s
			);
			// Measured from the report, not `run.anchor()`: the question is how
			// long this snapshot has gone unverified since it became available
			// to verify, which is when it landed — not how old the data inside
			// it is.
			!already && now.duration_since(run.reported_at) > bound.0
		}
	}
}

/// Whether a `migrate` declaration's server has left its candidate version
/// untried past the bound, and against which snapshot.
///
/// `None` when there is nothing to be overdue about: no candidate version, no
/// snapshot to migrate, a verdict already recorded for the pair, or still inside
/// the bound. A recorded verdict reaches the check through
/// [`crate::migration_tests::latest_verdict_by_key`] instead, so it is not this
/// path's business.
// spec: RST#alerting
/// The machine's snapshot and the candidate of an application on it: the pair a
/// migration test is about. Where a box hosts several workloads with
/// candidates, the first in the machine's own order is the one asked about, and
/// the others follow on later sweeps as their tests settle.
// spec: RST#candidate-versions
async fn untried_candidate(
	db: &mut AsyncPgConnection,
	declaration: &RestoreReplica,
	machine: &crate::machines::Machine,
	snapshots: &HashMap<(Uuid, BackupType), BackupRun>,
	now: Timestamp,
) -> Result<Option<(String, String)>> {
	let Some(bound) = declaration.overdue_after else {
		return Ok(None);
	};
	let Some(run) = snapshots.get(&(machine.id, declaration.r#type.clone())) else {
		return Ok(None);
	};
	let Some(snapshot_id) = run.snapshot_id.as_ref() else {
		return Ok(None);
	};
	let mut candidate = None;
	for application in machine.applications(db).await? {
		if let Some(version) = crate::migration_tests::candidate_for(db, &application).await? {
			candidate = Some(version);
			break;
		}
	}
	let Some(version) = candidate else {
		return Ok(None);
	};
	if crate::migration_tests::has_verdict(db, machine.id, snapshot_id, version.id).await? {
		return Ok(None);
	}
	// Measured from when the snapshot landed, which is when it became available
	// to migrate, not how old the data inside it is.
	if now.duration_since(run.reported_at) <= bound.0 {
		return Ok(None);
	}
	Ok(Some((
		version.as_semver().to_string(),
		snapshot_id.to_owned(),
	)))
}

/// The fields every instance of a restore check carries, whichever check it is:
/// what identifies the replica, for an operator reading the detail and for a
/// rule or silence written against one replica.
///
/// `replica_key` is the two dimensions joined, because a rule condition takes
/// one variable and a silence for one replica has to pin both (see
/// [`crate::check_policies::Condition`]).
fn instance_identity(key: &ReplicaKey) -> serde_json::Value {
	let (_, r#type, intent, declared_as) = key;
	serde_json::json!({
		"type": r#type.to_string(),
		"intent": intent.to_string(),
		"replica_key": format!("{type}:{intent}"),
		"replica": declared_as,
	})
}

/// Merge `extra` into an instance's identity fields.
fn instance_detail(key: &ReplicaKey, extra: serde_json::Value) -> Option<serde_json::Value> {
	let mut detail = instance_identity(key);
	let (Some(object), Some(extra)) = (detail.as_object_mut(), extra.as_object()) else {
		return Some(detail);
	};
	for (k, v) in extra {
		object.insert(k.clone(), v.clone());
	}
	Some(detail)
}

/// One replica's restore-verification instance: the worse of what its latest
/// report said and whether it has gone past its bound.
fn verification_instance(
	key: &ReplicaKey,
	label: &str,
	work: &KeyWork,
	latest: Option<&BackupRestoreCheck>,
) -> CheckInstance {
	let reported_unhealthy =
		latest.is_some_and(|r| r.outcome != RunOutcome::Success || !r.replica_healthy);
	let observed = if reported_unhealthy || work.overdue {
		CheckResult::Failed
	} else {
		CheckResult::Passed
	};
	let why = if reported_unhealthy {
		latest
			.and_then(|r| r.error.clone())
			.unwrap_or_else(|| "restored database did not come up healthy".into())
	} else if work.overdue && work.once {
		"latest snapshot not verified within its overdue bound".into()
	} else if work.overdue {
		"no healthy restore verification within its overdue bound".into()
	} else {
		"healthy".into()
	};
	CheckInstance {
		label: label.to_owned(),
		observed,
		detail: instance_detail(
			key,
			serde_json::json!({
				"snapshot_id": latest.and_then(|r| r.snapshot_id.clone()),
				"overdue": work.overdue,
				"latest_snapshot_unverified": work.overdue && work.once,
				"why": why,
			}),
		),
	}
}

/// The redaction instance for one replica, from its latest report. `None` when
/// the replica has not reported a redaction outcome, since nothing observed is
/// not the same as redacted — and a declaration that redacts but has never
/// reported has produced no replica to be unmasked.
fn redaction_instance(
	key: &ReplicaKey,
	label: &str,
	latest: Option<&BackupRestoreCheck>,
) -> Option<CheckInstance> {
	let latest = latest?;
	let outcome = latest.redaction_outcome?;
	let observed = match outcome {
		RedactionOutcome::Complete => CheckResult::Passed,
		RedactionOutcome::Partial | RedactionOutcome::Failed => CheckResult::Warning,
	};
	Some(CheckInstance {
		label: label.to_owned(),
		observed,
		detail: instance_detail(
			key,
			serde_json::json!({
				"outcome": outcome.to_string(),
				"manifest_version": latest.redaction_manifest_version,
				"columns_masked": latest.redaction_columns_masked,
				"columns_skipped": latest.redaction_columns_skipped,
				"error": latest.redaction_error,
				"why": outcome.to_string(),
			}),
		),
	})
}

/// The migration-test instance for one replica: what its latest verdict said,
/// or that its candidate has gone untried past the bound. Untested and failed
/// are both "this version is not known good against this group's data", so
/// they are one instance and the more urgent of the two wins.
///
/// `None` when there is neither a verdict nor a bound gone past: a replica with
/// nothing to test is not a passing test.
// spec: RST#alerting
fn migration_instance(
	key: &ReplicaKey,
	label: &str,
	work: &KeyWork,
	verdict: Option<&crate::migration_tests::KeyVerdict>,
) -> Option<CheckInstance> {
	// The version named is whichever side answered: the verdict names the one it
	// was reached against, the bound names the candidate it is still waiting for.
	let (observed, why, target_version, snapshot, failed_migration) = match (verdict, &work.untried)
	{
		(Some(v), _) if v.verdict == crate::migration_tests::Verdict::Failed => (
			CheckResult::Warning,
			match &v.failed_migration {
				Some(migration) => {
					format!("migration {migration} failed applying {}", v.target_version)
				}
				None => format!(
					"the restore never got as far as migrating {}",
					v.target_version
				),
			},
			Some(v.target_version.clone()),
			v.snapshot_id.clone(),
			v.failed_migration.clone(),
		),
		(_, Some((version, snapshot))) => (
			CheckResult::Warning,
			format!("migrations for {version} not tried within the overdue bound"),
			Some(version.clone()),
			Some(snapshot.clone()),
			None,
		),
		(Some(v), None) => (
			CheckResult::Passed,
			format!("migrations for {} applied", v.target_version),
			Some(v.target_version.clone()),
			v.snapshot_id.clone(),
			None,
		),
		(None, None) => return None,
	};

	Some(CheckInstance {
		label: label.to_owned(),
		observed,
		detail: instance_detail(
			key,
			serde_json::json!({
				"target_version": target_version,
				"failed_migration": failed_migration,
				"snapshot_id": snapshot,
				"why": why,
			}),
		),
	})
}

async fn file_verification(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	label: &str,
	instances: Vec<CheckInstance>,
) -> Result<usize> {
	let total = instances.len();
	file_restore_check(
		db,
		Scope::Machine(machine_id),
		RestoreCheck {
			r#ref: refs::RESTORE_VERIFICATION,
			documentation: refs::RESTORE_VERIFICATION_DOC,
			title: "restore verification failed",
			gone: &format!("No restore replica of {label} is tracked any more"),
		},
		instances,
		&|degraded| match degraded {
			[] => format!("Every restore replica of {label} is verifying healthily"),
			[one] => format!(
				"Restore verification failed for {label}: {} — {}",
				one.label,
				instance_why(one),
			),
			many => format!(
				"Restore verification failed for {} of {label}'s {total} replicas: {}",
				many.len(),
				instance_labels(many),
			),
		},
	)
	.await
}

async fn file_redaction(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	label: &str,
	instances: Vec<CheckInstance>,
) -> Result<usize> {
	let total = instances.len();
	file_restore_check(
		db,
		Scope::Machine(machine_id),
		RestoreCheck {
			r#ref: refs::REDACTION,
			documentation: refs::REDACTION_DOC,
			title: "redaction incomplete",
			gone: &format!("No redacting replica of {label} is tracked any more"),
		},
		instances,
		&|degraded| match degraded {
			[] => format!("Every redacting replica of {label} is fully masked"),
			[one] => format!(
				"Replica {} of {label} did not fully redact: {}",
				one.label,
				instance_field(one, "outcome"),
			),
			many => format!(
				"{} of {label}'s {total} redacting replicas did not fully redact: {}",
				many.len(),
				instance_labels(many),
			),
		},
	)
	.await
}

async fn file_migration(
	db: &mut AsyncPgConnection,
	application_id: Uuid,
	label: &str,
	instances: Vec<CheckInstance>,
) -> Result<usize> {
	let total = instances.len();
	file_restore_check(
		db,
		Scope::Application(application_id),
		RestoreCheck {
			r#ref: refs::MIGRATION_TEST,
			documentation: refs::MIGRATION_TEST_DOC,
			title: "candidate version not known good",
			gone: &format!("No candidate version is under test against {label}'s data"),
		},
		instances,
		&|degraded| match degraded {
			[] => format!("Candidate versions have been migration-tested against {label}"),
			[one] => format!(
				"Candidate version not clean against {label}'s data: {} — {}",
				one.label,
				instance_why(one),
			),
			many => format!(
				"Candidate versions not clean against {label}'s data for {} of {total} replicas: {}",
				many.len(),
				instance_labels(many),
			),
		},
	)
	.await
}

/// The fixed parts of one restore check: what it is called, the documentation it
/// ships with, its headline when degraded, and what it says once a server has no
/// instances of it left.
struct RestoreCheck<'a> {
	r#ref: &'a str,
	documentation: &'a str,
	title: &'a str,
	gone: &'a str,
}

/// File one of a server's restore checks from its instances, and say whether it
/// came out degraded.
///
/// Nothing is filed for a check that has nothing degraded and nothing open: a
/// server that has never had one of these findings does not need a passing row
/// and a catalog entry for it. A check that *is* open and has run out of
/// instances is recovered on its own — with no instances there is nothing left
/// to grade, so it is filed as the plain passing check it has become rather
/// than left open with nothing that could ever clear it.
async fn file_restore_check(
	db: &mut AsyncPgConnection,
	scope: Scope,
	check: RestoreCheck<'_>,
	instances: Vec<CheckInstance>,
	message: &(dyn Fn(&[GradedInstance]) -> String + Sync),
) -> Result<usize> {
	let RestoreCheck {
		r#ref,
		documentation,
		title,
		gone,
	} = check;
	let any_degraded = instances.iter().any(|i| i.observed != CheckResult::Passed);
	// The gate reads the column the check's own grain files on: a machine
	// check's issue is on `machine_id`, so asking after an application id would
	// never find it and the recovery would never be filed.
	let already_open = match scope {
		Scope::Machine(mid) => {
			crate::backup::staleness::open_machine_issue_active(db, mid, r#ref).await?
		}
		Scope::Application(aid) => {
			crate::backup::staleness::open_server_issue_active(db, aid, r#ref).await?
		}
		_ => unreachable!("restore checks file at machine or application scope only"),
	};
	if !any_degraded && !already_open {
		return Ok(0);
	}

	let issue = if instances.is_empty() {
		crate::issues::file_check(
			db,
			crate::issues::CheckFiling {
				source: crate::statuses::CANOPY_SOURCE,
				scope,
				device_id: None,
				check: r#ref,
				observed: CheckResult::Passed,
				title: None,
				message: gone,
				detail: None,
				default_ceiling: CheckResult::Warning,
				default_escalates: false,
				documentation: Some(documentation),
			},
		)
		.await?
	} else {
		file_check_instances(
			db,
			InstancedCheckFiling {
				source: crate::statuses::CANOPY_SOURCE,
				scope,
				device_id: None,
				check: r#ref,
				title: Some(title),
				instances,
				default_ceiling: CheckResult::Warning,
				default_escalates: false,
				documentation: Some(documentation),
			},
			message,
		)
		.await?
	};

	Ok(usize::from(issue.active))
}

/// A degraded instance's `why`, for a single-instance message.
fn instance_why(instance: &GradedInstance) -> String {
	instance_field(instance, "why")
}

fn instance_field(instance: &GradedInstance, field: &str) -> String {
	instance
		.detail
		.as_ref()
		.and_then(|d| d.get(field))
		.and_then(|v| v.as_str())
		.unwrap_or("no detail reported")
		.to_owned()
}

fn instance_labels(instances: &[GradedInstance]) -> String {
	instances
		.iter()
		.map(|i| i.label.as_str())
		.collect::<Vec<_>>()
		.join(", ")
}
