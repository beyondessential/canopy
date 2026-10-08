//! Recovery vault writer.
//!
//! Canopy owns every repo passphrase with no human copy, so it periodically
//! snapshots its recovery-critical state — server groups, backup configs with their
//! per-group passphrase keysets + repo coordinates, schedules, capabilities, the
//! server list, and every inventory variable — and writes it, **`age`-encrypted to
//! recipient public keys
//! Canopy never holds the private half of** ([`commons_servers::recovery_vault`]), to a
//! **versioned, object-locked** S3 bucket. Canopy can write the vault but cannot
//! read it back: a full Canopy compromise can't disclose the historical secrets,
//! and object-lock means even root can't delete a version before it expires.
//!
//! Recipients are **mandatory** (`CANOPY_RECOVERY_VAULT_KEYS`) — the backups pod
//! refuses to start without them (see the `backups` bin). The blob is written to
//! the same key each tick; bucket versioning keeps the history.
//!
//! A value that can't be read is left out of that tick's snapshot rather than
//! failing it, and the snapshot names what it left out. An escrow that wasn't
//! written whole raises the [`RECOVERY_ESCROW_REF`] self-alert and is retried
//! hourly until it is, or until retrying has stopped changing anything.
// spec: ESC

use std::{
	collections::{BTreeMap, BTreeSet},
	time::Duration,
};

use anyhow::{Context, Result};
use commons_servers::{
	backup_secrets::{BackupSecrets, ExposeSecret, SecretString},
	recovery_vault::Recipients,
};
use commons_types::status::CheckResult;
use database::{
	BackupConfigStatus, BackupTypeDefault, MachineBackupCapability, ServerGroupBackupConfig,
	ServerGroupBackupSchedule,
	applications::Application,
	backup::schedules::{MachineBackupSchedule, ScheduleChange},
	inventory_variables::{InventoryVariable, VariableScope},
	self_alerts::{self, RECOVERY_ESCROW_DOC, RECOVERY_ESCROW_REF},
	server_groups::ServerGroup,
};
use jiff::{SignedDuration, Timestamp};
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
/// How soon an escrow that wasn't written whole is tried again, when that is
/// sooner than the configured period.
const RETRY_PERIOD: Duration = Duration::from_secs(3600);
/// How many times in a row an escrow may be rewritten with the very same values
/// left out before it goes back to the configured period: each write is a new
/// object-locked version, and a fault that has outlasted this many retries isn't
/// a passing one.
const MAX_UNCHANGED_RETRIES: u32 = 3;
/// How long a backup configuration may be without its passphrase Secret before
/// that counts as a gap. Onboarding writes the configuration, then the Secret.
const ONBOARDING_GRACE: SignedDuration = SignedDuration::from_secs(600);
/// The key a group's passphrase keyset holds its current passphrase under.
const REPO_PASSWORD_KEY: &str = "password";
/// Version 3 added the fleet-wide per-type defaults, the machine schedule
/// overrides, and the schedule history (group overrides carry cron and zone).
/// Version 4 names what a snapshot left out: a keyset that couldn't be read is
/// null with an `unreadable` reason, and a scope's secret variables that couldn't
/// be read are listed under `unreadable`.
const SCHEMA_VERSION: u32 = 4;

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
	backup_type_defaults: Vec<BackupTypeDefault>,
	machine_backup_schedules: Vec<MachineBackupSchedule>,
	backup_schedule_history: Vec<ScheduleChange>,
	inventory_variables: Vec<RecoveryInventoryVariables>,
}

/// One scope's variables: the plain values as stored, and the secret ones read
/// out of the Secret they live under.
#[derive(Serialize)]
struct RecoveryInventoryVariables {
	#[serde(flatten)]
	scope: VariableScope,
	secret: String,
	values: BTreeMap<String, serde_json::Value>,
	#[serde(serialize_with = "expose")]
	keys: BTreeMap<String, SecretString>,
	/// Secret variables whose value isn't in `keys`, because the Secret couldn't
	/// be read or doesn't hold them.
	#[serde(skip_serializing_if = "Vec::is_empty")]
	unreadable: Vec<String>,
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
	/// whole point of the vault. Null if the Secret can't be read.
	#[serde(serialize_with = "expose_read")]
	keys: Option<BTreeMap<String, SecretString>>,
	/// Why the keyset isn't whole, where it isn't.
	#[serde(skip_serializing_if = "Option::is_none")]
	unreadable: Option<String>,
	schedules: Vec<ServerGroupBackupSchedule>,
}

