//! Recovery vault writer.
//!
//! Canopy owns every repo passphrase with no human copy, so it periodically
//! snapshots its recovery-critical state — server groups, backup configs with their
//! per-group passphrase keysets + repo coordinates, schedules, capabilities, the
//! server list, and the inventory secret variables — and writes it, **`age`-encrypted to recipient public keys
//! Canopy never holds the private half of** ([`commons_servers::recovery_vault`]), to a
//! **versioned, object-locked** S3 bucket. Canopy can write the vault but cannot
//! read it back: a full Canopy compromise can't disclose the historical secrets,
//! and object-lock means even root can't delete a version before it expires.
//!
//! Recipients are **mandatory** (`CANOPY_RECOVERY_VAULT_KEYS`) — the backups pod
//! refuses to start without them (see the `backups` bin). The blob is written to
//! the same key each tick; bucket versioning keeps the history.

use std::{
	collections::{BTreeMap, BTreeSet},
	time::Duration,
};

use anyhow::{Context, Result};
use commons_servers::{backup_secrets::BackupSecrets, recovery_vault::Recipients};
use database::{
	MachineBackupCapability, ServerGroupBackupConfig, ServerGroupBackupSchedule,
	applications::Application,
	inventory_secret_variables::{InventorySecretVariable, SecretScope},
	server_groups::ServerGroup,
};
use jiff::Timestamp;
use serde::Serialize;
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{error, info, warn};

use super::worker::Worker;

/// The vault object key (path within the bucket). Not configurable — there's only
/// ever one recovery-state object per bucket; bucket versioning keeps the history.
const VAULT_OBJECT_KEY: &str = "canopy-recovery/state.age";
const DEFAULT_SNAPSHOT_HOURS: u64 = 24;
const SCHEMA_VERSION: u32 = 2;

/// Where + how the recovery vault is written. Recipients are mandatory; the rest
/// comes from `CANOPY_RECOVERY_VAULT_*`.
#[derive(Clone, Debug)]
pub struct RecoveryVaultConfig {
	pub recipients: Recipients,
	pub bucket: String,
	pub region: Option<String>,
	/// Role to assume for the PutObject (the vault bucket lives in a separate
	/// account). `None` → use the pod's default credential chain directly.
	pub role_arn: Option<String>,
	pub period: Duration,
}

impl RecoveryVaultConfig {
	/// Build from the environment. `Err` (not `None`) if the mandatory recipients
	/// or bucket are missing — the backups pod must not run a silent recovery gap.
	pub fn from_env() -> std::result::Result<Self, String> {
		let recipients = Recipients::from_env()
			.map_err(|e| e.to_string())?
			.ok_or_else(|| {
				format!(
					"{} must be set — the recovery vault recipients are mandatory",
					commons_servers::recovery_vault::RECIPIENTS_ENV
				)
			})?;
		let bucket = std::env::var("CANOPY_RECOVERY_VAULT_BUCKET").map_err(|_| {
			"CANOPY_RECOVERY_VAULT_BUCKET must be set for the recovery vault".to_string()
		})?;
		let region = std::env::var("CANOPY_RECOVERY_VAULT_REGION").ok();
		let role_arn = std::env::var("CANOPY_RECOVERY_VAULT_ROLE_ARN").ok();
		let hours = std::env::var("CANOPY_RECOVERY_VAULT_SNAPSHOT_HOURS")
			.ok()
			.and_then(|s| s.parse::<u64>().ok())
			.filter(|h| *h > 0)
			.unwrap_or(DEFAULT_SNAPSHOT_HOURS);
		Ok(Self {
			recipients,
			bucket,
			region,
			role_arn,
			period: Duration::from_secs(hours * 3600),
		})
	}
}

// ── Snapshot shape ─────────────────────────────────────────────────────────

#[derive(Serialize)]
struct RecoverySnapshot {
	schema_version: u32,
	taken_at: String,
	groups: Vec<RecoveryGroup>,
	applications: Vec<Application>,
	enabled_capabilities: Vec<MachineBackupCapability>,
	inventory_secrets: Vec<RecoveryInventorySecrets>,
}

/// One scope's secret variables: the Secret they live under and its keyset,
/// name to value. Empty (logged) if the Secret can't be read.
#[derive(Serialize)]
struct RecoveryInventorySecrets {
	#[serde(flatten)]
	scope: SecretScope,
	secret: String,
	keys: BTreeMap<String, String>,
}

