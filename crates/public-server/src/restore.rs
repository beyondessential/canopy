//! Managed-restore endpoints (RST) — the consumer-facing side of the restore
//! control plane. All `BackupRestoreDevice`-authenticated, mounted at the root:
//!
//! - `POST /restore-capabilities` — the consumer registers the intents it can
//!   satisfy. Canopy dispatches only matching worklist entries.
//! - `GET  /restore-worklist` — the consumer's complete desired state: its
//!   enabled declarations expanded per current server, each carrying the
//!   snapshot Canopy wants restored and the repo coordinates to find it.
//! - `POST /restore-credentials` — short-lived **read-only** S3 creds plus the
//!   repo password for one `(group, type)` the consumer is authorized for.
//!
//! The `backup-restore` role is read-only by construction: it cannot reach the
//! `ServerDevice`-gated `/backup-credentials`, and `/restore-credentials` only
//! ever issues the read-only [`restore_session_policy`].

use aws_sdk_sts::operation::RequestId as _;
use axum::{Json, extract::State, http::StatusCode};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::BackupRestoreDevice;
use commons_types::backup::{
	BackupPurpose, BackupType, IntentDescriptor, ParamValues, RedactionOutcome, RestoreIntent,
	RunOutcome, redaction_params, resolve_params, semantics,
};
use commons_types::server::app_type::{ApplicationType, RedactionManifest};
use database::{
	Db,
	backups::{BackupRun, NewBackupCredentialIssuance, ServerGroupBackupConfig},
	migration_tests::{self, MigrationTest, NewMigrationTest},
	pg_duration::PgDuration,
	reporting_schemas::{NewReportingSchemaBuild, ReportingSchemaBuild},
	restore::{
		BackupRestoreCheck, NewBackupRestoreCheck, RestoreConsumerCapability, RestoreReplica,
	},
};
use diesel_async::AsyncPgConnection;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::{
	backup::{
		CredentialProcessOutput, REPO_PASSWORD_SECRET_KEY, instance_default_region,
		restore_session_policy,
	},
	state::{AppState, BackupSecrets},
};

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(capabilities))
		.routes(routes!(worklist))
		.routes(routes!(credentials))
		.routes(routes!(verification))
}

// ---------------------------------------------------------------------------
// POST /restore-capabilities
// ---------------------------------------------------------------------------

/// Request body for registering the restore intents a consumer device can
/// satisfy.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestoreCapabilitiesArgs {
	/// The intents this device can satisfy — arbitrary consumer-chosen
	/// identifiers (e.g. `verify`) — each with its description, the semantics
	/// it opts into, and the schema of the parameters it accepts. Replaces the
	/// device's previously advertised set wholesale.
	pub intents: Vec<IntentDescriptor>,
}

