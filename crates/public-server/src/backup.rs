//! Device backup endpoints — the on-demand credential-minting path of the
//! backup-credentials system.
//!
//! Five `ServerDevice`-authenticated endpoints, all mounted at the root:
//!
//! - `POST /backup-capabilities` — bestool registers the backup types it can
//!   run on this server.
//! - `POST /backup-credentials` — mint short-lived per-group S3 creds via a
//!   cross-account `sts:AssumeRole`, returned as `credential_process` JSON.
//! - `GET  /backup-target` — `{storage, bucket, prefix, region, repo_password}`
//!   so bestool can reconstruct the kopia repo connection on every run.
//! - `POST /backup-progress` — record a sample from a run still in flight into
//!   `backup_run_progress`.
//! - `POST /backup-report` — record a run outcome into `backup_runs`.
//!
//! They all resolve `device → live server → group_id → group backup config`
//! identically: **412** when the device is bound to no live server, **409**
//! when the server is ungrouped / has no `ready` config / (for a backup) the
//! type is neither an enabled capability nor has a pending request / (for a
//! restore) the machine's restore window isn't open, **502** when STS or kube
//! fails or isn't configured.
//!
//! `/backup-progress` and `/backup-report` deliberately stop at *grouped*: they
//! describe a run that is already happening, so gating them on a ready config
//! would blind Canopy exactly when a group is misconfigured. `/backup-progress`
//! additionally answers **429**, being the only endpoint a client calls on a
//! cadence of its own choosing.

use aws_sdk_sts::operation::RequestId as _;
use axum::{Json, extract::State, http::StatusCode};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::ServerDevice;
use commons_types::backup::{BackupConfigStatus, BackupPurpose, BackupType, RunOutcome};
use database::{
	Db,
	backups::BackupTypeDefault,
	backups::{
		BackupRequest, MachineBackupCapability, NewBackupCredentialIssuance, NewBackupRun,
		ServerGroupBackupConfig,
	},
	machines::Machine,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::{AppState, BackupSecrets};

/// The data key inside the repo-password k8s Secret. The onboarding component
/// that *creates* the Secret must store the kopia passphrase under this key.
/// Single source of truth for the key name.
pub const REPO_PASSWORD_SECRET_KEY: &str = "password";

/// Fallback AWS region served by `GET /backup-target` when the group config's
/// `region` is NULL. Read from `AWS_REGION` (the EKS pod always has it), with a
/// last-resort default so the endpoint always returns a concrete region string.
pub(crate) fn instance_default_region() -> String {
	std::env::var("AWS_REGION")
		.or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
		.unwrap_or_else(|_| "us-east-1".to_string())
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(public: capabilities))
		.routes(routes!(public: credentials))
		.routes(routes!(public: target))
		.routes(routes!(public: progress))
		.routes(routes!(public: report))
}

// ---------------------------------------------------------------------------
// Shared resolution: device → live server → group_id → ready config
// ---------------------------------------------------------------------------

/// Resolve the authenticated identity to the box it is enrolled as.
/// No machine ⇒ 412 (`DeviceHasNoServer`).
///
/// Resolution stops here and never reaches the applications on the box, so a
/// box running two workloads resolves exactly as one running a single workload.
// spec: BAK
async fn resolve_machine(
	conn: &mut database::diesel_async::AsyncPgConnection,
	device_id: Uuid,
) -> Result<Machine> {
	Machine::get_by_device_id(conn, device_id)
		.await?
		.ok_or(AppError::DeviceHasNoServer)
}

/// Read the machine's `group_id`; ungrouped ⇒ 409.
fn require_group(machine: &Machine) -> Result<Uuid> {
	machine
		.group_id
		.ok_or_else(|| AppError::Conflict("machine is not in a group".into()))
}

/// Load the group's backup config and require it be `ready`. Absent or
/// non-`ready` ⇒ 409 (dormant gate).
async fn require_ready_config(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Uuid,
) -> Result<ServerGroupBackupConfig> {
	let cfg = ServerGroupBackupConfig::get(conn, group_id)
		.await?
		.ok_or_else(|| AppError::Conflict("group has no backup config".into()))?;
	if cfg.status != BackupConfigStatus::Ready {
		return Err(AppError::Conflict(
			"group backup config is not ready".into(),
		));
	}
	// Both callers hand a device what it needs to start a backup — the
	// short-lived S3 credentials, and the repository passphrase. Neither may
	// go out while a rotation is in flight: `kopia change-password` rewrites
	// the format blob, so a run started now would fail partway with a
	// passphrase that stopped working under it.
	//
	// A marker older than the rotation window is a rotation whose process
	// died; it's ignored (and logged), so a crash can't block a group's
	// backups indefinitely.
	if let Some(since) = cfg.repo_password_rotating_since {
		let age = Timestamp::now().duration_since(since);
		if age < database::backups::ROTATION_WINDOW {
			return Err(AppError::BackupRotationInProgress);
		}
		tracing::warn!(
			group = %group_id,
			?age,
			"ignoring a stale passphrase-rotation marker; a rotation likely died without clearing it",
		);
	}
	Ok(cfg)
}