#[derive(Serialize)]
struct RecoveryGroup {
	#[serde(flatten)]
	group: ServerGroup,
	config: Option<RecoveryConfig>,
}

#[derive(Serialize)]
struct RecoveryConfig {
	#[serde(flatten)]
	config: ServerGroupBackupConfig,
	/// The Secret's keyset (`password`, and `password_next` mid-rotation) — the
	/// whole point of the vault. Empty (logged) if the Secret can't be read.
	keys: BTreeMap<String, String>,
	schedules: Vec<ServerGroupBackupSchedule>,
}

/// Gather the recovery-critical state and serialise it to JSON bytes (plaintext, before
/// encryption). Reads the passphrase keyset per group and the secret variables
/// per scope; a missing/unreadable Secret is logged and left empty rather than
/// failing the whole snapshot.
pub async fn build_snapshot_json(
	db: &mut database::diesel_async::AsyncPgConnection,
	secrets: &BackupSecrets,
	now: Timestamp,
) -> Result<Vec<u8>> {
	let groups = ServerGroup::list_all(db).await.context("list groups")?;
	let configs: BTreeMap<_, _> = ServerGroupBackupConfig::list(db)
		.await
		.context("list configs")?
		.into_iter()
		.map(|c| (c.group_id, c))
		.collect();

	let mut recovery_groups = Vec::with_capacity(groups.len());
	let mut applications: Vec<Application> = Vec::new();
	for group in groups {
		applications.extend(
			group
				.list_servers(db)
				.await
				.context("list group applications")?,
		);

		let config = match configs.get(&group.id) {
			Some(config) => {
				let keys = match secrets.read_keys(&config.repo_password_ref).await {
					Ok(keys) => keys,
					Err(e) => {
						warn!(group = %group.id, "recovery-snapshot: keyset unreadable ({e}); storing empty");
						BTreeMap::new()
					}
				};
				let schedules = ServerGroupBackupSchedule::list_for_group(db, group.id)
					.await
					.context("list schedules")?;
				Some(RecoveryConfig {
					config: config.clone(),
					keys,
					schedules,
				})
			}
			None => None,
		};
		recovery_groups.push(RecoveryGroup { group, config });
	}
	applications.extend(
		Application::list_ungrouped(db)
			.await
			.context("list ungrouped applications")?,
	);

	let scopes: BTreeSet<SecretScope> = InventorySecretVariable::list_all(db)
		.await
		.context("list secret variables")?
		.iter()
		.map(InventorySecretVariable::scope)
		.collect();
	let mut inventory_secrets = Vec::with_capacity(scopes.len());
	for scope in scopes {
		let secret = scope.secret_name();
		let keys = match secrets.read_keys(&secret).await {
			Ok(keys) => keys,
			Err(e) => {
				warn!(%secret, "recovery-snapshot: secret variables unreadable ({e}); storing empty");
				BTreeMap::new()
			}
		};
		inventory_secrets.push(RecoveryInventorySecrets {
			scope,
			secret,
			keys,
		});
	}

	let snapshot = RecoverySnapshot {
		schema_version: SCHEMA_VERSION,
		taken_at: now.to_string(),
		groups: recovery_groups,
		applications,
		enabled_capabilities: MachineBackupCapability::list_enabled(db)
			.await
			.context("list capabilities")?,
		inventory_secrets,
	};
	serde_json::to_vec(&snapshot).context("serialise snapshot")
}

/// Encrypt the ciphertext and PUT it to the (versioned, object-locked) vault.
async fn write_vault(config: &RecoveryVaultConfig, ciphertext: Vec<u8>) -> Result<()> {
	let sdk = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
	let mut builder = aws_sdk_s3::config::Builder::from(&sdk);
	if let Some(role_arn) = &config.role_arn {
		let sts = aws_sdk_sts::Client::new(&sdk);
		let assumed = sts
			.assume_role()
			.role_arn(role_arn)
			.role_session_name("canopy-recovery-vault")
			.send()
			.await
			.context("assume recovery vault role")?;
		let c = assumed
			.credentials()
			.context("AssumeRole returned no credentials")?;
		builder = builder.credentials_provider(aws_sdk_s3::config::Credentials::new(
			c.access_key_id(),
			c.secret_access_key(),
			Some(c.session_token().to_string()),
			None,
			"canopy-recovery-vault",
		));
	}
	if let Some(region) = &config.region {
		builder = builder.region(aws_sdk_s3::config::Region::new(region.clone()));
	}
	let s3 = aws_sdk_s3::Client::from_conf(builder.build());

	s3.put_object()
		.bucket(&config.bucket)
		.key(VAULT_OBJECT_KEY)
		.body(ciphertext.into())
		.content_type("application/age")
		.send()
		.await
		.context("put recovery vault object")?;
	Ok(())
}