/// Register the restore intents this device can satisfy.
///
/// Declares the restore intents the calling device supports, replacing any
/// previously advertised set. Only worklist entries whose intent is currently
/// advertised are dispatched to this device via `GET /restore-worklist`, so
/// register on startup and whenever the supported set changes.
#[utoipa::path(
	post,
	path = "/restore-capabilities",
	operation_id = "register_restore_capabilities",
	tag = "restore",
	security(("backup-restore-device" = [])),
	request_body = RestoreCapabilitiesArgs,
	responses((status = 204, description = "Capability set registered.")),
)]
async fn capabilities(
	State(db): State<Db>,
	device: BackupRestoreDevice,
	Json(args): Json<RestoreCapabilitiesArgs>,
) -> Result<StatusCode> {
	let mut conn = db.get().await?;
	let consumer_device_id = device.0.0.id;
	RestoreConsumerCapability::register(&mut conn, consumer_device_id, &args.intents).await?;
	Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// GET /restore-worklist
// ---------------------------------------------------------------------------

/// One replica the consumer device should currently maintain: an operator
/// declaration expanded against a single server, carrying the snapshot to
/// restore and the repository coordinates to find it. S3 credentials and the
/// repository passphrase are obtained separately via
/// `POST /restore-credentials`.
#[derive(Debug, Serialize, ToSchema)]
pub struct WorklistEntry {
	/// Identifier of the declaration this entry was expanded from. Echo it
	/// back in `POST /restore-verification` reports.
	pub replica_id: Uuid,
	/// The server group whose backup repository holds the snapshot.
	pub group_id: Uuid,
	/// The machine whose backup should be restored. Echo it back in reports.
	pub machine_id: Uuid,
	/// The same machine under the name this field carried when a server was a
	/// box and the software on it at once.
	///
	/// Deprecated in favour of `machine_id`, and emitted so a consumer built
	/// against the earlier shape keeps working across the transition. Every
	/// machine that predates the split took its application's id, so for those
	/// the two values are equal; a machine created since has no server to be.
	// spec: API#renaming-a-field
	#[deprecated(note = "use `machine_id`")]
	#[schema(deprecated)]
	pub server_id: Uuid,
	/// For a `migrate` entry, the type of application whose candidate version is
	/// under test. Absent on any other entry.
	///
	/// A snapshot is a machine's and a candidate version is an application's, so
	/// a migration test names both: it restores the machine's data and applies
	/// that application's next version's migrations to it. The workload is named
	/// by its type, which is what the reporter itself said it was; Canopy's own
	/// identifier for an application is internal and never on the wire.
	// spec: RST#candidate-versions
	#[serde(skip_serializing_if = "Option::is_none")]
	#[schema(value_type = Option<String>)]
	pub application_type: Option<ApplicationType>,
	/// The backup type to restore (e.g. `tamanu-postgres`).
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// The restore intent this entry is for; one of the intents this device
	/// advertised via `POST /restore-capabilities`.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// Operator-assigned label for the declaration.
	pub name: String,
	/// Bound, in whole seconds, after which the replica counts as overdue;
	/// `null` means no bound. Interpreted per the intent's semantics: for a
	/// run-once (`once`) intent, how long the latest snapshot may go without a
	/// healthy verification report; for a standing replica, how stale its last
	/// healthy report may be.
	pub overdue_after_seconds: Option<i64>,
	/// Resolved parameter values for this replica: one key per parameter the
	/// intent advertises. Parameters the operator left unset carry the
	/// intent's declared default, or JSON `null` when there is none.
	#[schema(value_type = Object)]
	pub params: ParamValues,
	/// Identifier of the snapshot to restore — the latest successful backup
	/// for this server and type. `null` when no successful backup is known
	/// yet.
	pub snapshot_id: Option<String>,
	/// When that snapshot was reported, as an RFC 3339 timestamp; `null` if
	/// unknown.
	pub snapshot_at: Option<String>,
	/// Kind of storage backend. Always `"s3"`.
	pub storage: String,
	/// Name of the S3 bucket holding the group's backup repository.
	pub bucket: String,
	/// Key prefix within the bucket under which the repository lives. Normally
	/// empty (the repository is at the bucket root).
	pub prefix: String,
	/// AWS region of the bucket.
	pub region: String,
	/// For a `migrate` intent, the version whose schema migrations to apply
	/// after restoring. Obtain them from that version's published artefacts, the
	/// same way a server being upgraded does. `null` for every other intent.
	pub target_version: Option<String>,
	/// Identifier of that version. Echo it back in the migration-test report.
	pub target_version_id: Option<Uuid>,
}

/// Fetch the full set of replicas this device should maintain.
///
/// Returns the device's complete desired state, computed fresh on every call:
/// each enabled restore declaration whose intent this device currently
/// advertises, expanded into one entry per server it covers. A group-wide
/// declaration expands to every live server in its group; a server-scoped
/// declaration yields a single entry and takes precedence over a group-wide
/// one covering the same server, type, and intent. Entries for groups whose
/// backup configuration is not ready are omitted, and entries for run-once
/// intents disappear once the latest snapshot has a healthy verification
/// report, reappearing when a newer snapshot exists.
///
/// An empty array means there is nothing to do. Poll this endpoint and
/// reconcile: create or refresh the replicas listed, and tear down any the
/// device is maintaining that no longer appear.
#[utoipa::path(
	get,
	path = "/restore-worklist",
	tag = "restore",
	security(("backup-restore-device" = [])),
	responses((status = 200, body = Vec<WorklistEntry>)),
)]
async fn worklist(
	State(db): State<Db>,
	device: BackupRestoreDevice,
) -> Result<Json<Vec<WorklistEntry>>> {
	let mut conn = db.get().await?;
	let consumer_device_id = device.0.0.id;

	// Only intents the consumer currently advertises are dispatched; a
	// declaration on an unadvertised intent is a gap, surfaced to operators,
	// never sent here. Keep the descriptors to read semantics and parameter
	// schemas per intent.
	let descriptors: HashMap<RestoreIntent, IntentDescriptor> =
		RestoreConsumerCapability::list_for_consumer(&mut conn, consumer_device_id)
			.await?
			.into_iter()
			.map(|d| (d.intent.clone(), d))
			.collect();

	let mut declarations = RestoreReplica::list_enabled_for_consumer(&mut conn, consumer_device_id)
		.await?
		.into_iter()
		.filter(|d| descriptors.contains_key(&d.intent))
		.collect::<Vec<_>>();
	// Process machine-specific declarations before group-wide ones so entries
	// arrive in a stable order whatever the declarations' creation order.
	declarations.sort_by_key(|d| d.machine_id.is_none());

	let mut out: Vec<WorklistEntry> = Vec::new();
	// One entry per named replica per machine. Several declarations may cover one
	// (machine, type, intent) — a raw one and a redacted one, a nightly one and a
	// weekly one — and are told apart by name, so the name is what the dedup
	// keys on. A group-wide and a machine-scoped declaration with different names
	// are two replicas of that machine, and both are dispatched.
	let mut seen: HashSet<(Uuid, String)> = HashSet::new();
	// Per-group caches so a group referenced by several declarations is resolved
	// once: the latest produced snapshot per (machine, type), and the latest
	// healthy-verified snapshot per (machine, type, intent) for `once` suppression.
	let mut snapshot_cache: HashMap<Uuid, HashMap<(Uuid, BackupType), BackupRun>> = HashMap::new();
	let mut verified_cache: HashMap<Uuid, HashMap<database::restore::ReplicaKey, String>> =
		HashMap::new();

	for d in declarations {
		// A worklist entry needs somewhere to restore from: skip groups without
		// a ready config (they surface elsewhere as not-yet-restorable).
		let Some(cfg) = ServerGroupBackupConfig::get(&mut conn, d.group_id).await? else {
			continue;
		};
		if cfg.status != commons_types::backup::BackupConfigStatus::Ready {
			continue;
		}

		// A declaration covers machines: what gets restored is a snapshot, and a
		// snapshot is what a machine backed up.
		// spec: RST#declared-replicas
		let machines = match d.machine_id {
			Some(mid) => {
				let m = database::machines::Machine::get_by_id(&mut conn, mid).await?;
				// Skip a declaration whose machine has left the group or been
				// archived; it lingers as a no-op until the operator retires it.
				if m.group_id == Some(d.group_id) && m.deleted_at.is_none() {
					vec![m]
				} else {
					vec![]
				}
			}
			None => database::machines::Machine::list_for_group(&mut conn, d.group_id).await?,
		};

		if let std::collections::hash_map::Entry::Vacant(e) = snapshot_cache.entry(d.group_id) {
			e.insert(
				BackupRun::latest_success_by_machine_type_for_group(&mut conn, d.group_id).await?,
			);
			verified_cache.insert(
				d.group_id,
				BackupRestoreCheck::latest_healthy_snapshot_by_key_for_group(&mut conn, d.group_id)
					.await?,
			);
		}
		let snapshots = &snapshot_cache[&d.group_id];
		let verified = &verified_cache[&d.group_id];

		// The descriptor governs the intent's semantics and parameter resolution.
		let descriptor = &descriptors[&d.intent];
		let once = descriptor.has_semantic(semantics::ONCE);
		let migrates = descriptor.has_semantic(semantics::MIGRATE);
		let owns_masking = descriptor.has_semantic(semantics::REDACT);
		let builds_schema = descriptor.has_semantic(semantics::REPORTING_SCHEMA);
		let replica_values: ParamValues =
			serde_json::from_value(d.params.clone()).unwrap_or_default();
		let params = resolve_params(&descriptor.params, &replica_values);

		let region = cfg.region.clone().unwrap_or_else(instance_default_region);

		// A build is dispatched per pair rather than per machine. The
		// configuration a schema follows from is held centrally, so every pair
		// of a group restores the same central's snapshot and differs only in
		// the version it is migrated to.
		// spec: RPT#the-build-contract
		if builds_schema {
			// Masking alters the configuration a schema follows from, so a
			// redacting declaration builds nothing rather than building from a
			// database that is no longer the group's.
			if d.redacts {
				continue;
			}

			let members =
				database::applications::Application::list_live_in_group(&mut conn, d.group_id)
					.await?;
			let Some(central) = database::server_groups::ServerGroup::canonical_central(&members)
			else {
				continue;
			};
			let central_type = central.r#type.clone();
			let machine =
				database::machines::Machine::get_by_id(&mut conn, central.machine_id).await?;
			let latest = snapshots.get(&(machine.id, d.r#type.clone()));

			for version in
				database::reporting_schemas::versions_for_group(&mut conn, d.group_id).await?
			{
				if once
					&& database::reporting_schemas::ReportingSchemaBuild::is_settled(
						&mut conn, d.group_id, version.id,
					)
					.await?
				{
					continue;
				}

				#[expect(deprecated, reason = "emitted for consumers on the earlier shape")]
				out.push(WorklistEntry {
					replica_id: d.id,
					group_id: d.group_id,
					machine_id: machine.id,
					server_id: machine.id,
					application_type: Some(central_type.clone()),
					r#type: d.r#type.clone(),
					intent: d.intent.clone(),
					name: d.name.clone(),
					overdue_after_seconds: d.overdue_after.map(|f| f.0.as_secs()),
					params: params.clone(),
					snapshot_id: latest.and_then(|r| r.snapshot_id.clone()),
					snapshot_at: latest.map(|r| r.reported_at.to_string()),
					storage: "s3".into(),
					bucket: cfg.bucket.clone(),
					prefix: cfg.prefix.clone(),
					region: region.clone(),
					target_version: Some(version.as_semver().to_string()),
					target_version_id: Some(version.id),
				});
			}

			continue;
		}

		for machine in machines {
			let key = (machine.id, d.name.clone());
			if !seen.insert(key) {
				continue;
			}
			let latest = snapshots.get(&(machine.id, d.r#type.clone()));
			let on_box = machine.applications(&mut conn).await?;

			// A `migrate` intent needs a version to migrate to, so a machine
			// none of whose applications has a candidate contributes nothing
			// rather than an entry naming none. The entry names the application
			// whose candidate it carries alongside the machine whose snapshot it
			// restores: the one place the two grains interleave.
			// spec: RST#dispatching-a-migration-test
			let target = if migrates {
				let mut found = None;
				for application in &on_box {
					if let Some(version) =
						migration_tests::candidate_for(&mut conn, application).await?
					{
						found = Some((application.r#type.clone(), version.id, version.as_semver()));
						break;
					}
				}
				match found {
					Some(t) => Some(t),
					None => continue,
				}
			} else {
				None
			};

			// The masking parameters are Canopy's for a `redact` intent: resolved
			// from the server's product when the declaration redacts, sent unset
			// when it doesn't. A redacting declaration contributes nothing for a
			// server that can't be redacted — an unredacted replica standing in
			// for a redacted one is worse than no replica.
			let params = if owns_masking {
				let manifest = if d.redacts {
					// What to mask is a property of the software in the snapshot,
					// so it resolves through the applications on the box.
					// spec: RST#the-masking-manifest
					match on_box.iter().find_map(|a| a.r#type.caps().redaction) {
						Some(manifest) => Some(manifest),
						None => continue,
					}
				} else {
					None
				};
				masked_params(&params, manifest)
			} else {
				params.clone()
			};

			// A `once` intent drops off the worklist once its work is settled for
			// the latest snapshot, and reappears only when a newer one exists. For
			// a `migrate` intent that settling is keyed to the target version too,
			// and a failure settles it as firmly as a pass.
			if once {
				let settled = match (&target, latest.and_then(|r| r.snapshot_id.as_ref())) {
					(Some((_, version_id, _)), Some(snapshot)) => {
						migration_tests::has_verdict(&mut conn, machine.id, snapshot, *version_id)
							.await?
					}
					(Some(_), None) => false,
					(None, snapshot) => {
						// Keyed by name as well: each named replica of a scope
						// verifies its own snapshot, so one of them settling does
						// not take its siblings off the worklist.
						let key = (
							machine.id,
							d.r#type.clone(),
							d.intent.clone(),
							Some(d.name.clone()),
						);
						matches!((verified.get(&key), snapshot), (Some(v), Some(s)) if v == s)
					}
				};
				if settled {
					continue;
				}
			}
			#[expect(deprecated, reason = "emitted for consumers on the earlier shape")]
			out.push(WorklistEntry {
				replica_id: d.id,
				group_id: d.group_id,
				machine_id: machine.id,
				server_id: machine.id,
				application_type: target.as_ref().map(|(ty, _, _)| ty.clone()),
				r#type: d.r#type.clone(),
				intent: d.intent.clone(),
				name: d.name.clone(),
				overdue_after_seconds: d.overdue_after.map(|f| f.0.as_secs()),
				params,
				snapshot_id: latest.and_then(|r| r.snapshot_id.clone()),
				snapshot_at: latest.map(|r| r.reported_at.to_string()),
				storage: "s3".into(),
				bucket: cfg.bucket.clone(),
				prefix: cfg.prefix.clone(),
				region: region.clone(),
				target_version: target.as_ref().map(|(_, _, version)| version.to_string()),
				target_version_id: target.as_ref().map(|(_, version_id, _)| *version_id),
			});
		}
	}

	Ok(Json(out))
}

// ---------------------------------------------------------------------------
// POST /restore-credentials
// ---------------------------------------------------------------------------

/// Request body for minting read-only restore credentials.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RestoreCredentialsArgs {
	/// The server group whose backup repository to read.
	pub group: Uuid,
	/// The backup type to restore (e.g. `tamanu-postgres`).
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// This must be the run-uuid the client minted for this run.
	/// The field is optional only so older clients don't break; it WILL be made
	/// mandatory in future.
	pub run_id: Option<Uuid>,
}

/// Read-only S3 credentials plus the repository passphrase for one group and
/// backup type: everything needed to open the group's backup repository and
/// read a snapshot out of it.
#[derive(Debug, Serialize, ToSchema)]
pub struct RestoreCredentials {
	/// Temporary read-only AWS credentials in the `credential_process` output
	/// format, valid for at most one hour.
	pub credentials: CredentialProcessOutput,
	/// Passphrase for the group's backup repository (a Kopia repository).
	pub repo_password: String,
}

/// Mint read-only credentials for a group's backup repository.
///
/// Issues temporary AWS credentials — always strictly read-only, scoped to the
/// group's backup storage — together with the repository passphrase, so the
/// device can read the snapshot named in a worklist entry. Credentials expire
/// after at most one hour; request a fresh set per restore rather than caching
/// them. Every issuance is recorded for audit.
///
/// The device must hold an enabled restore declaration covering the requested
/// group and type (i.e. the pair must appear in its worklist configuration);
/// otherwise the request is rejected with 403.
///
/// Errors: 403 when no enabled declaration authorizes this group and type;
/// 409 when the group has no ready backup configuration; 502 when the
/// credential issuer or the passphrase store is unavailable or not configured.
#[utoipa::path(
	post,
	path = "/restore-credentials",
	operation_id = "mint_restore_credentials",
	tag = "restore",
	security(("backup-restore-device" = [])),
	request_body = RestoreCredentialsArgs,
	responses(
		(status = 200, body = RestoreCredentials),
		(status = 403, description = "No enabled declaration authorizes this (group, type).", body = ProblemDetailsSchema),
		(status = 409, description = "Group has no ready backup config.", body = ProblemDetailsSchema),
		(status = 502, description = "STS issuance or repo-password read failed or is not configured.", body = ProblemDetailsSchema),
	),
)]
async fn credentials(
	State(db): State<Db>,
	State(sts): State<Option<aws_sdk_sts::Client>>,
	State(kube): State<Option<BackupSecrets>>,
	device: BackupRestoreDevice,
	Json(args): Json<RestoreCredentialsArgs>,
) -> Result<Json<RestoreCredentials>> {
	let mut conn = db.get().await?;
	let consumer_device_id = device.0.0.id;

	// Authorization is the declared replica: a consumer may read exactly the
	// (group, type) pairs its enabled declarations cover.
	if !RestoreReplica::authorizes(&mut conn, consumer_device_id, args.group, &args.r#type).await? {
		return Err(AppError::AuthInsufficientPermissions {
			required: "an enabled restore-replica declaration for this group and type".into(),
		});
	}

	let cfg = ServerGroupBackupConfig::get(&mut conn, args.group)
		.await?
		.ok_or_else(|| AppError::Conflict("group has no backup config".into()))?;
	if cfg.status != commons_types::backup::BackupConfigStatus::Ready {
		return Err(AppError::Conflict(
			"group backup config is not ready".into(),
		));
	}

	// Always read-only — this role cannot mint write creds.
	let session_policy = restore_session_policy(&cfg.bucket, &cfg.prefix);

	let Some(sts) = sts else {
		tracing::error!(group = %args.group, "restore-credentials: STS client not configured");
		return Err(AppError::Upstream(
			"credential issuer not configured".into(),
		));
	};

	let session_name = format!("canopy-restore-{consumer_device_id}");
	let resp = sts
		.assume_role()
		.role_arn(&cfg.target_role_arn)
		.role_session_name(session_name)
		.policy(session_policy)
		.duration_seconds(3600)
		.send()
		.await
		.map_err(|err| {
			let request_id = err.request_id().unwrap_or("<none>");
			tracing::error!(
				group = %args.group,
				role = %cfg.target_role_arn,
				request_id,
				error = ?err,
				"restore-credentials: AssumeRole failed",
			);
			AppError::Upstream("credential issuance failed".into())
		})?;

	let sts_request_id = resp.request_id().map(str::to_owned);
	let creds = resp.credentials().ok_or_else(|| {
		tracing::error!(group = %args.group, "restore-credentials: AssumeRole returned no credentials");
		AppError::Upstream("credential issuance returned no credentials".into())
	})?;

	let expiry_secs = creds.expiration().secs();
	let expires_at = Timestamp::from_second(expiry_secs).map_err(|err| {
		tracing::error!(group = %args.group, error = ?err, "restore-credentials: bad expiration");
		AppError::Upstream("credential issuance returned an invalid expiration".into())
	})?;
	let access_key_id = creds.access_key_id().to_owned();

	let Some(kube) = kube else {
		tracing::error!(group = %args.group, "restore-credentials: kube client not configured");
		return Err(AppError::Upstream("secret store not configured".into()));
	};
	let repo_password = kube
		.read_password(&cfg.repo_password_ref, REPO_PASSWORD_SECRET_KEY)
		.await
		.map_err(|err| {
			tracing::error!(
				group = %args.group,
				secret = %cfg.repo_password_ref,
				error = ?err,
				"restore-credentials: reading repo-password Secret failed",
			);
			AppError::Upstream("repo password unavailable".into())
		})?;

	// Audit BEFORE returning — never hand out creds we didn't record.
	database::backups::BackupCredentialIssuance::record(
		&mut conn,
		NewBackupCredentialIssuance {
			device_id: consumer_device_id,
			group_id: args.group,
			r#type: args.r#type.clone(),
			expires_at,
			purpose: BackupPurpose::Restore,
			sts_assumed_role: cfg.target_role_arn.clone(),
			sts_request_id,
			access_key_id: Some(access_key_id.clone()),
			bucket: cfg.bucket.clone(),
			prefix: cfg.prefix.clone(),
			run_id: args.run_id,
		},
	)
	.await?;

	Ok(Json(RestoreCredentials {
		credentials: CredentialProcessOutput {
			version: 1,
			access_key_id,
			secret_access_key: creds.secret_access_key().to_owned(),
			session_token: creds.session_token().to_owned(),
			expiration: expires_at.to_string(),
		},
		repo_password,
	}))
}

// ---------------------------------------------------------------------------
// POST /restore-verification
// ---------------------------------------------------------------------------

/// Report of a restore attempt and the health of the resulting replica.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VerificationArgs {
	/// The declaration this report concerns, taken from the worklist entry's
	/// `replica_id`. Required: several replicas can share one group, machine,
	/// type, and intent, so a report that named no declaration could not be
	/// attributed to one of them.
	pub replica_id: Uuid,
	/// The server group whose backup was restored.
	pub group: Uuid,
	/// The machine whose backup was restored, from the worklist entry's
	/// `machine_id`.
	///
	/// Optional only so a reporter built against the earlier shape, which knew
	/// this as `server_id`, is still accepted; one of the two must be present.
	pub machine_id: Option<Uuid>,
	/// The same machine under the name this field carried when a server was a
	/// box and the software on it at once.
	///
	/// Deprecated in favour of `machine_id`. A report naming only this is
	/// accepted and read as the machine, since a machine that predates the
	/// split took its application's id. Naming both is an error rather than a
	/// silent preference, because a reporter that disagrees with itself about
	/// what it restored has not been understood.
	// spec: API#renaming-a-field
	#[deprecated(note = "use `machine_id`")]
	#[schema(deprecated)]
	pub server_id: Option<Uuid>,
	/// The backup type that was restored (e.g. `tamanu-postgres`).
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// The restore intent this attempt was performed under.
	#[schema(value_type = String)]
	pub intent: RestoreIntent,
	/// Identifier of the snapshot that was restored. Omit on a failure that
	/// never got as far as selecting a snapshot.
	pub snapshot_id: Option<String>,
	/// Whether the restore succeeded (`success`) or failed (`failure`).
	#[schema(value_type = String)]
	pub outcome: RunOutcome,
	/// Human-readable error detail, when the restore failed.
	pub error: Option<String>,
	/// Whether the restored database came up healthy and passed readiness
	/// checks. A replica only counts as verified when the outcome is
	/// `success` and this is `true`.
	pub replica_healthy: bool,
	/// Version of the PostgreSQL server the data was restored into, if
	/// applicable.
	pub postgres_version: Option<String>,
	/// When the restore result was observed, as an RFC 3339 timestamp.
	#[schema(value_type = String)]
	pub observed_at: Timestamp,
	/// Bytes of raw HTTP traffic sent to S3 during the restore, including
	/// protocol and signing overhead. Omit when traffic was not measured.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Bytes of decoded object payload sent to S3 during the restore.
	pub s3_sent_payload_bytes: Option<i64>,
	/// Bytes of raw HTTP traffic received from S3 during the restore,
	/// including protocol overhead.
	pub s3_received_raw_bytes: Option<i64>,
	/// Bytes of decoded object payload received from S3 during the restore.
	pub s3_received_payload_bytes: Option<i64>,
	/// Arbitrary structured health data to record alongside the report
	/// (database statistics, whether indexes needed rebuilding, and so on).
	/// Stored and displayed as-is.
	pub health_details: Option<serde_json::Value>,
	/// This must be the run-uuid the client minted for this run.
	/// The field is optional only so older clients don't break; it WILL be made
	/// mandatory in future.
	pub run_id: Option<Uuid>,
	/// What the migrations did, for a report under a `migrate` intent. Omit for
	/// every other intent.
	pub migration: Option<MigrationArgs>,
	/// What a reporting-schema build produced, where the replica was restored
	/// for one. Absent on any other report.
	// spec: RPT#what-a-build-reports
	pub reporting_schema: Option<ReportingSchemaArgs>,
	/// What the masking manifest did, for a replica that redacts. Omit for a
	/// replica that doesn't.
	pub redaction: Option<RedactionArgs>,
}

/// Overlay Canopy's masking parameters onto an intent's resolved values.
///
/// Canopy owns these for any intent carrying `redact`, so whatever the
/// declaration stored for them is replaced: by the product's manifest when
/// the replica redacts, and by nothing when it doesn't. Sending them unset
/// is what tells the consumer not to redact, so this has to overwrite rather
/// than fill in.
// spec: RST#the-masking-manifest
fn masked_params(resolved: &ParamValues, manifest: Option<RedactionManifest>) -> ParamValues {
	let mut params = resolved.clone();
	for name in redaction_params::ALL {
		// Only parameters the intent advertises are sent, so an intent that
		// carries `redact` without accepting one of these doesn't gain it.
		if !params.contains_key(*name) {
			continue;
		}
		let value = match (manifest, *name) {
			(None, _) => Value::Null,
			(Some(m), redaction_params::MANIFEST_URL) => Value::from(m.url_template),
			(Some(m), redaction_params::VERSION_QUERY) => Value::from(m.version_query),
			(Some(m), redaction_params::VERSION_FALLBACK_TO_BASE) => {
				Value::from(m.fallback_to_base)
			}
			(Some(_), _) => unreachable!("every owned parameter is resolved"),
		};
		params.insert((*name).to_string(), value);
	}
	params
}

/// How the masking manifest went against the restored replica.
///
/// Reported when the redaction settles, which for a failure is before any
/// switchover: the restore itself succeeded and is reported healthy, and the
/// replica stays on the data it was already serving.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RedactionArgs {
	/// How far the manifest got: `complete`, `partial`, or `failed`.
	#[schema(value_type = String)]
	pub outcome: RedactionOutcome,
	/// The version resolved into the manifest URL. Omit when the URL named no
	/// version to resolve.
	pub manifest_version: Option<String>,
	/// How many columns the manifest masked.
	pub columns_masked: Option<i64>,
	/// How many columns the manifest named but could not mask. Non-zero is
	/// what makes an outcome `partial`.
	pub columns_skipped: Option<i64>,
	/// Why the redaction failed, when it did.
	pub error: Option<String>,
}

/// How the target version's migrations went against the restored replica.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MigrationArgs {
	/// The version whose migrations were applied, as semver, taken from the
	/// worklist entry's `target_version`. This is the version the consumer
	/// actually migrated to; send it in preference to echoing the identifier.
	pub target_version: Option<String>,
	/// The version whose migrations were applied, as the identifier a consumer
	/// echoes from the worklist entry's `target_version_id`. Accepted only for
	/// older consumers that report the identifier; omit it when `target_version`
	/// is sent.
	pub target_version_id: Option<Uuid>,
	/// The type of application whose candidate version was tried, echoed from
	/// the worklist entry's `application_type`. Omitted by a consumer that
	/// predates the entry carrying it, in which case Canopy derives the
	/// application from the machine and the version.
	#[schema(value_type = Option<String>)]
	pub application_type: Option<ApplicationType>,
	/// Whole seconds the whole migration run took.
	pub total_elapsed_seconds: i64,
	/// The migration that failed, when one did.
	pub failed_migration: Option<String>,
	/// Size of the data before the migrations ran.
	pub data_bytes_before: i64,
	/// Size of the data after they ran. The growth between the two is what shows
	/// a migration that backfills heavily.
	pub data_bytes_after: i64,
	/// One entry per migration that ran, in the order they ran.
	pub timings: Vec<MigrationTimingArgs>,
}