/// Every passphrase and secret variable is held in a [`SecretString`] until
/// here, so the only plain copy is the one going into the ciphertext.
fn expose<S: serde::Serializer>(
	keys: &BTreeMap<String, SecretString>,
	serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
	serializer.collect_map(
		keys.iter()
			.map(|(name, value)| (name, value.expose_secret())),
	)
}

/// [`expose`] for a keyset that may not have been read, which is null.
fn expose_read<S: serde::Serializer>(
	keys: &Option<BTreeMap<String, SecretString>>,
	serializer: S,
) -> std::result::Result<S::Ok, S::Error> {
	match keys {
		Some(keys) => expose(keys, serializer),
		None => serializer.serialize_none(),
	}
}

/// What was read of a group's passphrase keyset.
struct Keyset {
	/// The keys that were read, if the Secret could be.
	keys: Option<BTreeMap<String, SecretString>>,
	/// Why the keyset isn't whole, where it isn't.
	unreadable: Option<String>,
	/// Whether that is a value left out (reported), as opposed to a
	/// configuration still being onboarded.
	gap: bool,
}

/// Read a group's passphrase keyset, keeping whatever keys can be read: a keyset
/// without a current passphrase can't open the repository, so that is a gap too.
async fn read_keyset(
	secrets: &BackupSecrets,
	config: &ServerGroupBackupConfig,
	now: Timestamp,
) -> Keyset {
	match secrets
		.try_read_secret_keys_partial(&config.repo_password_ref)
		.await
	{
		Ok(Some((keys, skipped))) => {
			let mut problems = Vec::new();
			if !keys.contains_key(REPO_PASSWORD_KEY) {
				problems.push(format!("keyset has no `{REPO_PASSWORD_KEY}`"));
			}
			if !skipped.is_empty() {
				problems.push(format!("keys not UTF-8: {}", skipped.join(", ")));
			}
			Keyset {
				keys: Some(keys),
				gap: !problems.is_empty(),
				unreadable: (!problems.is_empty()).then(|| problems.join("; ")),
			}
		}
		Ok(None) => Keyset {
			keys: None,
			unreadable: Some("keyset does not exist".into()),
			gap: !(config.status == BackupConfigStatus::Provisioning
				&& now.duration_since(config.created_at) < ONBOARDING_GRACE),
		},
		Err(e) => Keyset {
			keys: None,
			unreadable: Some(format!("keyset unreadable ({e})")),
			gap: true,
		},
	}
}

/// A serialised snapshot (plaintext, before encryption), and what it left out.
pub struct Snapshot {
	pub json: Vec<u8>,
	/// One entry per Secret whose values aren't all in the snapshot, naming
	/// it and what is missing. Empty when the snapshot is whole.
	pub gaps: Vec<String>,
}