/// The per-`(server, type)` **backup**-issuance gate. Mirrors what the heartbeat
/// is willing to ask the device to back up: a type may be issued backup creds
/// when it's an enabled capability (on the auto-schedule) or has a pending
/// operator "backup now" request. Neither ⇒ 409.
async fn require_backupable_capability(
	conn: &mut database::diesel_async::AsyncPgConnection,
	machine_id: Uuid,
	r#type: &BackupType,
) -> Result<()> {
	let enabled = MachineBackupCapability::list_for_machine(conn, machine_id)
		.await?
		.into_iter()
		.any(|c| &c.r#type == r#type && c.enabled);
	if enabled || BackupRequest::exists(conn, machine_id, r#type, BackupPurpose::Backup).await? {
		Ok(())
	} else {
		Err(AppError::Conflict(format!(
			"backup type {type} is not an enabled capability for this server, and no backup is pending",
			type = r#type
		)))
	}
}

/// The **restore**-issuance gate: an operator must have opened the machine's
/// time-boxed restore window. Restore creds are read access to the group's
/// entire backup history, so an ad-hoc `bestool canopy restore` self-authorizes
/// only while that deliberate window is open — it is not always available.
/// (Canopy still drives *automated* restores separately, via the PGRO
/// restore-replica path.) Window closed or expired ⇒ 409.
// spec: BAK#the-restore-window
fn require_restore_allowed(machine: &Machine) -> Result<()> {
	if machine.restore_allowed() {
		Ok(())
	} else {
		Err(AppError::Conflict(
			"restores are not currently allowed for this machine; \
			 enable restores for it in canopy (they stay open for 24 hours)"
				.into(),
		))
	}
}

// ---------------------------------------------------------------------------
// POST /backup-capabilities
// ---------------------------------------------------------------------------

/// Request body for registering the backup types a server can run.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BackupCapabilitiesArgs {
	/// The backup types this server is able to run. Each type is a plain
	/// string (e.g. `tamanu-postgres`); custom type names are accepted.
	#[schema(value_type = Vec<String>)]
	pub types: Vec<BackupType>,
}