/// What a reporting-schema build reports beyond its replica's restore health.
// spec: RPT#what-a-build-reports
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReportingSchemaArgs {
	/// The version the schema was built for, as semver, echoed from the
	/// worklist entry's `target_version`.
	pub target_version: Option<String>,
	/// The same version as the identifier, echoed from `target_version_id`.
	/// Accepted for a consumer that reports the identifier; omit it when
	/// `target_version` is sent.
	pub target_version_id: Option<Uuid>,
	/// Whether a schema came out of the build.
	pub built: bool,
	/// What went wrong, where the build failed.
	pub error: Option<String>,
	/// The artifacts the build registered, of which the schema is one.
	#[serde(default)]
	pub artifacts: Vec<Uuid>,
}

/// How long one migration took.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MigrationTimingArgs {
	/// The migration's name, as the migration runner reports it.
	pub name: String,
	/// Whole seconds it took.
	pub elapsed_seconds: i64,
}

/// Report the outcome of a restore attempt and the replica's health.
///
/// Records a verification report for a restore the device performed from its
/// worklist. Send one report per attempt, on success and on failure alike. A
/// report with a `success` outcome and `replica_healthy: true` marks the
/// snapshot as verified; for run-once intents this is what removes the entry
/// from `GET /restore-worklist` until a newer snapshot appears.
///
/// Authorization matches `POST /restore-credentials`: the device must hold an
/// enabled restore declaration covering the reported group and type,
/// otherwise the request is rejected with 403.
///
/// The report names the declaration it is about, and that declaration must
/// still exist and belong to the calling consumer. A replica nothing declares
/// any more is not one Canopy tracks, so a report naming a retired declaration
/// is refused rather than recorded against a replica that could never recover.
#[utoipa::path(
	post,
	path = "/restore-verification",
	tag = "restore",
	security(("backup-restore-device" = [])),
	request_body = VerificationArgs,
	responses(
		(status = 204, description = "Report recorded."),
		(status = 403, description = "No enabled declaration authorizes this (group, type), or the named declaration belongs to another consumer.", body = ProblemDetailsSchema),
		(status = 404, description = "The named declaration does not exist.", body = ProblemDetailsSchema),
	),
)]
async fn verification(
	State(db): State<Db>,
	device: BackupRestoreDevice,
	Json(args): Json<VerificationArgs>,
) -> Result<StatusCode> {
	let mut conn = db.get().await?;
	let consumer_device_id = device.0.0.id;

	// `server_id` is what this was called before a machine and the software on
	// it were separate records. A reporter still on that shape is understood;
	// one naming both fields is not, since the two disagreeing about what was
	// restored leaves nothing to prefer.
	#[expect(deprecated, reason = "accepted from reporters on the earlier shape")]
	let machine_id = match (args.machine_id, args.server_id) {
		(Some(machine), None) | (None, Some(machine)) => machine,
		(Some(_), Some(_)) => {
			return Err(AppError::BadRequest(
				"name the restored machine once: `machine_id`, or `server_id` on the earlier shape"
					.into(),
			));
		}
		(None, None) => {
			return Err(AppError::BadRequest(
				"the restored machine is required: send `machine_id`".into(),
			));
		}
	};

	// Same authorization as credentials: a consumer may report only on the
	// (group, type) pairs its enabled declarations cover.
	if !RestoreReplica::authorizes(&mut conn, consumer_device_id, args.group, &args.r#type).await? {
		return Err(AppError::AuthInsufficientPermissions {
			required: "an enabled restore-replica declaration for this group and type".into(),
		});
	}

	// The report says which replica it is about, and only its own consumer may
	// speak for it: the name resolved from here is what separates a scope's
	// replicas, so a report attributed to the wrong one would grade a replica
	// on a restore that was never its.
	let declaration = RestoreReplica::get(&mut conn, args.replica_id)
		.await
		.map_err(|_| AppError::NotFound("no such restore replica declaration".into()))?;
	if declaration.consumer_device_id != consumer_device_id {
		return Err(AppError::AuthInsufficientPermissions {
			required: "the named restore-replica declaration to be this consumer's own".into(),
		});
	}

	let report = NewBackupRestoreCheck {
		replica_id: Some(args.replica_id),
		// Resolved from `replica_id` when the report is recorded.
		replica_name: None,
		consumer_device_id,
		group_id: args.group,
		machine_id: Some(machine_id),
		r#type: args.r#type,
		intent: args.intent,
		snapshot_id: args.snapshot_id,
		outcome: args.outcome,
		error: args.error,
		replica_healthy: args.replica_healthy,
		postgres_version: args.postgres_version,
		observed_at: args.observed_at,
		s3_sent_raw_bytes: args.s3_sent_raw_bytes,
		s3_sent_payload_bytes: args.s3_sent_payload_bytes,
		s3_received_raw_bytes: args.s3_received_raw_bytes,
		s3_received_payload_bytes: args.s3_received_payload_bytes,
		health_details: args.health_details,
		run_id: args.run_id,
		redaction_outcome: args.redaction.as_ref().map(|r| r.outcome),
		redaction_manifest_version: args
			.redaction
			.as_ref()
			.and_then(|r| r.manifest_version.clone()),
		redaction_columns_masked: args.redaction.as_ref().and_then(|r| r.columns_masked),
		redaction_columns_skipped: args.redaction.as_ref().and_then(|r| r.columns_skipped),
		redaction_error: args.redaction.as_ref().and_then(|r| r.error.clone()),
	};

	match (args.migration, args.reporting_schema) {
		// A build rides the migrate pathway, so a report may carry both; the
		// build is the one that settles the pair.
		(_, Some(build)) => {
			let version_id = resolve_build_target(&mut conn, &build).await?;
			// The build is held against the group's central application, which is
			// the one whose database the schema followed from and the one the
			// entry named.
			// spec: RPT#alerting
			let members =
				database::applications::Application::list_live_in_group(&mut conn, args.group)
					.await?;
			let application_id =
				database::server_groups::ServerGroup::canonical_central(&members).map(|a| a.id);
			ReportingSchemaBuild::record(
				&mut conn,
				report,
				NewReportingSchemaBuild {
					group_id: args.group,
					version_id,
					application_id,
					built: build.built,
					error: build.error,
					artifact_ids: build.artifacts,
				},
			)
			.await?;
		}
		(Some(migration), None) => {
			let target_version_id = resolve_migration_target(&mut conn, &migration).await?;
			let application_id =
				resolve_migration_application(&mut conn, &migration, machine_id, target_version_id)
					.await?;
			MigrationTest::record(
				&mut conn,
				report,
				migration.into_new(target_version_id, application_id),
			)
			.await?;
		}
		(None, None) => {
			BackupRestoreCheck::record_report(&mut conn, report).await?;
		}
	}

	Ok(StatusCode::NO_CONTENT)
}