async fn tick(worker: &Worker, config: &RecoveryVaultConfig) -> Result<()> {
	let mut db = worker
		.pool
		.get()
		.await
		.map_err(|e| anyhow::anyhow!("db: {e}"))?;
	let plaintext = build_snapshot_json(&mut db, &worker.secrets, Timestamp::now()).await?;
	let ciphertext = config
		.recipients
		.encrypt(&plaintext)
		.map_err(|e| anyhow::anyhow!("encrypt: {e}"))?;
	let bytes = ciphertext.len();
	write_vault(config, ciphertext).await?;
	info!(
		bucket = %config.bucket,
		key = VAULT_OBJECT_KEY,
		recipients = config.recipients.len(),
		bytes,
		"recovery-snapshot: wrote encrypted vault object"
	);
	if let Err(e) = database::RecoveryVaultWrite::record(&mut db, bytes as i64).await {
		warn!("recovery-snapshot: failed to record write bookkeeping: {e:#}");
	}
	Ok(())
}

pub fn spawn(worker: Worker, config: RecoveryVaultConfig) -> JoinHandle<()> {
	task::spawn(async move {
		info!(
			period_secs = config.period.as_secs(),
			recipients = config.recipients.len(),
			"recovery-snapshot writer started"
		);
		loop {
			if let Err(e) = tick(&worker, &config).await {
				error!("recovery-snapshot tick failed: {e:#}");
			}
			sleep(config.period).await;
		}
	})
}

#[cfg(test)]
mod tests {
	use super::*;
	use commons_tests::db::TestDb;
	use database::diesel_async::SimpleAsyncConnection;

	#[tokio::test(flavor = "multi_thread")]
	async fn snapshot_includes_group_config_and_keyset() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g');
				 INSERT INTO server_group_backup_config
				   (group_id, bucket, prefix, target_role_arn, maintenance_role_arn,
				    repo_password_ref, status, mode)
				 VALUES ('{group_id}', 'bkt', 'p/', 'arn:dev', 'arn:maint',
				    'backup-repo-{group_id}', 'ready', 'from_birth');"
			))
			.await
			.unwrap();

			// Seed the passphrase keyset in the in-memory secret store.
			let secrets = BackupSecrets::memory();
			secrets
				.create_password(&format!("backup-repo-{group_id}"), "password", "sekret")
				.await
				.unwrap();

			let json = build_snapshot_json(&mut conn, &secrets, Timestamp::now())
				.await
				.unwrap();
			let value: serde_json::Value = serde_json::from_slice(&json).unwrap();

			assert_eq!(value["schema_version"], SCHEMA_VERSION);
			let group = &value["groups"][0];
			assert_eq!(group["id"], group_id.to_string());
			assert_eq!(group["config"]["bucket"], "bkt");
			assert_eq!(group["config"]["maintenance_role_arn"], "arn:maint");
			// The passphrase keyset is the whole point — it must be present.
			assert_eq!(group["config"]["keys"]["password"], "sekret");
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn snapshot_includes_inventory_secrets() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g');
				 INSERT INTO inventory_secret_variables (server_group_id, rank, name)
				 VALUES ('{group_id}', 'production', 'salt');"
			))
			.await
			.unwrap();

			let secrets = BackupSecrets::memory();
			let secret = format!("inventory-vars-{group_id}-production");
			secrets
				.put_keys(
					&secret,
					&BTreeMap::from([("salt".to_string(), "pepper".to_string())]),
				)
				.await
				.unwrap();

			let json = build_snapshot_json(&mut conn, &secrets, Timestamp::now())
				.await
				.unwrap();
			let value: serde_json::Value = serde_json::from_slice(&json).unwrap();

			let entry = &value["inventory_secrets"][0];
			assert_eq!(entry["group_id"], group_id.to_string());
			assert_eq!(entry["rank"], "production");
			assert_eq!(entry["secret"], secret);
			assert_eq!(entry["keys"]["salt"], "pepper");
		})
		.await;
	}
}