/// Register the backup types this server can run.
///
/// Declares the set of backup types the calling device is able to execute on
/// its server. Types not seen before are added to the server's capability set,
/// starting out enabled or disabled according to the fleet-wide default for
/// that type (disabled if the type has no default configured). Types already
/// registered keep whatever enabled/disabled state an operator has set for
/// them, so re-registering on every startup is safe and expected.
///
/// Only types that are registered here (and enabled) can later be issued
/// credentials via `POST /backup-credentials`, outside of explicit
/// operator-requested runs.
///
/// Errors: 412 when the calling device is not bound to a live server; 409 when
/// the server is not in a group.
#[utoipa::path(
	post,
	path = "/backup-capabilities",
	operation_id = "register_backup_capabilities",
	tag = "backup",
	security(("server-device" = [])),
	request_body = BackupCapabilitiesArgs,
	responses(
		(status = 204, description = "Capabilities registered."),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
		(status = 409, description = "Application is not in a group.", body = ProblemDetailsSchema),
	),
)]
async fn capabilities(
	State(db): State<Db>,
	device: ServerDevice,
	Json(args): Json<BackupCapabilitiesArgs>,
) -> Result<StatusCode> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;

	let machine = resolve_machine(&mut conn, device_id).await?;
	// A capability is per-server; the group is required so the server is a
	// real, grouped member (matches the other endpoints' 409 on ungrouped).
	require_group(&machine)?;

	for r#type in &args.types {
		// Seed `enabled` from the type's auto_enable default for newly-seen
		// types; existing rows keep their operator-set `enabled`.
		let seed = BackupTypeDefault::get(&mut conn, r#type)
			.await?
			.map(|d| d.auto_enable)
			.unwrap_or(false);
		MachineBackupCapability::register(&mut conn, machine.id, r#type, seed).await?;
	}

	Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /backup-credentials
// ---------------------------------------------------------------------------

/// Request body for minting short-lived S3 credentials for a backup or
/// restore run.
#[derive(Debug, Deserialize, ToSchema)]
pub struct BackupCredentialsArgs {
	/// The backup type the credentials are for (e.g. `tamanu-postgres`). For a
	/// `backup`, the type must be an enabled capability of this server or the
	/// subject of a pending "backup now" request; for a `restore`, the server's
	/// restore window must be open (an operator allows restores for it in
	/// canopy). Otherwise the request is rejected with 409.
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// What the credentials will be used for. `backup` (the default) grants
	/// write access for uploading backups; `restore` grants strictly read-only
	/// access. Either way the credentials are scoped to the group's backup
	/// storage only.
	#[serde(default)]
	pub purpose: BackupPurpose,
	/// This must be the run-uuid the client minted for this run.
	/// The field is optional only so older clients don't break; it WILL be made
	/// mandatory in future.
	pub run_id: Option<Uuid>,
}

/// Short-lived AWS credentials in the AWS `credential_process` output format,
/// so they can be consumed directly by AWS SDKs and tools. Field names use the
/// exact casing (`Version`, `AccessKeyId`, ...) that format requires.
#[derive(Debug, Serialize, ToSchema)]
#[serde(rename_all = "PascalCase")]
pub struct CredentialProcessOutput {
	/// Version of the `credential_process` format. Always the literal `1`.
	pub version: u8,
	/// The temporary AWS access key ID.
	pub access_key_id: String,
	/// The temporary AWS secret access key.
	pub secret_access_key: String,
	/// The session token that must accompany the temporary key pair.
	pub session_token: String,
	/// When the credentials expire, as an RFC 3339 / ISO 8601 UTC instant.
	/// Credentials last at most one hour; request a fresh set per run.
	#[schema(format = DateTime)]
	pub expiration: String,
}

/// Build the read-only **restore** session policy for a bucket/prefix. ANDs
/// down to read-only against the per-bucket role. `GetBucketLocation` is its
/// own unconditioned statement (the `s3:prefix` context key isn't populated for
/// it, so folding it under the prefix condition would silently deny it).
pub fn restore_session_policy(bucket: &str, prefix: &str) -> String {
	serde_json::json!({
		"Version": "2012-10-17",
		"Statement": [
			{
				"Effect": "Allow",
				"Action": ["s3:GetObject"],
				"Resource": format!("arn:aws:s3:::{bucket}/{prefix}*"),
			},
			{
				"Effect": "Allow",
				"Action": ["s3:GetBucketLocation"],
				"Resource": format!("arn:aws:s3:::{bucket}"),
			},
			{
				"Effect": "Allow",
				"Action": ["s3:ListBucket"],
				"Resource": format!("arn:aws:s3:::{bucket}"),
				"Condition": { "StringLike": { "s3:prefix": [format!("{prefix}*")] } },
			},
		],
	})
	.to_string()
}

/// Build the **backup** session policy for a bucket/prefix. Grants the kopia
/// multipart write set **plus `s3:DeleteObject`** — but **no**
/// `s3:DeleteObjectVersion`, `s3:BypassGovernanceRetention`, or
/// `s3:PutObjectRetention` — scoped to this one bucket. kopia needs delete to
/// manage its own metadata (session markers, index compaction, manifests); on
/// our versioned + GOVERNANCE-locked buckets a version-less `DeleteObject` only
/// writes a delete-marker — the locked version stays immutable and is reclaimed
/// by the bucket lifecycle once its lock lapses — so a compromised device still
/// can't destroy backups (it can at most write recoverable delete-markers).
/// Attached to every backup issuance: redundant for a dedicated per-bucket role
/// (external), but the linchpin of shared-account isolation (the shared device
/// role is broad over `bes-canopy-backup-*`, so the session policy is what
/// confines a device's creds to its own bucket). `GetBucketLocation`/
/// `ListBucketMultipartUploads` are unconditioned bucket-level; `ListBucket` is
/// prefix-conditioned (the `s3:prefix` key isn't populated for the others).
pub fn backup_session_policy(bucket: &str, prefix: &str) -> String {
	serde_json::json!({
		"Version": "2012-10-17",
		"Statement": [
			{
				"Effect": "Allow",
				"Action": [
					"s3:GetObject",
					"s3:PutObject",
					"s3:DeleteObject",
					"s3:AbortMultipartUpload",
					"s3:ListMultipartUploadParts",
				],
				"Resource": format!("arn:aws:s3:::{bucket}/{prefix}*"),
			},
			{
				"Effect": "Allow",
				"Action": ["s3:GetBucketLocation", "s3:ListBucketMultipartUploads"],
				"Resource": format!("arn:aws:s3:::{bucket}"),
			},
			{
				"Effect": "Allow",
				"Action": ["s3:ListBucket"],
				"Resource": format!("arn:aws:s3:::{bucket}"),
				"Condition": { "StringLike": { "s3:prefix": [format!("{prefix}*")] } },
			},
		],
	})
	.to_string()
}

/// Mint short-lived S3 credentials for a backup or restore run.
///
/// Issues temporary AWS credentials scoped to the backup storage of the
/// calling server's group, in the `credential_process` output format. With
/// `purpose: backup` the credentials can upload and manage backup data but
/// cannot destroy existing backups; with `purpose: restore` they are strictly
/// read-only. They expire after at most one hour, so request a fresh set for
/// each run rather than caching them. Every issuance is recorded for audit.
///
/// The storage coordinates the credentials apply to (bucket, prefix, region)
/// come from `GET /backup-target`.
///
/// Errors: 409 when the server is not in a group, when the group's backup
/// configuration is not ready, when a `backup` type is neither an enabled
/// capability of this server nor the subject of a pending "backup now" request,
/// or when a `restore` is requested but the server's restore window is not
/// open; 412 when the device is not bound to a live server; 502 when the
/// credential issuer is unavailable or not configured.
#[utoipa::path(
	post,
	path = "/backup-credentials",
	operation_id = "mint_backup_credentials",
	tag = "backup",
	security(("server-device" = [])),
	request_body = BackupCredentialsArgs,
	responses(
		(status = 200, body = CredentialProcessOutput),
		(status = 409, description = "Application ungrouped, no ready config, backup type not enabled, or restore window not open.", body = ProblemDetailsSchema),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
		(status = 502, description = "STS issuance failed or is not configured.", body = ProblemDetailsSchema),
	),
)]
async fn credentials(
	State(db): State<Db>,
	State(sts): State<Option<aws_sdk_sts::Client>>,
	device: ServerDevice,
	Json(args): Json<BackupCredentialsArgs>,
) -> Result<Json<CredentialProcessOutput>> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;

	let machine = resolve_machine(&mut conn, device_id).await?;
	let group_id = require_group(&machine)?;
	let cfg = require_ready_config(&mut conn, group_id).await?;
	match args.purpose {
		BackupPurpose::Backup => {
			require_backupable_capability(&mut conn, machine.id, &args.r#type).await?
		}
		BackupPurpose::Restore => require_restore_allowed(&machine)?,
	}

	// Always attach a bucket-scoped session policy so the issued creds can only
	// reach this group's bucket — redundant for a dedicated per-bucket role
	// (external placement), essential for the shared device role (shared).
	let session_policy = match args.purpose {
		BackupPurpose::Restore => restore_session_policy(&cfg.bucket, &cfg.prefix),
		BackupPurpose::Backup => backup_session_policy(&cfg.bucket, &cfg.prefix),
	};

	let Some(sts) = sts else {
		tracing::error!(group = %group_id, "backup-credentials: STS client not configured");
		return Err(AppError::Upstream(
			"credential issuer not configured".into(),
		));
	};

	let session_name = format!("canopy-{purpose}-{device_id}", purpose = args.purpose);

	let resp = sts
		.assume_role()
		.role_arn(&cfg.target_role_arn)
		.role_session_name(session_name)
		.policy(session_policy)
		// Chained sessions cap at 1h regardless; ask for the max.
		.duration_seconds(3600)
		.send()
		.await
		.map_err(|err| {
			let request_id = err.request_id().unwrap_or("<none>");
			tracing::error!(
				group = %group_id,
				role = %cfg.target_role_arn,
				request_id,
				error = ?err,
				"backup-credentials: AssumeRole failed",
			);
			AppError::Upstream("credential issuance failed".into())
		})?;

	let sts_request_id = resp.request_id().map(str::to_owned);

	let creds = resp.credentials().ok_or_else(|| {
		tracing::error!(group = %group_id, "backup-credentials: AssumeRole returned no credentials");
		AppError::Upstream("credential issuance returned no credentials".into())
	})?;

	// STS `Expiration` is a smithy DateTime → convert to a jiff Timestamp for
	// the audit row, and emit RFC3339 `Z` for the credential_process output.
	let expiry_secs = creds.expiration().secs();
	let expires_at = Timestamp::from_second(expiry_secs).map_err(|err| {
		tracing::error!(group = %group_id, error = ?err, "backup-credentials: bad expiration");
		AppError::Upstream("credential issuance returned an invalid expiration".into())
	})?;

	let access_key_id = creds.access_key_id().to_owned();

	// Audit insert BEFORE returning — never hand out creds we didn't record.
	database::backups::BackupCredentialIssuance::record(
		&mut conn,
		NewBackupCredentialIssuance {
			device_id,
			group_id,
			r#type: args.r#type.clone(),
			expires_at,
			purpose: args.purpose,
			sts_assumed_role: cfg.target_role_arn.clone(),
			sts_request_id,
			access_key_id: Some(access_key_id.clone()),
			bucket: cfg.bucket.clone(),
			prefix: cfg.prefix.clone(),
			run_id: args.run_id,
		},
	)
	.await?;

	Ok(Json(CredentialProcessOutput {
		version: 1,
		access_key_id,
		secret_access_key: creds.secret_access_key().to_owned(),
		session_token: creds.session_token().to_owned(),
		expiration: expires_at.to_string(),
	}))
}