/// Resolve the version a migration report is about.
///
/// A report may name it as a semver — the going-forward form, what the consumer
/// actually migrated to — or as the `target_version_id` an older consumer
/// echoes from its worklist entry. The semver wins when both are present. A
/// report naming neither, or a semver matching no known version, cannot be
/// attributed to a version and is refused.
async fn resolve_migration_target(
	conn: &mut AsyncPgConnection,
	migration: &MigrationArgs,
) -> Result<Uuid> {
	if let Some(semver) = &migration.target_version {
		return Ok(
			database::versions::Version::get_by_version(conn, semver.parse()?)
				.await?
				.id,
		);
	}
	migration
		.target_version_id
		.ok_or_else(|| AppError::BadRequest("migration report names no target version".into()))
}

/// Resolve the version a reporting-schema build is about.
///
/// The semver is preferred, matching a migration report: it is what the entry
/// carried and what the builder actually built for.
async fn resolve_build_target(
	conn: &mut AsyncPgConnection,
	build: &ReportingSchemaArgs,
) -> Result<Uuid> {
	if let Some(semver) = &build.target_version {
		return Ok(
			database::versions::Version::get_by_version(conn, semver.parse()?)
				.await?
				.id,
		);
	}
	build
		.target_version_id
		.ok_or_else(|| AppError::BadRequest("build report names no version".into()))
}