/// Gather the recovery-critical state and serialise it. Reads the passphrase
/// keyset per group and the secret values per variable scope; a value that
/// can't be read is logged, marked as left out in the snapshot, and reported
/// in [`Snapshot::gaps`], rather than failing the whole snapshot.
pub async fn build_snapshot(
	db: &mut database::diesel_async::AsyncPgConnection,
	secrets: &BackupSecrets,
	now: Timestamp,
) -> Result<Snapshot> {
	let mut gaps = Vec::new();
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
				let Keyset {
					keys,
					unreadable,
					gap,
				} = read_keyset(secrets, config, now).await;
				if let Some(reason) = &unreadable {
					warn!(group = %group.id, secret = %config.repo_password_ref, "recovery-snapshot: {reason}");
				}
				if let (true, Some(reason)) = (gap, &unreadable) {
					gaps.push(format!(
						"{} for group {} ({}): {reason}",
						config.repo_password_ref, group.name, group.id
					));
				}
				let schedules = ServerGroupBackupSchedule::list_for_group(db, group.id)
					.await
					.context("list schedules")?;
				Some(RecoveryConfig {
					config: config.clone(),
					keys,
					unreadable,
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

	let mut by_scope: BTreeMap<VariableScope, BTreeMap<String, Option<serde_json::Value>>> =
		BTreeMap::new();
	for variable in InventoryVariable::list_all(db)
		.await
		.context("list inventory variables")?
	{
		by_scope
			.entry(variable.scope())
			.or_default()
			.insert(variable.name.clone(), variable.value.clone());
	}
	let mut inventory_variables = Vec::with_capacity(by_scope.len());
	for (scope, variables) in by_scope {
		let secret = scope.secret_name();
		let held: BTreeSet<&str> = variables
			.iter()
			.filter(|(_, value)| value.is_none())
			.map(|(name, _)| name.as_str())
			.collect();
		// Only the names a variable still carries: a Secret can hold a key whose
		// row is gone, and the vault it is written to is object-locked.
		let keys: BTreeMap<String, SecretString> = if held.is_empty() {
			BTreeMap::new()
		} else {
			match secrets.try_read_secret_keys_partial(&secret).await {
				Ok(found) => found
					.map(|(keys, _)| keys)
					.unwrap_or_default()
					.into_iter()
					.filter(|(name, _)| held.contains(name.as_str()))
					.collect(),
				Err(e) => {
					warn!(%secret, "recovery-snapshot: secret variables unreadable ({e}); leaving them out");
					BTreeMap::new()
				}
			}
		};
		let unreadable: Vec<String> = held
			.iter()
			.filter(|name| !keys.contains_key(**name))
			.map(|name| name.to_string())
			.collect();
		if !unreadable.is_empty() {
			warn!(%secret, missing = ?unreadable, "recovery-snapshot: secret variables not read; leaving them out");
			gaps.push(format!("{secret}: {}", unreadable.join(", ")));
		}
		inventory_variables.push(RecoveryInventoryVariables {
			scope,
			secret,
			values: variables
				.into_iter()
				.filter_map(|(name, value)| value.map(|value| (name, value)))
				.collect(),
			keys,
			unreadable,
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
		backup_type_defaults: BackupTypeDefault::list(db)
			.await
			.context("list backup type defaults")?,
		machine_backup_schedules: MachineBackupSchedule::list_all(db)
			.await
			.context("list machine backup schedules")?,
		backup_schedule_history: ScheduleChange::list_all(db)
			.await
			.context("list backup schedule history")?,
		inventory_variables,
	};
	Ok(Snapshot {
		json: serde_json::to_vec(&snapshot).context("serialise snapshot")?,
		gaps,
	})
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

/// Write one escrow, answering what it left out.
async fn tick(worker: &Worker, config: &RecoveryVaultConfig) -> Result<Vec<String>> {
	let mut db = worker
		.pool
		.get()
		.await
		.map_err(|e| anyhow::anyhow!("db: {e}"))?;
	let snapshot = build_snapshot(&mut db, &worker.secrets, Timestamp::now()).await?;
	let ciphertext = config
		.recipients
		.encrypt(&snapshot.json)
		.map_err(|e| anyhow::anyhow!("encrypt: {e}"))?;
	let bytes = ciphertext.len();
	write_vault(config, ciphertext).await?;
	info!(
		bucket = %config.bucket,
		key = VAULT_OBJECT_KEY,
		recipients = config.recipients.len(),
		bytes,
		left_out = snapshot.gaps.len(),
		"recovery-snapshot: wrote encrypted vault object"
	);
	if let Err(e) = database::RecoveryVaultWrite::record(&mut db, bytes as i64).await {
		warn!("recovery-snapshot: failed to record write bookkeeping: {e:#}");
	}
	Ok(snapshot.gaps)
}

/// Raise or recover the escrow self-alert from one tick's outcome: the
/// error if the escrow wasn't written, else what it left out.
// spec: ESC#keeping-the-escrow-whole
async fn report(
	db: &mut database::diesel_async::AsyncPgConnection,
	outcome: &Result<Vec<String>>,
) -> Result<()> {
	let (title, message) = match outcome {
		Ok(gaps) if gaps.is_empty() => {
			self_alerts::recover(db, RECOVERY_ESCROW_REF, "the escrow was written whole").await?;
			return Ok(());
		}
		Ok(gaps) => (
			"Recovery escrow incomplete",
			format!(
				"the escrow was written leaving out what Canopy could not read: {}",
				gaps.join("; ")
			),
		),
		Err(e) => (
			"Recovery escrow not written",
			format!("the escrow could not be written: {e:#}"),
		),
	};
	self_alerts::raise(
		db,
		RECOVERY_ESCROW_REF,
		CheckResult::Failed,
		CheckResult::Warning,
		false,
		Some(RECOVERY_ESCROW_DOC),
		title,
		&message,
	)
	.await?;
	Ok(())
}

/// Decides how long to wait after each tick. An escrow that wasn't written
/// is retried hourly; one written with values left out is too, until it has been
/// rewritten with the same values left out [`MAX_UNCHANGED_RETRIES`] times over,
/// when more retries would only pile up locked versions.
#[derive(Default)]
struct Backoff {
	gaps: Vec<String>,
	unchanged: u32,
}

impl Backoff {
	fn after(&mut self, period: Duration, outcome: &Result<Vec<String>>) -> Duration {
		match outcome {
			Ok(gaps) if gaps.is_empty() => {
				*self = Self::default();
				period
			}
			Ok(gaps) => {
				let mut gaps = gaps.clone();
				gaps.sort();
				if gaps == self.gaps {
					self.unchanged += 1;
				} else {
					self.gaps = gaps;
					self.unchanged = 0;
				}
				if self.unchanged < MAX_UNCHANGED_RETRIES {
					period.min(RETRY_PERIOD)
				} else {
					period
				}
			}
			Err(_) => {
				*self = Self::default();
				period.min(RETRY_PERIOD)
			}
		}
	}
}

pub fn spawn(worker: Worker, config: RecoveryVaultConfig) -> JoinHandle<()> {
	task::spawn(async move {
		info!(
			period_secs = config.period.as_secs(),
			recipients = config.recipients.len(),
			"recovery-snapshot writer started"
		);
		let mut backoff = Backoff::default();
		loop {
			let outcome = tick(&worker, &config).await;
			if let Err(e) = &outcome {
				error!("recovery-snapshot tick failed: {e:#}");
			}
			match worker.pool.get().await {
				Ok(mut db) => {
					if let Err(e) = report(&mut db, &outcome).await {
						error!("recovery-snapshot: failed to report escrow state: {e:#}");
					}
				}
				Err(e) => error!("recovery-snapshot: failed to report escrow state: db: {e}"),
			}
			sleep(backoff.after(config.period, &outcome)).await;
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

			let snapshot = build_snapshot(&mut conn, &secrets, Timestamp::now())
				.await
				.unwrap();
			assert!(snapshot.gaps.is_empty(), "{:?}", snapshot.gaps);
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();

			assert_eq!(value["schema_version"], SCHEMA_VERSION);
			let group = &value["groups"][0];
			assert_eq!(group["id"], group_id.to_string());
			assert_eq!(group["config"]["bucket"], "bkt");
			assert_eq!(group["config"]["maintenance_role_arn"], "arn:maint");
			// The passphrase keyset is the whole point — it must be present.
			assert_eq!(group["config"]["keys"]["password"], "sekret");
			assert!(group["config"].get("unreadable").is_none());
		})
		.await;
	}

	/// The escrow carries every layer of the schedules that frame a recovery:
	/// the fleet defaults, each group's overrides, each machine's overrides, and
	/// the history, under a version that says so.
	// spec: ESC, BKO#scheduling
	#[tokio::test(flavor = "multi_thread")]
	async fn snapshot_includes_every_schedule_layer_and_the_history() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = uuid::Uuid::new_v4();
			let machine_id = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g');
				 INSERT INTO machines (id, name, group_id) VALUES ('{machine_id}', 'box', '{group_id}');
				 INSERT INTO server_group_backup_config
				   (group_id, bucket, prefix, target_role_arn, maintenance_role_arn,
				    repo_password_ref, status, mode)
				 VALUES ('{group_id}', 'bkt', 'p/', 'arn:dev', 'arn:maint',
				    'backup-repo-{group_id}', 'ready', 'from_birth');
				 INSERT INTO server_group_backup_schedule
				   (group_id, type, expected_cron, schedule_zone)
				 VALUES ('{group_id}', 'tamanu-postgres', '0 2 * * *', 'Pacific/Auckland');
				 INSERT INTO machine_backup_schedule (machine_id, type, expected_interval)
				 VALUES ('{machine_id}', 'tamanu-postgres', INTERVAL '12 hours');
				 INSERT INTO backup_schedule_history (layer, type, machine_id, kind, interval, changed_by)
				 VALUES ('machine', 'tamanu-postgres', '{machine_id}', 'interval', INTERVAL '12 hours', 'ann@x');"
			))
			.await
			.unwrap();

			let snapshot = build_snapshot(&mut conn, &BackupSecrets::memory(), Timestamp::now())
				.await
				.unwrap();
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();

			assert_eq!(value["schema_version"], SCHEMA_VERSION);
			let fleet = value["backup_type_defaults"].as_array().unwrap();
			assert!(
				fleet.iter().any(|d| d["type"] == "tamanu-postgres"),
				"the seeded fleet default is carried"
			);
			let machines = value["machine_backup_schedules"].as_array().unwrap();
			assert_eq!(machines.len(), 1);
			assert_eq!(machines[0]["machine_id"], machine_id.to_string());
			let history = value["backup_schedule_history"].as_array().unwrap();
			assert!(history.iter().any(|h| h["changed_by"] == "ann@x"));

			// A group's own overrides ride with the group, cron and zone included.
			let group = &value["groups"][0]["config"]["schedules"][0];
			assert_eq!(group["expected_cron"], "0 2 * * *");
			assert_eq!(group["schedule_zone"], "Pacific/Auckland");
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn snapshot_includes_inventory_variables() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g');
				 INSERT INTO inventory_variables (server_group_id, rank, name, is_secret)
				 VALUES ('{group_id}', 'production', 'salt', TRUE);
				 INSERT INTO inventory_variables (server_group_id, rank, name, value, is_secret)
				 VALUES ('{group_id}', 'production', 'timezone', '\"Pacific/Fiji\"', FALSE);"
			))
			.await
			.unwrap();

			let secrets = BackupSecrets::memory();
			let secret = format!("inv-vars-production-{group_id}");
			secrets
				.put_keys(
					&secret,
					&BTreeMap::from([("salt".to_string(), "\"pepper\"".to_string())]),
				)
				.await
				.unwrap();

			let snapshot = build_snapshot(&mut conn, &secrets, Timestamp::now())
				.await
				.unwrap();
			assert!(snapshot.gaps.is_empty(), "{:?}", snapshot.gaps);
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();

			let entry = &value["inventory_variables"][0];
			assert_eq!(entry["group_id"], group_id.to_string());
			assert_eq!(entry["rank"], "production");
			assert_eq!(entry["secret"], secret);
			assert_eq!(entry["keys"]["salt"], "\"pepper\"");
			assert_eq!(entry["values"]["timezone"], "Pacific/Fiji");
			assert!(entry.get("unreadable").is_none());
		})
		.await;
	}

	async fn seed_configured_group(
		conn: &mut database::diesel_async::AsyncPgConnection,
	) -> uuid::Uuid {
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
		group_id
	}

	/// A passphrase keyset that can't be read is written as absent, not as an
	/// empty keyset, and the snapshot is still written.
	// spec: ESC#what-a-recovery-needs
	#[tokio::test(flavor = "multi_thread")]
	async fn unreadable_keyset_is_written_as_absent() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = seed_configured_group(&mut conn).await;

			let snapshot = build_snapshot(&mut conn, &BackupSecrets::memory(), Timestamp::now())
				.await
				.unwrap();
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();

			let config = &value["groups"][0]["config"];
			assert_eq!(config["bucket"], "bkt");
			assert!(config["keys"].is_null());
			assert!(
				config["unreadable"]
					.as_str()
					.unwrap()
					.contains("keyset does not exist")
			);
			assert_eq!(snapshot.gaps.len(), 1);
			assert!(
				snapshot.gaps[0].contains(&format!("backup-repo-{group_id}")),
				"{:?}",
				snapshot.gaps
			);
		})
		.await;
	}

	/// A configuration still being onboarded has no Secret yet; that is not
	/// a gap until it has been that way for longer than onboarding takes.
	// spec: ESC#what-a-recovery-needs
	#[tokio::test(flavor = "multi_thread")]
	async fn keyset_not_yet_created_is_a_gap_only_once_onboarding_should_be_over() {
		TestDb::run(|mut conn, _url| async move {
			seed_configured_group(&mut conn).await;
			conn.batch_execute("UPDATE server_group_backup_config SET status = 'provisioning'")
				.await
				.unwrap();

			let snapshot = build_snapshot(&mut conn, &BackupSecrets::memory(), Timestamp::now())
				.await
				.unwrap();
			assert!(snapshot.gaps.is_empty(), "{:?}", snapshot.gaps);
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();
			assert!(value["groups"][0]["config"]["keys"].is_null());

			let later = Timestamp::now() + SignedDuration::from_secs(3600);
			let snapshot = build_snapshot(&mut conn, &BackupSecrets::memory(), later)
				.await
				.unwrap();
			assert_eq!(snapshot.gaps.len(), 1);
		})
		.await;
	}

	/// A keyset that reads but holds no current passphrase can't open the
	/// repository either, so it counts as left out too.
	// spec: ESC#what-a-recovery-needs
	#[tokio::test(flavor = "multi_thread")]
	async fn keyset_without_a_password_is_a_gap() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = seed_configured_group(&mut conn).await;
			let secrets = BackupSecrets::memory();
			secrets
				.create_password(&format!("backup-repo-{group_id}"), "password_next", "nxt")
				.await
				.unwrap();

			let snapshot = build_snapshot(&mut conn, &secrets, Timestamp::now())
				.await
				.unwrap();
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();

			let config = &value["groups"][0]["config"];
			assert_eq!(config["keys"]["password_next"], "nxt");
			assert!(config["unreadable"].as_str().unwrap().contains("password"));
			assert_eq!(snapshot.gaps.len(), 1);
		})
		.await;
	}

	/// Secret variables whose values can't be read are named, so the snapshot
	/// still says they exist; a Secret that reads but lacks one is the same.
	// spec: ESC#what-a-recovery-needs
	#[tokio::test(flavor = "multi_thread")]
	async fn unreadable_secret_variables_are_named() {
		TestDb::run(|mut conn, _url| async move {
			let group_id = uuid::Uuid::new_v4();
			let machine_id = uuid::Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g');
				 INSERT INTO machines (id, name, group_id) VALUES ('{machine_id}', 'box', '{group_id}');
				 INSERT INTO inventory_variables (server_group_id, rank, name, is_secret)
				 VALUES ('{group_id}', 'production', 'salt', TRUE);
				 INSERT INTO inventory_variables (machine_id, name, is_secret)
				 VALUES ('{machine_id}', 'token', TRUE), ('{machine_id}', 'key', TRUE);"
			))
			.await
			.unwrap();

			// The environment's Secret is absent; the machine's lacks `key`.
			let secrets = BackupSecrets::memory();
			let machine_secret = format!("inv-vars-m-{machine_id}");
			secrets
				.put_keys(
					&machine_secret,
					&BTreeMap::from([("token".to_string(), "\"t\"".to_string())]),
				)
				.await
				.unwrap();

			let snapshot = build_snapshot(&mut conn, &secrets, Timestamp::now())
				.await
				.unwrap();
			let value: serde_json::Value = serde_json::from_slice(&snapshot.json).unwrap();

			let entries = value["inventory_variables"].as_array().unwrap();
			let env = entries.iter().find(|e| e["rank"] == "production").unwrap();
			assert_eq!(env["unreadable"], serde_json::json!(["salt"]));
			assert_eq!(env["keys"], serde_json::json!({}));
			let machine = entries
				.iter()
				.find(|e| e["machine_id"] == machine_id.to_string())
				.unwrap();
			assert_eq!(machine["keys"]["token"], "\"t\"");
			assert_eq!(machine["unreadable"], serde_json::json!(["key"]));
			assert_eq!(snapshot.gaps.len(), 2, "{:?}", snapshot.gaps);
		})
		.await;
	}

	/// An escrow left incomplete or unwritten raises the self-alert, shipped
	/// at a warning ceiling; the next whole write recovers it.
	// spec: ESC#keeping-the-escrow-whole, SELF
	#[tokio::test(flavor = "multi_thread")]
	async fn report_raises_and_recovers_the_escrow_alert() {
		TestDb::run(|mut conn, _url| async move {
			report(&mut conn, &Ok(vec!["inv-vars-g-x: salt".into()]))
				.await
				.unwrap();
			let issue = self_alerts::current(&mut conn, RECOVERY_ESCROW_REF)
				.await
				.unwrap()
				.expect("raised");
			assert!(issue.active);
			assert!(
				issue.message.contains("inv-vars-g-x: salt"),
				"{}",
				issue.message
			);
			assert_eq!(issue.effective_result, Some(CheckResult::Warning));

			report(&mut conn, &Ok(Vec::new())).await.unwrap();
			let issue = self_alerts::current(&mut conn, RECOVERY_ESCROW_REF)
				.await
				.unwrap()
				.unwrap();
			assert!(!issue.active);

			report(
				&mut conn,
				&Err(anyhow::anyhow!("put recovery vault object")),
			)
			.await
			.unwrap();
			let issue = self_alerts::current(&mut conn, RECOVERY_ESCROW_REF)
				.await
				.unwrap()
				.unwrap();
			assert!(issue.active);
			assert!(
				issue.message.contains("could not be written"),
				"{}",
				issue.message
			);
		})
		.await;
	}

	/// An escrow that can't be written is retried hourly without end; one left
	/// incomplete the same way is retried hourly only a few times over.
	// spec: ESC#keeping-the-escrow-whole
	#[test]
	fn backoff_retries_hourly_then_backs_off_while_the_gaps_stand() {
		let day = Duration::from_secs(86400);
		let gaps = || Ok(vec!["b".to_string(), "a".to_string()]);
		let mut backoff = Backoff::default();

		for _ in 0..=MAX_UNCHANGED_RETRIES {
			assert_eq!(backoff.after(day, &Ok(Vec::new())), day);
		}
		let waits: Vec<_> = (0..MAX_UNCHANGED_RETRIES + 2)
			.map(|_| backoff.after(day, &gaps()))
			.collect();
		assert_eq!(waits[..MAX_UNCHANGED_RETRIES as usize], [RETRY_PERIOD; 3]);
		assert_eq!(waits[MAX_UNCHANGED_RETRIES as usize..], [day; 2]);

		// A different gap, a failed write, or a whole write starts over.
		let other = Ok(vec!["c".to_string()]);
		assert_eq!(backoff.after(day, &other), RETRY_PERIOD);
		for _ in 0..=MAX_UNCHANGED_RETRIES {
			backoff.after(day, &other);
		}
		assert_eq!(
			backoff.after(day, &Err(anyhow::anyhow!("put"))),
			RETRY_PERIOD
		);
		assert_eq!(backoff.after(day, &other), RETRY_PERIOD);
		for _ in 0..=MAX_UNCHANGED_RETRIES {
			backoff.after(day, &other);
		}
		assert_eq!(backoff.after(day, &Ok(Vec::new())), day);
		assert_eq!(backoff.after(day, &other), RETRY_PERIOD);

		// A period already shorter than the retry period is kept.
		let short = Duration::from_secs(600);
		assert_eq!(Backoff::default().after(short, &other), short);
	}
}