// ---------------------------------------------------------------------------
// GET /backup-target
// ---------------------------------------------------------------------------

/// The backup storage target for the calling server's group: where the backup
/// repository lives and the passphrase to open it.
#[derive(Debug, Serialize, ToSchema)]
pub struct BackupTarget {
	/// Kind of storage backend. Always `"s3"`.
	pub storage: String,
	/// Name of the S3 bucket holding the group's backup repository.
	pub bucket: String,
	/// Key prefix within the bucket under which the repository lives. Normally
	/// empty (the repository is at the bucket root).
	pub prefix: String,
	/// AWS region of the bucket.
	pub region: String,
	/// Passphrase for the group's backup repository (a Kopia repository).
	pub repo_password: String,
}

/// Fetch the backup storage target for this server's group.
///
/// Returns the bucket, prefix, region, and repository passphrase the device
/// needs to connect to its group's backup repository. Call it on every run
/// rather than caching the result, as the target can change. S3 credentials
/// are obtained separately via `POST /backup-credentials`.
///
/// Errors: 409 when the server is not in a group or the group's backup
/// configuration is not ready; 412 when the device is not bound to a live
/// server; 502 when the passphrase store is unavailable or not configured.
#[utoipa::path(
	get,
	path = "/backup-target",
	tag = "backup",
	security(("server-device" = [])),
	responses(
		(status = 200, body = BackupTarget),
		(status = 409, description = "Application ungrouped or no ready config.", body = ProblemDetailsSchema),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
		(status = 502, description = "Repo-password Secret unavailable or kube not configured.", body = ProblemDetailsSchema),
	),
)]
async fn target(
	State(db): State<Db>,
	State(kube): State<Option<BackupSecrets>>,
	device: ServerDevice,
) -> Result<Json<BackupTarget>> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;

	let machine = resolve_machine(&mut conn, device_id).await?;
	let group_id = require_group(&machine)?;
	let cfg = require_ready_config(&mut conn, group_id).await?;

	let Some(kube) = kube else {
		tracing::error!(group = %group_id, "backup-target: kube client not configured");
		return Err(AppError::Upstream("secret store not configured".into()));
	};

	let repo_password = kube
		.read_password(&cfg.repo_password_ref, REPO_PASSWORD_SECRET_KEY)
		.await
		.map_err(|err| {
			tracing::error!(
				group = %group_id,
				secret = %cfg.repo_password_ref,
				error = ?err,
				"backup-target: reading repo-password Secret failed",
			);
			AppError::Upstream("repo password unavailable".into())
		})?;

	let region = cfg.region.clone().unwrap_or_else(instance_default_region);

	Ok(Json(BackupTarget {
		storage: "s3".into(),
		bucket: cfg.bucket,
		prefix: cfg.prefix,
		region,
		repo_password,
	}))
}