/// Resolve the application a migration report is about.
///
/// The version under test is an application's candidate while the data is the
/// machine's snapshot, so the report has to say which workload on the box it
/// was for. A consumer echoes the `application_type` from its worklist entry;
/// one that predates the entry carrying it gets the application derived — the
/// one on the machine whose candidate is that version, or the sole application
/// where a box runs only one.
// spec: RST#candidate-versions
async fn resolve_migration_application(
	conn: &mut AsyncPgConnection,
	migration: &MigrationArgs,
	machine_id: Uuid,
	target_version_id: Uuid,
) -> Result<Uuid> {
	let machine = database::machines::Machine::get_by_id(conn, machine_id).await?;
	let on_box = machine.applications(conn).await?;
	if let Some(named) = &migration.application_type
		&& let Some(application) = on_box.iter().find(|a| &a.r#type == named)
	{
		return Ok(application.id);
	}
	if let [only] = on_box.as_slice() {
		return Ok(only.id);
	}
	for application in &on_box {
		if let Some(version) = migration_tests::candidate_for(conn, application).await?
			&& version.id == target_version_id
		{
			return Ok(application.id);
		}
	}
	Err(AppError::BadRequest(
		"migration report names no application, and this machine runs several with no candidate \
		 matching the version reported"
			.into(),
	))
}

impl MigrationArgs {
	fn into_new(self, target_version_id: Uuid, application_id: Uuid) -> NewMigrationTest {
		NewMigrationTest {
			application_id,
			target_version_id,
			total_elapsed: PgDuration(SignedDuration::from_secs(self.total_elapsed_seconds)),
			failed_migration: self.failed_migration,
			data_bytes_before: self.data_bytes_before,
			data_bytes_after: self.data_bytes_after,
			timings: self
				.timings
				.into_iter()
				.map(|timing| {
					(
						timing.name,
						PgDuration(SignedDuration::from_secs(timing.elapsed_seconds)),
					)
				})
				.collect(),
		}
	}
}