// ---------------------------------------------------------------------------
// POST /backup-progress
// ---------------------------------------------------------------------------

/// Progress reports accepted per device within [`PROGRESS_RL_WINDOW`]. Generous
/// against any sane sampling cadence (a 5s cadence fits inside it), tight enough
/// that a stuck client can't flood the table. Cadence itself is the client's to
/// pick; Canopy only caps it.
const PROGRESS_RL_PER_DEVICE: u32 = 60;
const PROGRESS_RL_WINDOW: std::time::Duration = std::time::Duration::from_secs(300);

/// A progress sample from a run still in flight.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ProgressArgs {
	/// The run-uuid the client minted for this run — the same one it passes to
	/// `POST /backup-credentials` and reports under at `POST /backup-report`.
	pub run_id: Uuid,
	/// The backup type being run (e.g. `tamanu-postgres`).
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Whether this is a `backup` or a `restore` run.
	#[serde(default)]
	pub purpose: BackupPurpose,
	/// When this run froze the data it is backing up — the point in time the
	/// backup represents, as opposed to when its upload finishes. Send it as soon
	/// as it is known (before any transfer starts). Recorded once per run: the
	/// first value Canopy sees stands, whether it arrives here or on the report.
	pub snapshot_taken_at: Option<Timestamp>,
	/// Source bytes read so far.
	pub bytes_read: Option<i64>,
	/// Bytes processed (hashed, compressed) so far.
	pub bytes_hashed: Option<i64>,
	/// Bytes uploaded to the repository so far.
	pub bytes_uploaded: Option<i64>,
	/// Bytes found already present in the repository, and so not re-uploaded.
	pub bytes_cached: Option<i64>,
	/// Total bytes this run currently expects to handle. May be revised upward.
	pub bytes_estimated: Option<i64>,
	/// Files finished so far.
	pub files_done: Option<i64>,
	/// Total files this run currently expects to handle.
	pub files_estimated: Option<i64>,
	/// Errors hit so far.
	pub errors: Option<i64>,
	/// Errors hit and deliberately ignored so far.
	pub ignored_errors: Option<i64>,
	/// What the run is working on right now, for display.
	pub current_path: Option<String>,
	/// Bytes of raw HTTP traffic sent to S3 so far, including protocol and
	/// signing overhead.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Bytes of decoded object payload sent to S3 so far.
	pub s3_sent_payload_bytes: Option<i64>,
	/// Bytes of raw HTTP traffic received from S3 so far.
	pub s3_received_raw_bytes: Option<i64>,
	/// Bytes of decoded object payload received from S3 so far.
	pub s3_received_payload_bytes: Option<i64>,
	/// Any further detail the backup engine emits. Canopy makes no commitment
	/// about its shape: it is stored and shown verbatim, never interpreted.
	#[serde(default)]
	#[schema(value_type = Object)]
	pub extra: serde_json::Value,
}

/// Report progress for a run that is still in flight.
///
/// Optional throughout: a run that never reports progress is recorded and
/// displayed exactly as it is today. Reporting it lets Canopy show how far a
/// long-running backup has got, at what rate, and when it last heard from the
/// device — which for a multi-hour backup is the difference between "running"
/// and "running, and moving".
///
/// **Every counter is cumulative from the start of the run**, not an interval
/// delta. Send totals-so-far each time. A dropped or repeated report then costs
/// only resolution, never the accuracy of a total, and the last report Canopy
/// received can stand in for a figure the final report omits. Omit any counter
/// you do not measure rather than sending zero.
///
/// Canopy timestamps each report on receipt, so no clock agreement is needed —
/// except for `snapshot_taken_at`, which is necessarily the device's own claim
/// about its filesystem.
///
/// Unlike `POST /backup-credentials`, this does not require the group's backup
/// configuration to be ready or the type to be an enabled capability: it
/// describes a run already under way, and refusing it would blind Canopy exactly
/// when something is misconfigured.
///
/// A refused report is never a reason to abandon a run — this is telemetry.
/// Reporting progress for a run that has already been reported complete is
/// accepted rather than refused, so a report racing the completion is not an
/// error.
///
/// Errors: 412 when the calling device is not bound to a live server; 409 when
/// the server is not in a group; 429 when reporting faster than Canopy accepts.
#[utoipa::path(
	post,
	path = "/backup-progress",
	operation_id = "report_backup_progress",
	tag = "backup",
	security(("server-device" = [])),
	request_body = ProgressArgs,
	responses(
		(status = 204, description = "Progress recorded."),
		(status = 409, description = "Application is not in a group.", body = ProblemDetailsSchema),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
		(status = 429, description = "Reporting progress too frequently.", body = ProblemDetailsSchema),
	),
)]
async fn progress(
	State(db): State<Db>,
	State(rl): State<crate::ratelimit::RateLimiter>,
	device: ServerDevice,
	Json(args): Json<ProgressArgs>,
) -> Result<StatusCode> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;

	if !rl.check(
		&format!("backup-progress:{device_id}"),
		PROGRESS_RL_PER_DEVICE,
		PROGRESS_RL_WINDOW,
	) {
		tracing::warn!(
			target: "backup",
			%device_id,
			run_id = %args.run_id,
			"backup-progress rate limit exceeded",
		);
		return Err(AppError::RateLimited);
	}

	let machine = resolve_machine(&mut conn, device_id).await?;
	// As with a report: the server must be grouped so device/group/server come
	// from the authenticated context rather than the body, but the config need not
	// be ready — the run is already happening either way.
	let group_id = require_group(&machine)?;

	database::backups::BackupRunProgress::record(
		&mut conn,
		database::backups::NewBackupRunProgress {
			run_id: args.run_id,
			device_id,
			group_id,
			machine_id: Some(machine.id),
			r#type: args.r#type,
			purpose: args.purpose,
			snapshot_taken_at: args.snapshot_taken_at,
			bytes_read: args.bytes_read,
			bytes_hashed: args.bytes_hashed,
			bytes_uploaded: args.bytes_uploaded,
			bytes_cached: args.bytes_cached,
			bytes_estimated: args.bytes_estimated,
			files_done: args.files_done,
			files_estimated: args.files_estimated,
			errors: args.errors,
			ignored_errors: args.ignored_errors,
			current_path: args.current_path,
			s3_sent_raw_bytes: args.s3_sent_raw_bytes,
			s3_sent_payload_bytes: args.s3_sent_payload_bytes,
			s3_received_raw_bytes: args.s3_received_raw_bytes,
			s3_received_payload_bytes: args.s3_received_payload_bytes,
			extra: args.extra,
		},
	)
	.await?;

	Ok(StatusCode::NO_CONTENT)
}

// ---------------------------------------------------------------------------
// POST /backup-report
// ---------------------------------------------------------------------------

/// Report of a completed backup or restore run.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ReportArgs {
	/// Client-generated UUID identifying this run, minted at run start. Each
	/// run must use a fresh UUID: reporting the same `run_id` twice is
	/// rejected with 409.
	pub run_id: Uuid,
	/// The backup type that ran (e.g. `tamanu-postgres`).
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Whether the run was a `backup` or a `restore`.
	pub purpose: BackupPurpose,
	/// Whether the run succeeded (`success`) or failed (`failure`).
	pub outcome: RunOutcome,
	/// Human-readable error detail, when the run failed.
	pub error: Option<String>,
	/// Total bytes of backup data uploaded during the run, if known.
	pub bytes_uploaded: Option<i64>,
	/// Identifier of the repository snapshot the run produced, for a
	/// successful backup.
	pub snapshot_id: Option<String>,
	/// Bytes of raw HTTP traffic sent to S3 during the run, including protocol
	/// and signing overhead. Report on both success and failure; omit when
	/// traffic was not measured.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Bytes of decoded object payload sent to S3 during the run (excluding
	/// protocol and signing overhead).
	pub s3_sent_payload_bytes: Option<i64>,
	/// Bytes of raw HTTP traffic received from S3 during the run, including
	/// protocol overhead.
	pub s3_received_raw_bytes: Option<i64>,
	/// Bytes of decoded object payload received from S3 during the run.
	pub s3_received_payload_bytes: Option<i64>,
	/// When this run froze the data it backed up — the point in time the backup
	/// represents, as opposed to when its upload finished. Often a filesystem-level
	/// snapshot taken before the transfer, in which case it is not recoverable
	/// from the repository and only the device can report it. Recorded once per
	/// run: if progress reports already carried it, that value stands.
	pub snapshot_taken_at: Option<Timestamp>,
}

/// Report the outcome of a backup or restore run.
///
/// Records the run against the calling server and its group. Send one report
/// per run, on success and on failure alike. Reporting also clears any pending
/// operator-requested run for the same type and purpose — regardless of
/// outcome, since an operator request is for one attempt — so the server's
/// status responses stop asking for it (see the `backup_now` field of the
/// status-push response).
///
/// Errors: 409 when the server is not in a group, or when the `run_id` has
/// already been reported; 412 when the device is not bound to a live server.
#[utoipa::path(
	post,
	path = "/backup-report",
	tag = "backup",
	security(("server-device" = [])),
	request_body = ReportArgs,
	responses(
		(status = 204, description = "Run recorded."),
		(status = 409, description = "Application ungrouped, or duplicate run_id.", body = ProblemDetailsSchema),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
	),
)]
async fn report(
	State(db): State<Db>,
	device: ServerDevice,
	Json(rep): Json<ReportArgs>,
) -> Result<StatusCode> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;

	let machine = resolve_machine(&mut conn, device_id).await?;
	// A report need not be `ready`, but the server must be grouped:
	// device_id/group_id come from the authenticated context, never the body.
	let group_id = require_group(&machine)?;

	// Where the report omits a figure this run already reported as progress, take
	// the last progress value. Progress counters are cumulative, so the final
	// sample is very nearly the run's total — "as of the last sample" rather than
	// exact, which is worth far more than a NULL for a client that reports
	// sparsely (and keeps the size-discrepancy check fed). A figure the report
	// does supply always wins.
	let last_progress =
		database::backups::BackupRunProgress::latest_for_run(&mut conn, rep.run_id).await?;
	// The freeze moment follows a different rule from the figures above: it is
	// write-once per run and the *first* value seen stands, so a moment already
	// announced during the run wins over one repeated on the report. A device
	// often sends it on its first sample only, which is why this is its own query
	// rather than a read of the last sample.
	let progress_snapshot_taken_at =
		database::backups::BackupRunProgress::earliest_snapshot_taken_at_for_run(
			&mut conn, rep.run_id,
		)
		.await?;

	// `record` maps a PK (duplicate run_id) violation to AppError::Conflict (409).
	database::backups::BackupRun::record(
		&mut conn,
		NewBackupRun {
			id: rep.run_id,
			device_id,
			group_id,
			machine_id: Some(machine.id),
			r#type: rep.r#type.clone(),
			purpose: rep.purpose,
			outcome: rep.outcome,
			error: rep.error,
			bytes_uploaded: rep
				.bytes_uploaded
				.or_else(|| last_progress.as_ref().and_then(|p| p.bytes_uploaded)),
			snapshot_id: rep.snapshot_id,
			s3_sent_raw_bytes: rep
				.s3_sent_raw_bytes
				.or_else(|| last_progress.as_ref().and_then(|p| p.s3_sent_raw_bytes)),
			s3_sent_payload_bytes: rep
				.s3_sent_payload_bytes
				.or_else(|| last_progress.as_ref().and_then(|p| p.s3_sent_payload_bytes)),
			s3_received_raw_bytes: rep
				.s3_received_raw_bytes
				.or_else(|| last_progress.as_ref().and_then(|p| p.s3_received_raw_bytes)),
			s3_received_payload_bytes: rep.s3_received_payload_bytes.or_else(|| {
				last_progress
					.as_ref()
					.and_then(|p| p.s3_received_payload_bytes)
			}),
			// Write-once across both endpoints, first value seen wins — so unlike the
			// figures above, progress takes precedence over the report here.
			snapshot_taken_at: progress_snapshot_taken_at.or(rep.snapshot_taken_at),
		},
	)
	.await?;

	// Clear any matching operator one-off so the heartbeat stops re-emitting
	// "back up now" for it — regardless of outcome (the operator asked for one
	// attempt; they can re-request). A scheduled-due signal self-clears once the
	// successful run advances the staleness anchor, so it needs no clearing here.
	BackupRequest::clear(&mut conn, machine.id, &rep.r#type, rep.purpose).await?;

	Ok(StatusCode::NO_CONTENT)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn restore_policy_has_three_statements_and_prefix_substitution() {
		let policy = restore_session_policy("my-bucket", "");
		let v: serde_json::Value = serde_json::from_str(&policy).unwrap();

		assert_eq!(v["Version"], "2012-10-17");
		let stmts = v["Statement"].as_array().unwrap();
		assert_eq!(stmts.len(), 3);

		// 1: GetObject scoped to the prefix.
		assert_eq!(stmts[0]["Action"], serde_json::json!(["s3:GetObject"]));
		assert_eq!(stmts[0]["Resource"], "arn:aws:s3:::my-bucket/*");
		assert!(stmts[0].get("Condition").is_none());

		// 2: GetBucketLocation, unconditioned, bucket-level resource.
		assert_eq!(
			stmts[1]["Action"],
			serde_json::json!(["s3:GetBucketLocation"])
		);
		assert_eq!(stmts[1]["Resource"], "arn:aws:s3:::my-bucket");
		assert!(
			stmts[1].get("Condition").is_none(),
			"GetBucketLocation must be unconditioned — the s3:prefix key isn't populated for it",
		);

		// 3: ListBucket conditioned on the prefix.
		assert_eq!(stmts[2]["Action"], serde_json::json!(["s3:ListBucket"]));
		assert_eq!(stmts[2]["Resource"], "arn:aws:s3:::my-bucket");
		assert_eq!(
			stmts[2]["Condition"]["StringLike"]["s3:prefix"],
			serde_json::json!(["*"]),
		);

		// No mutation actions anywhere.
		let blob = policy.to_lowercase();
		for forbidden in ["putobject", "deleteobject", "abortmultipart"] {
			assert!(
				!blob.contains(forbidden),
				"restore policy must not grant {forbidden}"
			);
		}
	}

	#[test]
	fn restore_policy_substitutes_nonempty_prefix() {
		let policy = restore_session_policy("b", "repo/");
		let v: serde_json::Value = serde_json::from_str(&policy).unwrap();
		let stmts = v["Statement"].as_array().unwrap();
		assert_eq!(stmts[0]["Resource"], "arn:aws:s3:::b/repo/*");
		assert_eq!(
			stmts[2]["Condition"]["StringLike"]["s3:prefix"],
			serde_json::json!(["repo/*"]),
		);
	}

	#[test]
	fn backup_policy_grants_delete_but_not_version_delete_or_retention() {
		let policy = backup_session_policy("my-bucket", "");
		let v: serde_json::Value = serde_json::from_str(&policy).unwrap();
		let stmts = v["Statement"].as_array().unwrap();

		// Object-level write+delete set, scoped to the bucket. DeleteObject is
		// required so kopia can write its delete-markers (metadata management);
		// it's version-less, so the locked version stays immutable.
		assert_eq!(stmts[0]["Resource"], "arn:aws:s3:::my-bucket/*");
		let obj_actions = stmts[0]["Action"].as_array().unwrap();
		for needed in [
			"s3:GetObject",
			"s3:PutObject",
			"s3:DeleteObject",
			"s3:AbortMultipartUpload",
		] {
			assert!(
				obj_actions.contains(&serde_json::json!(needed)),
				"missing {needed}"
			);
		}
		// ListBucket is prefix-conditioned.
		assert_eq!(
			stmts[2]["Condition"]["StringLike"]["s3:prefix"],
			serde_json::json!(["*"]),
		);

		// The device must never be able to remove a *locked version* or weaken a
		// lock — only version-less DeleteObject (delete-markers) is allowed.
		let blob = policy.to_lowercase();
		for forbidden in [
			"deleteobjectversion",
			"bypassgovernanceretention",
			"putobjectretention",
			"putbucketobjectlock",
		] {
			assert!(
				!blob.contains(forbidden),
				"backup policy must not grant {forbidden}"
			);
		}
	}

	#[test]
	fn credential_process_output_uses_exact_aws_casing() {
		let out = CredentialProcessOutput {
			version: 1,
			access_key_id: "AKIA".into(),
			secret_access_key: "secret".into(),
			session_token: "token".into(),
			expiration: "2026-05-21T13:00:00Z".into(),
		};
		let v: serde_json::Value = serde_json::to_value(&out).unwrap();
		assert_eq!(v["Version"], 1);
		assert_eq!(v["AccessKeyId"], "AKIA");
		assert_eq!(v["SecretAccessKey"], "secret");
		assert_eq!(v["SessionToken"], "token");
		assert_eq!(v["Expiration"], "2026-05-21T13:00:00Z");
	}
}
