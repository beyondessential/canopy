//! Persistent state for the backup-credentials system: the control plane that
//! issues short-lived S3 creds to devices and owns repo maintenance. Backups
//! are keyed `(server, type)` — the repo/bucket is per-group and shared by all
//! the group's backup *types*, while the type (e.g. `tamanu-postgres`) is a
//! dimension on runs / issuances / requests / snapshots.
//!
//! This module owns the diesel models and the DB-layer helpers; the calling
//! logic (STS issuance, scheduler loops, operator UI) lives in the
//! public-server, `jobs`, and private-server components.

use std::collections::{HashMap, HashSet};

use commons_errors::{AppError, Result};
use commons_types::backup::{
	BackupConfigStatus, BackupPlacement, BackupPurpose, BackupRepoMode, BackupType,
	MaintenanceKind, RunOutcome, schedule::Schedule,
};
use diesel::{
	dsl::now,
	prelude::*,
	result::{DatabaseErrorKind, Error as DieselError},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;

/// How long a rotation marker is believed. A `kopia change-password` is one
/// short call, so anything older is a rotation whose process died: readers
/// ignore it rather than let a crash block a group's backups forever.
pub const ROTATION_WINDOW: SignedDuration = SignedDuration::from_mins(15);
use uuid::Uuid;

use crate::pg_duration::PgDuration;

// ---------------------------------------------------------------------------
// RetentionPolicy — the kopia keep-* policy, modelled as a typed struct so the
// wire/openapi shape is concrete (not a raw JSON blob). Stored as JSONB on the
// per-(group,type) schedule and on the type defaults.
// ---------------------------------------------------------------------------

/// How many backups to keep at each retention tier once older snapshots are
/// pruned. The organization enforces minimum floors (at least 7 daily, 4
/// weekly, and 6 monthly) on write, unless the containing schedule or default
/// explicitly opts out via its `allow_below_floor` flag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
pub struct RetentionPolicy {
	/// Number of most-recent backups to always keep regardless of age.
	/// Defaults to 1.
	#[serde(default = "RetentionPolicy::default_keep_latest")]
	pub keep_latest: i32,
	/// Number of most-recent daily backups to keep. Must be at least 7 unless
	/// the retention floor is bypassed.
	pub keep_daily: i32,
	/// Number of most-recent weekly backups to keep. Must be at least 4 unless
	/// the retention floor is bypassed.
	pub keep_weekly: i32,
	/// Number of most-recent monthly backups to keep. Must be at least 6
	/// unless the retention floor is bypassed.
	pub keep_monthly: i32,
	/// Number of most-recent annual backups to keep. Defaults to 0 (no annual
	/// retention).
	#[serde(default)]
	pub keep_annual: i32,
}

impl RetentionPolicy {
	pub const FLOOR_DAILY: i32 = 7;
	pub const FLOOR_WEEKLY: i32 = 4;
	pub const FLOOR_MONTHLY: i32 = 6;

	fn default_keep_latest() -> i32 {
		1
	}

	/// Reject a policy below the org-minimum floor. Returns
	/// [`AppError::BadRequest`] (→ 400) listing the violated field(s).
	pub fn validate_floor(&self) -> Result<()> {
		let mut violations = Vec::new();
		if self.keep_daily < Self::FLOOR_DAILY {
			violations.push(format!("keep_daily must be ≥ {}", Self::FLOOR_DAILY));
		}
		if self.keep_weekly < Self::FLOOR_WEEKLY {
			violations.push(format!("keep_weekly must be ≥ {}", Self::FLOOR_WEEKLY));
		}
		if self.keep_monthly < Self::FLOOR_MONTHLY {
			violations.push(format!("keep_monthly must be ≥ {}", Self::FLOOR_MONTHLY));
		}
		if violations.is_empty() {
			Ok(())
		} else {
			Err(AppError::BadRequest(format!(
				"retention below org minimum: {}",
				violations.join(", ")
			)))
		}
	}

	/// Parse a stored JSONB retention into the typed policy. Returns `None` for
	/// a JSON value that doesn't fit the shape (forward-compatible: a future
	/// extra key is ignored by serde, but a structurally wrong blob is dropped
	/// rather than erroring the whole listing).
	pub fn from_json(value: &JsonValue) -> Option<Self> {
		serde_json::from_value(value.clone()).ok()
	}

	pub fn to_json(&self) -> JsonValue {
		serde_json::to_value(self).expect("RetentionPolicy serializes")
	}
}

// ---------------------------------------------------------------------------
// server_group_backup_config — repo-level config, one row per configured group
// ---------------------------------------------------------------------------

/// Backup repository configuration for a server group: which bucket/prefix
/// the group's backups are stored in, the IAM roles used to access it, and
/// the current provisioning lifecycle state.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::server_group_backup_config)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ServerGroupBackupConfig {
	/// ID of the server group this backup configuration belongs to.
	pub group_id: Uuid,
	/// Name of the S3 bucket backups are stored in.
	pub bucket: String,
	/// Key prefix within the bucket under which this group's backup
	/// repository lives. Empty string means the repository sits at the
	/// bucket root.
	pub prefix: String,
	/// ARN of the IAM role devices assume to upload their own backups
	/// (write-only; no delete permission).
	pub target_role_arn: String,
	/// AWS region the bucket lives in, if known.
	pub region: Option<String>,
	/// Reference to where the repository's encryption passphrase is stored.
	/// Never the passphrase itself.
	pub repo_password_ref: String,
	/// Current lifecycle state of this configuration.
	pub status: BackupConfigStatus,
	/// When this configuration was first created.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When this configuration was last updated.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// How the repository's passphrase was sourced: generated fresh for a new
	/// repository, or supplied by the operator to connect to an existing one.
	#[schema(value_type = String)]
	pub mode: BackupRepoMode,
	/// Error message from the most recent failed provisioning attempt, if
	/// any.
	pub last_init_error: Option<String>,
	/// ARN of the IAM role used to run repository maintenance, inspection,
	/// and metrics collection (full read/write/delete permission).
	///
	/// Sits here, rather than beside `target_role_arn` where it reads more
	/// naturally, because `Queryable` maps positionally against the column
	/// list `schema.rs` declares, and that file is generated in the table's
	/// physical column order. This column was added late. Grouping it with
	/// the other ARN means hand-patching `schema.rs` after every
	/// regeneration.
	pub maintenance_role_arn: String,
	/// Where the backup bucket lives and who provisioned it: an externally
	/// provisioned account the group brought itself, or an
	/// automatically provisioned bucket in a shared account.
	#[schema(value_type = String)]
	pub placement: BackupPlacement,
	/// Set when an operator has requested a one-off full maintenance run; the
	/// scheduler honors it on its next tick (bypassing the cadence slot) and
	/// clears it once the run is spawned. `None` = no pending request.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub force_full_maintenance_at: Option<Timestamp>,
	/// Who requested the pending full-maintenance run, if any.
	pub force_full_maintenance_by: Option<String>,
	/// When the repository passphrase was last successfully rotated. `None`
	/// means it hasn't been since the column was added — such a group is due
	/// at its next rotation target. The rotation scheduler's cadence anchor.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub repo_password_rotated_at: Option<Timestamp>,
	/// Set while a passphrase rotation is in flight. Credential and target
	/// issuance is refused meanwhile, so no device starts a backup with a
	/// passphrase that is about to stop working. A value older than
	/// [`ROTATION_WINDOW`] is a crashed rotation and is ignored.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub repo_password_rotating_since: Option<Timestamp>,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::server_group_backup_config)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewServerGroupBackupConfig {
	pub group_id: Uuid,
	pub bucket: String,
	pub prefix: String,
	pub target_role_arn: String,
	pub maintenance_role_arn: String,
	pub region: Option<String>,
	pub repo_password_ref: String,
	pub status: BackupConfigStatus,
	pub mode: BackupRepoMode,
	pub placement: BackupPlacement,
}

impl ServerGroupBackupConfig {
	/// Fetch a group's backup config (absent → caller maps to 409).
	pub async fn get(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Option<Self>> {
		use crate::schema::server_group_backup_config::dsl;

		dsl::server_group_backup_config
			.filter(dsl::group_id.eq(group_id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Fetch a group's backup config, mapping absent → 404
	/// (`AppError::DatabaseQuery(NotFound)`). For mutation handlers that
	/// require an existing config; `get` keeps the `Option` for the zero-state.
	pub async fn get_required(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Self> {
		Self::get(db, group_id)
			.await?
			.ok_or(AppError::DatabaseQuery(DieselError::NotFound))
	}

	/// All configured groups (the onboarding/stats listing).
	pub async fn list(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::server_group_backup_config::dsl;

		dsl::server_group_backup_config
			.order(dsl::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Create or replace a group's config (the operator onboarding write path).
	/// `created_at`/`updated_at` keep their DB defaults / auto-touch.
	pub async fn upsert(
		db: &mut AsyncPgConnection,
		new: NewServerGroupBackupConfig,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::insert_into(dsl::server_group_backup_config)
			.values(&new)
			.on_conflict(dsl::group_id)
			.do_update()
			.set((
				dsl::bucket.eq(&new.bucket),
				dsl::prefix.eq(&new.prefix),
				dsl::target_role_arn.eq(&new.target_role_arn),
				dsl::maintenance_role_arn.eq(&new.maintenance_role_arn),
				dsl::region.eq(new.region.as_deref()),
				dsl::repo_password_ref.eq(&new.repo_password_ref),
				dsl::status.eq(new.status),
				dsl::mode.eq(new.mode),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Insert a fresh config row (the operator onboarding *create* path). A
	/// second create for the same group violates the PK and is surfaced as
	/// [`AppError::Conflict`] so the handler can return 409 ("already
	/// configured") rather than silently overwriting a live repo's config.
	pub async fn insert(
		db: &mut AsyncPgConnection,
		new: NewServerGroupBackupConfig,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		match diesel::insert_into(dsl::server_group_backup_config)
			.values(&new)
			.returning(Self::as_select())
			.get_result(db)
			.await
		{
			Ok(row) => Ok(row),
			Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => Err(
				AppError::Conflict("group already has a backup config".into()),
			),
			Err(e) => Err(AppError::from(e)),
		}
	}

	/// Edit the non-structural config fields (region only — interval/retention
	/// live on the per-(group,type) schedule). Structural fields
	/// (bucket/role/mode) are immutable post-creation; the handler rejects
	/// those before calling here.
	pub async fn update_region(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		region: Option<&str>,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set((dsl::region.eq(region), dsl::updated_at.eq(now)))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Update the mutable structural fields for the machine config-as-a-resource
	/// upsert: the two role ARNs + region. Bucket/prefix/mode stay immutable (the
	/// handler rejects changes to those before calling here).
	pub async fn update_roles_region(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		target_role_arn: &str,
		maintenance_role_arn: &str,
		region: Option<&str>,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set((
				dsl::target_role_arn.eq(target_role_arn),
				dsl::maintenance_role_arn.eq(maintenance_role_arn),
				dsl::region.eq(region),
				dsl::updated_at.eq(now),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Flag a one-off full maintenance run for the scheduler to pick up on its
	/// next tick (bypassing the cadence slot). Idempotent: re-requesting refreshes
	/// the timestamp/requester. `updated_at` is deliberately not touched — this is
	/// operator intent, not a config edit.
	pub async fn request_full_maintenance(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		requested_by: Option<&str>,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set((
				dsl::force_full_maintenance_at.eq(now),
				dsl::force_full_maintenance_by.eq(requested_by),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Clear a pending full-maintenance request — called both when the scheduler
	/// spawns the run and when an operator cancels a not-yet-picked-up request.
	pub async fn clear_full_maintenance_request(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<()> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set((
				dsl::force_full_maintenance_at.eq(None::<jiff_diesel::Timestamp>),
				dsl::force_full_maintenance_by.eq(None::<String>),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Stamp a successful passphrase rotation, which is the rotation
	/// scheduler's cadence anchor. `updated_at` is deliberately not touched —
	/// this records work done, not a config edit.
	pub async fn mark_passphrase_rotated(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set(dsl::repo_password_rotated_at.eq(now))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Claim the rotation interlock for a group, refusing if one is already
	/// in flight (and still fresh). Returns `true` when claimed.
	///
	/// A marker older than [`ROTATION_WINDOW`] is a rotation whose process
	/// died before clearing it; taking it over is correct, and leaving it set
	/// would block the group's backups indefinitely.
	pub async fn begin_passphrase_rotation(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		at: Timestamp,
	) -> Result<bool> {
		use crate::schema::server_group_backup_config::dsl;

		let stale_before = at - ROTATION_WINDOW;
		let claimed = diesel::update(
			dsl::server_group_backup_config
				.filter(dsl::group_id.eq(group_id))
				.filter(
					dsl::repo_password_rotating_since
						.is_null()
						.or(dsl::repo_password_rotating_since
							.lt(jiff_diesel::Timestamp::from(stale_before))),
				),
		)
		.set(dsl::repo_password_rotating_since.eq(jiff_diesel::Timestamp::from(at)))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(claimed > 0)
	}

	/// Release the rotation interlock, whether the rotation succeeded or not.
	pub async fn end_passphrase_rotation(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set(dsl::repo_password_rotating_since.eq(None::<jiff_diesel::Timestamp>))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Whether a rotation is in flight for the group *right now* — the
	/// question the credential and target endpoints ask before handing out a
	/// passphrase. A stale marker reads as "not rotating".
	pub async fn passphrase_rotation_in_flight(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		at: Timestamp,
	) -> Result<bool> {
		use crate::schema::server_group_backup_config::dsl;

		let since: Option<Option<Timestamp>> = dsl::server_group_backup_config
			.select(dsl::repo_password_rotating_since.nullable())
			.filter(dsl::group_id.eq(group_id))
			.first::<Option<jiff_diesel::Timestamp>>(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.map(|v| v.map(Into::into));
		Ok(matches!(since.flatten(), Some(t) if at.duration_since(t) < ROTATION_WINDOW))
	}

	/// Advance (or reset) the lifecycle status (repo-init flow).
	pub async fn set_status(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		status: BackupConfigStatus,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set(dsl::status.eq(status))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Record intent for the init Job: set `status='provisioning'` and clear
	/// any prior `last_init_error`. Idempotent — re-running it on an already-
	/// provisioning row is a no-op retry. The jobs-side init op observes the
	/// `provisioning` status and drives the row to `ready`.
	pub async fn mark_provisioning(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set((
				dsl::status.eq(BackupConfigStatus::Provisioning),
				dsl::last_init_error.eq(None::<String>),
				dsl::updated_at.eq(now),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Record an init-Job failure: keep `status='provisioning'` and surface the
	/// error for the operator. (The jobs component calls this; included here so
	/// the operator-UI and tests can simulate the failure branch.)
	pub async fn set_last_init_error(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		error: &str,
	) -> Result<Self> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::update(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.set((
				dsl::status.eq(BackupConfigStatus::Provisioning),
				dsl::last_init_error.eq(Some(error)),
				dsl::updated_at.eq(now),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Delete a group's config row (decommission). Audit tables intentionally
	/// have no CASCADE here and the bucket persists (object-locked).
	pub async fn delete(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<()> {
		use crate::schema::server_group_backup_config::dsl;

		diesel::delete(dsl::server_group_backup_config.filter(dsl::group_id.eq(group_id)))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// backup_type_defaults — canopy-wide per-type defaults
// ---------------------------------------------------------------------------

/// Canopy-wide default schedule and retention for a backup type. Any group
/// that doesn't set its own override for the type inherits these values.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::backup_type_defaults)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupTypeDefault {
	/// The backup type this default applies to (e.g. `tamanu-postgres`).
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Default interval, in seconds, between scheduled backups of this type.
	/// `None` means manual-only (no schedule).
	#[schema(value_type = Option<i64>, format = "int64")]
	pub default_interval: Option<PgDuration>,
	/// Default retention policy for this type: how many backups to keep at
	/// each retention tier.
	pub default_retention: JsonValue,
	/// Whether a server that advertises this type has it enabled
	/// automatically, without an operator opting in.
	pub auto_enable: bool,
	/// Opt out of the org retention floor for this type's default (dangerous):
	/// the floor is neither validated on write nor enforced on resolve.
	pub allow_below_floor: bool,
	/// Default cron expression, when the default is a cron schedule rather than
	/// an interval. At most one of this and `default_interval` is set; neither
	/// means manual-only.
	pub default_cron: Option<String>,
	/// The timezone `default_cron` is read in; none reads it in each machine's
	/// own. Only set beside a cron expression.
	pub default_zone: Option<String>,
}

impl BackupTypeDefault {
	/// The default schedule for the type.
	// spec: BKO#scheduling
	pub fn schedule(&self) -> Schedule {
		Schedule::from_columns(
			self.default_interval.map(|d| d.0.as_secs()),
			self.default_cron.clone(),
			self.default_zone.clone(),
		)
	}
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::backup_type_defaults)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewBackupTypeDefault {
	#[diesel(column_name = type_)]
	pub r#type: BackupType,
	pub default_interval: Option<PgDuration>,
	pub default_retention: JsonValue,
	pub auto_enable: bool,
	pub allow_below_floor: bool,
	pub default_cron: Option<String>,
	pub default_zone: Option<String>,
}

impl BackupTypeDefault {
	pub async fn get(db: &mut AsyncPgConnection, r#type: &BackupType) -> Result<Option<Self>> {
		use crate::schema::backup_type_defaults::dsl;

		dsl::backup_type_defaults
			.filter(dsl::type_.eq(r#type.as_str()))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	pub async fn list(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::backup_type_defaults::dsl;

		dsl::backup_type_defaults
			.order(dsl::type_)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn upsert(db: &mut AsyncPgConnection, new: NewBackupTypeDefault) -> Result<Self> {
		use crate::schema::backup_type_defaults::dsl;

		diesel::insert_into(dsl::backup_type_defaults)
			.values(&new)
			.on_conflict(dsl::type_)
			.do_update()
			.set((
				dsl::default_interval.eq(new.default_interval),
				dsl::default_cron.eq(&new.default_cron),
				dsl::default_zone.eq(&new.default_zone),
				dsl::default_retention.eq(&new.default_retention),
				dsl::auto_enable.eq(new.auto_enable),
				dsl::allow_below_floor.eq(new.allow_below_floor),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}
}

// ---------------------------------------------------------------------------
// machine_backup_capabilities — what a server advertises it can back up
// ---------------------------------------------------------------------------

/// A backup type that a machine has advertised it can run, and whether it is
/// currently enabled for that box.
///
/// A capability is the box's: what a run captures is its data, and a box
/// shared by two workloads advertises once.
// spec: BAK
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::machine_backup_capabilities)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MachineBackupCapability {
	/// The box that advertised this capability.
	pub machine_id: Uuid,
	/// The backup type advertised (e.g. `tamanu-postgres`).
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Whether this `(machine, type)` pair is enabled — only enabled pairs are
	/// scheduled and issued credentials.
	pub enabled: bool,
	/// When the box first advertised this capability.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub registered_at: Timestamp,
}

impl MachineBackupCapability {
	/// Register a capability advertised by bestool. `enabled_seed` is the
	/// type's `auto_enable` default and is applied only when the row is first
	/// created — an existing row keeps its operator-set `enabled`.
	pub async fn register(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
		enabled_seed: bool,
	) -> Result<Self> {
		use crate::schema::machine_backup_capabilities::dsl;

		diesel::insert_into(dsl::machine_backup_capabilities)
			.values((
				dsl::machine_id.eq(machine_id),
				dsl::type_.eq(r#type.as_str()),
				dsl::enabled.eq(enabled_seed),
			))
			.on_conflict((dsl::machine_id, dsl::type_))
			// Keep the existing (operator-set) enabled; no-op update so we can
			// RETURNING the existing row.
			.do_update()
			.set(dsl::machine_id.eq(machine_id))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Operator toggle for a `(server, type)`; durable, not re-seeded by the
	/// type default.
	pub async fn set_enabled(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
		enabled: bool,
	) -> Result<Self> {
		use crate::schema::machine_backup_capabilities::dsl;

		diesel::update(
			dsl::machine_backup_capabilities
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::type_.eq(r#type.as_str())),
		)
		.set(dsl::enabled.eq(enabled))
		.returning(Self::as_select())
		.get_result(db)
		.await
		.map_err(AppError::from)
	}

	pub async fn list_for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::machine_backup_capabilities::dsl;

		dsl::machine_backup_capabilities
			.filter(dsl::machine_id.eq(machine_id))
			.order(dsl::type_)
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// All enabled `(machine, type)` capabilities fleet-wide — the candidate set
	/// the scheduler / staleness scan starts from.
	pub async fn list_enabled(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::machine_backup_capabilities::dsl;

		dsl::machine_backup_capabilities
			.filter(dsl::enabled.eq(true))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every enabled `(machine, type)` on a non-archived machine in the group.
	pub async fn enabled_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Vec<(Uuid, BackupType)>> {
		use crate::schema::{machine_backup_capabilities as cap, machines};

		cap::table
			.inner_join(machines::table.on(machines::id.eq(cap::machine_id)))
			.filter(machines::group_id.eq(group_id))
			.filter(machines::deleted_at.is_null())
			.filter(cap::enabled.eq(true))
			.select((cap::machine_id, cap::type_))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Distinct backup types that are **enabled** on any non-archived machine in
	/// the group — i.e. the types the group is actively expected to back up on a
	/// schedule. Used for scheduling cadence and staleness alerting.
	pub async fn enabled_types_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Vec<BackupType>> {
		Self::types_for_group(db, group_id, true).await
	}

	/// Distinct backup types **declared** (advertised by bestool) on any
	/// non-archived machine in the group, regardless of their enabled flag — i.e.
	/// every type the repo can hold snapshots for, including manual-only
	/// (disabled) ones. Retention is resolved per declared type so a manual
	/// backup of a non-scheduled type still gets its own policy, not the global.
	pub async fn declared_types_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Vec<BackupType>> {
		Self::types_for_group(db, group_id, false).await
	}

	/// A capability belongs to a machine, so this reaches the group through the
	/// box that declared it. Going via the applications would find nothing for a
	/// box carrying no workload, and would double-count one carrying two.
	// spec: BKO#participation-and-on-demand
	async fn types_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		enabled_only: bool,
	) -> Result<Vec<BackupType>> {
		use crate::schema::{machine_backup_capabilities as cap, machines};

		let mut q = cap::table
			.inner_join(machines::table.on(machines::id.eq(cap::machine_id)))
			.filter(machines::group_id.eq(group_id))
			.filter(machines::deleted_at.is_null())
			.into_boxed();
		if enabled_only {
			q = q.filter(cap::enabled.eq(true));
		}
		q.select(cap::type_)
			.distinct()
			.load::<BackupType>(db)
			.await
			.map_err(AppError::from)
	}
}

// ---------------------------------------------------------------------------
// server_group_backup_schedule — per-(group,type) schedule/retention overrides
// ---------------------------------------------------------------------------

/// A group's override of the schedule and/or retention for one backup type,
/// taking precedence over the canopy-wide default for that type.
///
/// The row carries the two overrides apart: `has_schedule` says whether the
/// schedule columns override anything, and `retention` is null when retention
/// is inherited. A row with neither does not exist.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::server_group_backup_schedule)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ServerGroupBackupSchedule {
	/// ID of the server group this override applies to.
	pub group_id: Uuid,
	/// The backup type this override applies to (e.g. `tamanu-postgres`).
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Interval, in seconds, between scheduled backups of this type for this
	/// group. Meaningful only when `has_schedule`; with no cron expression,
	/// `None` means manual-only (no schedule).
	#[schema(value_type = Option<i64>, format = "int64")]
	pub expected_interval: Option<PgDuration>,
	/// Retention policy override for this group and type, in the same shape
	/// as the retention policy schema. `None` means inherit the type's
	/// canopy-wide default.
	pub retention: Option<JsonValue>,
	/// When this override was first created.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When this override was last updated.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// Opt out of the org retention floor for this override (dangerous): the
	/// floor is neither validated on write nor enforced on resolve.
	pub allow_below_floor: bool,
	/// A cron expression, when the schedule override is cron rather than an
	/// interval.
	pub expected_cron: Option<String>,
	/// The timezone the cron expression is read in; none reads it in each
	/// machine's own.
	pub schedule_zone: Option<String>,
	/// Whether the schedule columns override the schedule beneath, or this row
	/// is only a retention override.
	pub has_schedule: bool,
}

impl ServerGroupBackupSchedule {
	/// The schedule this overrides with, if it overrides one.
	// spec: BKO#scheduling
	pub fn schedule(&self) -> Option<Schedule> {
		self.has_schedule.then(|| {
			Schedule::from_columns(
				self.expected_interval.map(|d| d.0.as_secs()),
				self.expected_cron.clone(),
				self.schedule_zone.clone(),
			)
		})
	}

	pub async fn get(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		r#type: &BackupType,
	) -> Result<Option<Self>> {
		use crate::schema::server_group_backup_schedule::dsl;

		dsl::server_group_backup_schedule
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::type_.eq(r#type.as_str()))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	pub async fn list_for_group(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::server_group_backup_schedule::dsl;

		dsl::server_group_backup_schedule
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::type_)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Set the group's schedule override for a type, leaving any retention
	/// override as it is.
	pub async fn set_schedule(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		r#type: &BackupType,
		schedule: &Schedule,
	) -> Result<Self> {
		use crate::schema::server_group_backup_schedule::dsl;

		let (interval, cron, zone) = schedule.to_columns();
		let interval = interval.map(|s| PgDuration(SignedDuration::from_secs(s)));
		diesel::insert_into(dsl::server_group_backup_schedule)
			.values((
				dsl::group_id.eq(group_id),
				dsl::type_.eq(r#type.as_str()),
				dsl::expected_interval.eq(interval),
				dsl::expected_cron.eq(&cron),
				dsl::schedule_zone.eq(&zone),
				dsl::has_schedule.eq(true),
			))
			.on_conflict((dsl::group_id, dsl::type_))
			.do_update()
			.set((
				dsl::expected_interval.eq(interval),
				dsl::expected_cron.eq(&cron),
				dsl::schedule_zone.eq(&zone),
				dsl::has_schedule.eq(true),
				dsl::updated_at.eq(now),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Remove the group's schedule override for a type, so it inherits the
	/// fleet default again. A retention override on the same row stays; a row
	/// left overriding nothing is removed.
	pub async fn clear_schedule(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		r#type: &BackupType,
	) -> Result<()> {
		use crate::schema::server_group_backup_schedule::dsl;

		let filter = dsl::server_group_backup_schedule
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::type_.eq(r#type.as_str()));
		diesel::update(filter.clone())
			.set((
				dsl::expected_interval.eq(None::<PgDuration>),
				dsl::expected_cron.eq(None::<String>),
				dsl::schedule_zone.eq(None::<String>),
				dsl::has_schedule.eq(false),
				dsl::updated_at.eq(now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		diesel::delete(filter.filter(dsl::retention.is_null()))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Set the group's retention override for a type, leaving any schedule
	/// override as it is.
	pub async fn set_retention(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		r#type: &BackupType,
		retention: &JsonValue,
		allow_below_floor: bool,
	) -> Result<Self> {
		use crate::schema::server_group_backup_schedule::dsl;

		diesel::insert_into(dsl::server_group_backup_schedule)
			.values((
				dsl::group_id.eq(group_id),
				dsl::type_.eq(r#type.as_str()),
				dsl::retention.eq(Some(retention)),
				dsl::allow_below_floor.eq(allow_below_floor),
				dsl::has_schedule.eq(false),
			))
			.on_conflict((dsl::group_id, dsl::type_))
			.do_update()
			.set((
				dsl::retention.eq(Some(retention)),
				dsl::allow_below_floor.eq(allow_below_floor),
				dsl::updated_at.eq(now),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Remove the group's retention override for a type, so it inherits the
	/// fleet default again. A schedule override on the same row stays; a row
	/// left overriding nothing is removed.
	pub async fn clear_retention(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		r#type: &BackupType,
	) -> Result<()> {
		use crate::schema::server_group_backup_schedule::dsl;

		let filter = dsl::server_group_backup_schedule
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::type_.eq(r#type.as_str()));
		diesel::update(filter.clone())
			.set((
				dsl::retention.eq(None::<JsonValue>),
				dsl::allow_below_floor.eq(false),
				dsl::updated_at.eq(now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		diesel::delete(filter.filter(dsl::has_schedule.eq(false)))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// backup_credential_issuances — audit log of every STS issuance
// ---------------------------------------------------------------------------

/// Audit record of one set of temporary S3 credentials issued to a device for
/// backup or restore. Only the access key id is recorded — the secret key and
/// session token are handed to the device but never stored.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::backup_credential_issuances)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupCredentialIssuance {
	/// Unique id of this issuance record.
	pub id: i64,
	/// ID of the device the credentials were issued to.
	pub device_id: Uuid,
	/// ID of the server group whose backup repository the credentials grant
	/// access to.
	pub group_id: Uuid,
	/// The backup type the credentials were issued for (e.g.
	/// `tamanu-postgres`).
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// When the credentials were issued.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub issued_at: Timestamp,
	/// When the credentials expire and are no longer usable.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub expires_at: Timestamp,
	/// Whether these credentials were issued for uploading a backup or for
	/// restoring from one.
	pub purpose: BackupPurpose,
	/// ARN of the IAM role that was assumed to produce these credentials.
	pub sts_assumed_role: String,
	/// Request id AWS STS returned for the AssumeRole call, if available.
	pub sts_request_id: Option<String>,
	/// Access key id of the issued credentials. The corresponding secret key
	/// and session token are never persisted.
	pub access_key_id: Option<String>,
	/// Name of the S3 bucket these credentials grant access to, as it was at
	/// the time of issuance.
	pub bucket: String,
	/// Key prefix within the bucket these credentials grant access to, as it
	/// was at the time of issuance.
	pub prefix: String,
	/// The run this issuance was minted for, when the client supplied its
	/// run-uuid on the credential request. Ties an issuance to its reported run
	/// exactly (so duration and same-server concurrent runs are unambiguous).
	/// `None` for older clients that don't send it, which fall back to
	/// time-window matching.
	pub run_id: Option<Uuid>,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::backup_credential_issuances)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewBackupCredentialIssuance {
	pub device_id: Uuid,
	pub group_id: Uuid,
	#[diesel(column_name = type_)]
	pub r#type: BackupType,
	#[diesel(serialize_as = jiff_diesel::Timestamp)]
	pub expires_at: Timestamp,
	pub purpose: BackupPurpose,
	pub sts_assumed_role: String,
	pub sts_request_id: Option<String>,
	pub access_key_id: Option<String>,
	/// Snapshot of `bucket` at issuance time (not an FK back to config).
	pub bucket: String,
	/// Snapshot of `prefix` at issuance time.
	pub prefix: String,
	/// Optional run-uuid the client minted at run start, correlating this
	/// issuance with the run it belongs to.
	pub run_id: Option<Uuid>,
}

impl BackupCredentialIssuance {
	/// Record an issuance (public-server, after a successful AssumeRole).
	/// `device_id`/`group_id`/bucket/prefix are resolved by the caller, not
	/// read from a client body.
	pub async fn record(
		db: &mut AsyncPgConnection,
		new: NewBackupCredentialIssuance,
	) -> Result<Self> {
		use crate::schema::backup_credential_issuances::dsl;

		diesel::insert_into(dsl::backup_credential_issuances)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Recent issuances for a device, newest-first (served by the
	/// `(device_id, issued_at DESC)` index).
	pub async fn list_for_device(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_credential_issuances::dsl;

		dsl::backup_credential_issuances
			.filter(dsl::device_id.eq(device_id))
			.order(dsl::issued_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Recent issuances for a group, newest-first.
	pub async fn list_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_credential_issuances::dsl;

		dsl::backup_credential_issuances
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::issued_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Whether any credentials issued for this group are still within their
	/// lifetime — i.e. a device may be running a backup right now, using the
	/// passphrase it was handed.
	///
	/// Rotation asks this before rewriting the repository's format blob:
	/// `kopia change-password` invalidates the passphrase in flight, and the
	/// device only discovers that when its backup fails. Issuances are
	/// recorded before the credentials are returned, so this cannot miss one
	/// that has already been handed out.
	pub async fn any_live_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		at: Timestamp,
	) -> Result<bool> {
		use crate::schema::backup_credential_issuances::dsl;

		let n: i64 = dsl::backup_credential_issuances
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::expires_at.gt(jiff_diesel::Timestamp::from(at)))
			.count()
			.get_result(db)
			.await
			.map_err(AppError::from)?;
		Ok(n > 0)
	}

	/// Issuances for a group issued at or after `since`, newest-first. Backs the
	/// recent-runs view: each issuance is a run *start*, paired with a reported run
	/// (to derive its duration) or surfaced on its own (an unreported / in-flight
	/// restore or backup).
	pub async fn list_for_group_since(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		since: Timestamp,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_credential_issuances::dsl;

		dsl::backup_credential_issuances
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::issued_at.ge(jiff_diesel::Timestamp::from(since)))
			.order(dsl::issued_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Latest *backup-purpose* credential issuance per `(device, type)` within a
	/// group. Keyed `(device_id, type)`; restore issuances are excluded. Used to
	/// infer an in-flight backup: creds still within their validity window with no
	/// newer run report.
	pub async fn latest_backup_by_device_type_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<HashMap<(Uuid, BackupType), LatestIssuance>> {
		use crate::schema::backup_credential_issuances::dsl;

		let rows: Vec<Self> = dsl::backup_credential_issuances
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.distinct_on((dsl::device_id, dsl::type_))
			.order_by((dsl::device_id, dsl::type_, dsl::issued_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.map(|r| {
				(
					(r.device_id, r.r#type.clone()),
					LatestIssuance {
						issued_at: r.issued_at,
						expires_at: r.expires_at,
						run_id: r.run_id,
					},
				)
			})
			.collect())
	}
}

/// The latest backup credential issuance for a `(device, type)`, reduced to what
/// the activity views need of it.
///
/// `run_id` is what ties the inferred in-flight state to the progress that run has
/// been reporting; an issuance from a client predating run correlation has none,
/// and so has no progress to match.
#[derive(Debug, Clone, Copy)]
pub struct LatestIssuance {
	pub issued_at: Timestamp,
	pub expires_at: Timestamp,
	pub run_id: Option<Uuid>,
}

// ---------------------------------------------------------------------------
// backup_runs — what bestool reported per run (client-minted UUID PK)
// ---------------------------------------------------------------------------

/// A backup or restore run reported by a device, with its outcome and any
/// size/traffic figures collected for it.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::backup_runs)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRun {
	/// Unique id of this run, minted by the reporting device.
	pub id: Uuid,
	/// ID of the device that performed and reported this run.
	pub device_id: Uuid,
	/// ID of the server group the run's backup repository belongs to.
	pub group_id: Uuid,
	/// ID of the server the run was performed for, if known.
	pub machine_id: Option<Uuid>,
	/// The backup type this run performed (e.g. `tamanu-postgres`).
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Whether this run was a backup (upload) or a restore (download).
	pub purpose: BackupPurpose,
	/// Whether the run succeeded or failed.
	pub outcome: RunOutcome,
	/// Error message reported for a failed run, if any.
	pub error: Option<String>,
	/// Number of bytes the device reports having uploaded for this run.
	pub bytes_uploaded: Option<i64>,
	/// Id of the snapshot this run produced, if any.
	pub snapshot_id: Option<String>,
	/// When this run was reported.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub reported_at: Timestamp,
	/// Bytes sent to S3 for this run, counting the full HTTP request including
	/// signing/chunking overhead.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Bytes sent to S3 for this run, counting only the decoded object data
	/// (excludes request/signing overhead).
	pub s3_sent_payload_bytes: Option<i64>,
	/// Bytes received from S3 for this run, counting the full HTTP response
	/// including framing overhead.
	pub s3_received_raw_bytes: Option<i64>,
	/// Bytes received from S3 for this run, counting only the decoded object
	/// data (excludes response framing overhead).
	pub s3_received_payload_bytes: Option<i64>,
	/// Logical (uncompressed) size of this run's snapshot, as independently
	/// observed by repository inspection rather than reported by the device.
	/// Distinct from `bytes_uploaded`; filled in after the fact, once, and
	/// never overwritten.
	pub snapshot_logical_bytes: Option<i64>,
	/// When the run froze the data it captured — the point in time this backup
	/// represents, as opposed to [`Self::reported_at`], when its upload finished.
	/// For a large backup the two are hours apart. Often a filesystem-level
	/// snapshot taken *below* the backup engine, so it leaves no trace in the
	/// repository and can only come from the device. Written once per run,
	/// whichever report carries it first. `None` for a client that doesn't
	/// report it, in which case [`Self::anchor`] falls back to `reported_at`.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub snapshot_taken_at: Option<Timestamp>,
}

impl BackupRun {
	/// When this run's data is *as of*, for any question about the age of what
	/// was backed up: the moment the run froze its data, falling back to the
	/// report time when the client didn't report one.
	///
	/// This is the staleness measure (see BKJ). It is deliberately not used for
	/// the reconcile signals, which assert that the *reporting path* works and so
	/// belong on `reported_at`.
	pub fn anchor(&self) -> Timestamp {
		self.snapshot_taken_at.unwrap_or(self.reported_at)
	}
}

/// SQL for [`BackupRun::anchor`], for ordering and filtering in the database.
///
/// Queries that pick the "latest" successful run must order by *this*, not by
/// `reported_at`, whenever the caller then measures staleness from
/// [`BackupRun::anchor`]. If selection and measure disagree, a server's freshness
/// can travel backwards: given run A (reported 08:00, taken 04:00) and run B
/// (reported 09:00, taken 03:00), ordering by `reported_at` picks B, whose data
/// is the *older* of the two — so the arrival of a newer run would age the
/// server and flap its staleness state.
pub const ANCHOR_SQL: &str = "COALESCE(snapshot_taken_at, reported_at)";

/// [`ANCHOR_SQL`] as a typed Diesel expression, for `ORDER BY`.
fn anchor_expr() -> diesel::expression::SqlLiteral<diesel::sql_types::Timestamptz> {
	diesel::dsl::sql::<diesel::sql_types::Timestamptz>(ANCHOR_SQL)
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::backup_runs)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewBackupRun {
	/// The run-uuid bestool minted at run start (becomes the PK).
	pub id: Uuid,
	pub device_id: Uuid,
	pub group_id: Uuid,
	pub machine_id: Option<Uuid>,
	#[diesel(column_name = type_)]
	pub r#type: BackupType,
	pub purpose: BackupPurpose,
	pub outcome: RunOutcome,
	pub error: Option<String>,
	pub bytes_uploaded: Option<i64>,
	pub snapshot_id: Option<String>,
	pub s3_sent_raw_bytes: Option<i64>,
	pub s3_sent_payload_bytes: Option<i64>,
	pub s3_received_raw_bytes: Option<i64>,
	pub s3_received_payload_bytes: Option<i64>,
	#[diesel(serialize_as = jiff_diesel::NullableTimestamp)]
	pub snapshot_taken_at: Option<Timestamp>,
}

/// Multi-field filter for the fleet-wide backup-run history query. Each field
/// is opt-in: `Default` matches every run.
#[derive(Debug, Clone, Default)]
pub struct BackupRunFilters {
	pub group_id: Option<Uuid>,
	pub machine_id: Option<Uuid>,
	pub r#type: Option<BackupType>,
	pub outcome: Option<RunOutcome>,
	/// When `Some`, restrict to runs reported at or after this time.
	pub since: Option<Timestamp>,
}

impl BackupRun {
	/// Record a reported run. A duplicate `id` (the client-minted run-uuid)
	/// fails its own insert; that PK violation is surfaced as
	/// [`AppError::Conflict`] so the endpoint can choose 409 vs idempotent 204.
	pub async fn record(db: &mut AsyncPgConnection, new: NewBackupRun) -> Result<Self> {
		use crate::schema::backup_runs::dsl;

		let id = new.id;
		match diesel::insert_into(dsl::backup_runs)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
		{
			Ok(run) => Ok(run),
			Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => Err(
				AppError::Conflict(format!("backup run {id} already reported")),
			),
			Err(e) => Err(AppError::from(e)),
		}
	}

	/// Latest successful *backup* (never restore) for a `(server, type)` — the
	/// staleness anchor. Filters `purpose='backup'` + `outcome='success'` so a
	/// recent successful restore can't reset backup staleness.
	///
	/// "Latest" is by [`Self::anchor`] — the data's own moment — not by report
	/// time; see [`ANCHOR_SQL`].
	pub async fn latest_success_for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
	) -> Result<Option<Self>> {
		use crate::schema::backup_runs::dsl;

		dsl::backup_runs
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::type_.eq(r#type.as_str()))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.filter(dsl::outcome.eq(RunOutcome::Success))
			.order(anchor_expr().desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Latest successful backup per `(server, type)` within a group — the bulk
	/// staleness-scan input. Keyed `(machine_id, type)`; rows with a NULL
	/// `machine_id` are skipped (they can't be attributed to a server).
	///
	/// "Latest" is by [`Self::anchor`], matching what the caller then measures
	/// staleness from; see [`ANCHOR_SQL`] for why the two must agree.
	/// Latest successful backup per `(machine, type)` within a group.
	///
	/// This is the snapshot authority a restore replica is pointed at: what gets
	/// restored is a snapshot, and a snapshot is what a machine backed up (see
	/// RST, "Snapshot authority"). A box carrying two workloads has one latest
	/// snapshot per type, being one box.
	// spec: RST#snapshot-authority
	pub async fn latest_success_by_machine_type_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<HashMap<(Uuid, BackupType), Self>> {
		use crate::schema::backup_runs::dsl;

		let rows: Vec<Self> = dsl::backup_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.filter(dsl::outcome.eq(RunOutcome::Success))
			.filter(dsl::machine_id.is_not_null())
			.distinct_on((dsl::machine_id, dsl::type_))
			.order_by((dsl::machine_id, dsl::type_, anchor_expr().desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter_map(|r| r.machine_id.map(|mid| ((mid, r.r#type.clone()), r)))
			.collect())
	}

	/// Latest *reported* backup per `(machine, type)` within a group, regardless
	/// of outcome (success or failure). Used to tell whether a backup has been
	/// reported since credentials were last issued — i.e. whether one is still
	/// in flight.
	pub async fn latest_report_by_machine_type_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<HashMap<(Uuid, BackupType), Timestamp>> {
		use crate::schema::backup_runs::dsl;

		let rows: Vec<Self> = dsl::backup_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.filter(dsl::machine_id.is_not_null())
			.distinct_on((dsl::machine_id, dsl::type_))
			.order_by((dsl::machine_id, dsl::type_, dsl::reported_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter_map(|r| {
				r.machine_id
					.map(|sid| ((sid, r.r#type.clone()), r.reported_at))
			})
			.collect())
	}

	/// When the group last had a successful *backup* reported (any server/type),
	/// or `None` if it never has. Drives prompt post-backup inspection.
	pub async fn latest_backup_at_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Option<Timestamp>> {
		use crate::schema::backup_runs::dsl;

		let row: Option<Self> = dsl::backup_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.filter(dsl::outcome.eq(RunOutcome::Success))
			.order_by(dsl::reported_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;
		Ok(row.map(|r| r.reported_at))
	}

	/// Recent runs for a group, newest-first (stats panel).
	pub async fn list_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_runs::dsl;

		dsl::backup_runs
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::reported_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Fleet-wide run history, newest first, narrowed by whichever filters are
	/// set. Backs the MCP `list_backup_runs` tool.
	pub async fn list_filtered(
		db: &mut AsyncPgConnection,
		filters: BackupRunFilters,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_runs::dsl;

		let mut q = dsl::backup_runs.into_boxed();
		if let Some(gid) = filters.group_id {
			q = q.filter(dsl::group_id.eq(gid));
		}
		if let Some(sid) = filters.machine_id {
			q = q.filter(dsl::machine_id.eq(sid));
		}
		if let Some(ty) = &filters.r#type {
			q = q.filter(dsl::type_.eq(ty.as_str()));
		}
		if let Some(outcome) = filters.outcome {
			q = q.filter(dsl::outcome.eq(outcome));
		}
		if let Some(since) = filters.since {
			q = q.filter(dsl::reported_at.ge(jiff_diesel::Timestamp::from(since)));
		}
		q.order(dsl::reported_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Total S3 bytes sent/received (raw — the full wire size including SigV4
	/// chunk framing, not the decoded payload) across the group's device backup
	/// runs reported so far this calendar month (UTC boundary). Returns
	/// `(sent, received)`, `0` for a side with no tallied runs.
	///
	/// This only covers what bestool's proxy tallies on `backup_runs` during
	/// device backups — repo maintenance and inspection traffic against the
	/// bucket isn't tallied anywhere, so the total undercounts the bucket's
	/// actual monthly S3 traffic.
	pub async fn s3_traffic_this_month_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<(i64, i64)> {
		#[derive(diesel::QueryableByName)]
		struct Totals {
			#[diesel(sql_type = diesel::sql_types::Int8)]
			sent: i64,
			#[diesel(sql_type = diesel::sql_types::Int8)]
			received: i64,
		}

		// SUM() over bigint columns promotes to numeric in Postgres, so the
		// query casts back to bigint explicitly; COALESCE turns "no rows"/
		// "all NULL" into 0 rather than surfacing NULL.
		let totals: Totals = diesel::sql_query(
			"SELECT \
				COALESCE(SUM(s3_sent_raw_bytes), 0)::bigint AS sent, \
				COALESCE(SUM(s3_received_raw_bytes), 0)::bigint AS received \
			FROM backup_runs \
			WHERE group_id = $1 \
				AND reported_at >= date_trunc('month', now(), 'UTC')",
		)
		.bind::<diesel::sql_types::Uuid, _>(group_id)
		.get_result(db)
		.await
		.map_err(AppError::from)?;

		Ok((totals.sent, totals.received))
	}

	/// The size of each named snapshot, resolved from the *backup* runs that
	/// produced them: the device-reported size (`bytes_uploaded`) or, failing
	/// that, the inspection-observed size (`snapshot_logical_bytes`) — the same
	/// preference the recent-runs UI applies to the producing run itself. Keyed
	/// by snapshot id; ids with no sized producing run are absent.
	///
	/// This is what lets a restore run show the size of the snapshot it used
	/// immediately: the snapshot was sized when the backup that created it ran
	/// (or was inspected), so there is no need to wait for inspection to
	/// backfill the restore run's own row.
	pub async fn snapshot_sizes_by_id(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		snapshot_ids: &[String],
	) -> Result<HashMap<String, i64>> {
		use crate::schema::backup_runs::dsl;

		if snapshot_ids.is_empty() {
			return Ok(HashMap::new());
		}

		let rows: Vec<Self> = dsl::backup_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.filter(dsl::snapshot_id.eq_any(snapshot_ids))
			.filter(
				dsl::bytes_uploaded
					.is_not_null()
					.or(dsl::snapshot_logical_bytes.is_not_null()),
			)
			.distinct_on(dsl::snapshot_id)
			.order_by((dsl::snapshot_id, dsl::reported_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter_map(|r| {
				let id = r.snapshot_id?;
				let size = r.bytes_uploaded.or(r.snapshot_logical_bytes)?;
				Some((id, size))
			})
			.collect())
	}

	/// Fill `snapshot_logical_bytes` from repo inspection for runs matched by
	/// snapshot id, only where it is still unset — write-once, since a snapshot
	/// is immutable. `sizes` maps a snapshot id to its observed logical size.
	/// Returns the number of runs updated.
	pub async fn backfill_snapshot_logical_bytes(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		sizes: &[(String, i64)],
	) -> Result<usize> {
		use crate::schema::backup_runs::dsl;

		let mut updated = 0usize;
		for (snapshot_id, size) in sizes {
			updated += diesel::update(dsl::backup_runs)
				.filter(dsl::group_id.eq(group_id))
				.filter(dsl::snapshot_id.eq(snapshot_id))
				.filter(dsl::snapshot_logical_bytes.is_null())
				.set(dsl::snapshot_logical_bytes.eq(size))
				.execute(db)
				.await
				.map_err(AppError::from)?;
		}
		Ok(updated)
	}

	/// The latest backup run per `(server, type)` in a group that carries both a
	/// device-reported size (`bytes_uploaded`) and an inspection-observed size
	/// (`snapshot_logical_bytes`), both non-zero. Keyed `(machine_id, type)` →
	/// `(reported, observed)`. The basis for the size-discrepancy check.
	pub async fn latest_sized_by_machine_type_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<HashMap<(Uuid, BackupType), (i64, i64)>> {
		use crate::schema::backup_runs::dsl;

		let rows: Vec<Self> = dsl::backup_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::purpose.eq(BackupPurpose::Backup))
			.filter(dsl::machine_id.is_not_null())
			.filter(dsl::bytes_uploaded.is_not_null())
			.filter(dsl::snapshot_logical_bytes.is_not_null())
			.distinct_on((dsl::machine_id, dsl::type_))
			.order_by((dsl::machine_id, dsl::type_, dsl::reported_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter_map(|r| {
				let sid = r.machine_id?;
				let (up, snap) = (r.bytes_uploaded?, r.snapshot_logical_bytes?);
				(up > 0 && snap > 0).then_some(((sid, r.r#type.clone()), (up, snap)))
			})
			.collect())
	}
}

// ---------------------------------------------------------------------------
// backup_run_progress — what bestool reports *while* a run is in flight
// ---------------------------------------------------------------------------

/// One progress sample reported by a device during a run.
///
/// Unlike [`BackupRun`], which exists only once a run has finished, a sample
/// describes a run still under way — so it carries no foreign key to
/// `backup_runs` and is self-describing on device/group/server/type/purpose,
/// exactly as [`BackupCredentialIssuance`] is and for the same reason.
///
/// **Every counter is cumulative from the start of the run**, never an interval
/// delta. That is what makes a dropped or repeated sample cost only resolution
/// rather than corrupt a total, and it is why the last sample of a run is a
/// usable stand-in for a figure the run's report omitted. All counters are
/// optional: a device omits whatever it does not measure.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::backup_run_progress)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRunProgress {
	/// Row id. Not meaningful beyond ordering ties within an instant.
	pub id: i64,
	/// The run this sample belongs to — the uuid the device minted for it, the
	/// same one it reports the run under. Not a foreign key: the run has no row
	/// until it finishes.
	pub run_id: Uuid,
	/// The device that reported the sample.
	pub device_id: Uuid,
	/// The group whose repository the run is writing to or reading from.
	pub group_id: Uuid,
	/// The server the run is for, when the device resolved to one.
	pub machine_id: Option<Uuid>,
	/// The backup type being run.
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Whether the run is a backup (upload) or a restore (download).
	pub purpose: BackupPurpose,
	/// When Canopy received this sample. Application-stamped, not device-supplied:
	/// transfer rate is derived from it, and "is this run moving, as far as
	/// Canopy can tell" is a receipt-time question.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub observed_at: Timestamp,
	/// When the run froze the data it captured, as reported by the device. See
	/// [`BackupRun::snapshot_taken_at`] — known before any transfer starts, so a
	/// device can report it on its very first sample.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub snapshot_taken_at: Option<Timestamp>,
	/// Source bytes the run has read so far.
	pub bytes_read: Option<i64>,
	/// Bytes the run has processed (hashed/compressed) so far.
	pub bytes_hashed: Option<i64>,
	/// Bytes the run has uploaded to the repository so far.
	pub bytes_uploaded: Option<i64>,
	/// Bytes the run found already present and so did not re-upload.
	pub bytes_cached: Option<i64>,
	/// Total bytes the run expects to handle, as it currently estimates. An
	/// estimate, and one that may be revised upward mid-run.
	pub bytes_estimated: Option<i64>,
	/// Files the run has finished with so far.
	pub files_done: Option<i64>,
	/// Total files the run expects to handle, as it currently estimates.
	pub files_estimated: Option<i64>,
	/// Errors the run has hit so far.
	pub errors: Option<i64>,
	/// Errors the run has hit and deliberately ignored so far.
	pub ignored_errors: Option<i64>,
	/// What the run was working on when it took the sample.
	pub current_path: Option<String>,
	/// Bytes sent to object storage so far, counting full HTTP requests.
	pub s3_sent_raw_bytes: Option<i64>,
	/// Bytes sent to object storage so far, counting decoded payload only.
	pub s3_sent_payload_bytes: Option<i64>,
	/// Bytes received from object storage so far, counting full HTTP responses.
	pub s3_received_raw_bytes: Option<i64>,
	/// Bytes received from object storage so far, counting decoded payload only.
	pub s3_received_payload_bytes: Option<i64>,
	/// Engine-specific detail Canopy makes no commitment about. Stored and
	/// surfaced verbatim; never interpreted, never queried against.
	pub extra: JsonValue,
}

#[derive(Debug, Clone, Insertable)]
#[diesel(table_name = crate::schema::backup_run_progress)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewBackupRunProgress {
	pub run_id: Uuid,
	pub device_id: Uuid,
	pub group_id: Uuid,
	pub machine_id: Option<Uuid>,
	#[diesel(column_name = type_)]
	pub r#type: BackupType,
	pub purpose: BackupPurpose,
	#[diesel(serialize_as = jiff_diesel::NullableTimestamp)]
	pub snapshot_taken_at: Option<Timestamp>,
	pub bytes_read: Option<i64>,
	pub bytes_hashed: Option<i64>,
	pub bytes_uploaded: Option<i64>,
	pub bytes_cached: Option<i64>,
	pub bytes_estimated: Option<i64>,
	pub files_done: Option<i64>,
	pub files_estimated: Option<i64>,
	pub errors: Option<i64>,
	pub ignored_errors: Option<i64>,
	pub current_path: Option<String>,
	pub s3_sent_raw_bytes: Option<i64>,
	pub s3_sent_payload_bytes: Option<i64>,
	pub s3_received_raw_bytes: Option<i64>,
	pub s3_received_payload_bytes: Option<i64>,
	pub extra: JsonValue,
}

impl BackupRunProgress {
	/// Store a sample. `observed_at` is left to the column default so the time is
	/// Postgres's, not the device's and not the application's.
	pub async fn record(db: &mut AsyncPgConnection, new: NewBackupRunProgress) -> Result<Self> {
		use crate::schema::backup_run_progress::dsl;

		diesel::insert_into(dsl::backup_run_progress)
			.values(new)
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// The whole series for one run, oldest-first — the shape a rate chart wants.
	/// Served by the `(run_id, observed_at DESC)` index.
	pub async fn series_for_run(db: &mut AsyncPgConnection, run_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::backup_run_progress::dsl;

		dsl::backup_run_progress
			.filter(dsl::run_id.eq(run_id))
			.order_by((dsl::observed_at.asc(), dsl::id.asc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The most recent sample for one run, if it has reported any.
	pub async fn latest_for_run(db: &mut AsyncPgConnection, run_id: Uuid) -> Result<Option<Self>> {
		use crate::schema::backup_run_progress::dsl;

		dsl::backup_run_progress
			.filter(dsl::run_id.eq(run_id))
			.order_by((dsl::observed_at.desc(), dsl::id.desc()))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The most recent sample per run for a set of runs, in one query.
	///
	/// This is the batch loader the group activity view uses: it has many
	/// in-flight rows to decorate, and issuing [`Self::latest_for_run`] per row
	/// would be an N+1. Empty input short-circuits without touching the database.
	pub async fn latest_by_run(
		db: &mut AsyncPgConnection,
		run_ids: &[Uuid],
	) -> Result<HashMap<Uuid, Self>> {
		use crate::schema::backup_run_progress::dsl;

		if run_ids.is_empty() {
			return Ok(HashMap::new());
		}

		let rows: Vec<Self> = dsl::backup_run_progress
			.filter(dsl::run_id.eq_any(run_ids))
			.distinct_on(dsl::run_id)
			.order_by((dsl::run_id, dsl::observed_at.desc(), dsl::id.desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows.into_iter().map(|r| (r.run_id, r)).collect())
	}

	/// The earliest freeze moment this run reported as progress, if it reported one
	/// at all.
	///
	/// Distinct from reading [`Self::latest_for_run`]'s `snapshot_taken_at`: a
	/// device may announce the moment on its first sample and omit it from every
	/// sample after, so the latest sample often has NULL where an earlier one had
	/// the value. `snapshot_taken_at` is write-once per run — first value seen
	/// stands — and this is what "first seen" means on the progress side.
	pub async fn earliest_snapshot_taken_at_for_run(
		db: &mut AsyncPgConnection,
		run_id: Uuid,
	) -> Result<Option<Timestamp>> {
		use crate::schema::backup_run_progress::dsl;

		let found: Option<jiff_diesel::Timestamp> = dsl::backup_run_progress
			.filter(dsl::run_id.eq(run_id))
			.filter(dsl::snapshot_taken_at.is_not_null())
			.order_by((dsl::observed_at.asc(), dsl::id.asc()))
			.select(dsl::snapshot_taken_at.assume_not_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;

		Ok(found.map(Timestamp::from))
	}

	/// Samples for one run at or after `since`, oldest-first. Backs rate over a
	/// trailing window without pulling a long run's entire series.
	pub async fn for_run_since(
		db: &mut AsyncPgConnection,
		run_id: Uuid,
		since: Timestamp,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_run_progress::dsl;

		dsl::backup_run_progress
			.filter(dsl::run_id.eq(run_id))
			.filter(dsl::observed_at.ge(jiff_diesel::Timestamp::from(since)))
			.order_by((dsl::observed_at.asc(), dsl::id.asc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The earliest freeze moment each of `run_ids` reported, for those that
	/// reported one. Batch counterpart to
	/// [`Self::earliest_snapshot_taken_at_for_run`], and subject to the same
	/// subtlety: a device usually announces the moment once, on an early sample, so
	/// this cannot be read off the latest sample.
	pub async fn earliest_snapshot_taken_at_by_run(
		db: &mut AsyncPgConnection,
		run_ids: &[Uuid],
	) -> Result<HashMap<Uuid, Timestamp>> {
		use crate::schema::backup_run_progress::dsl;

		if run_ids.is_empty() {
			return Ok(HashMap::new());
		}

		let rows: Vec<(Uuid, jiff_diesel::Timestamp)> = dsl::backup_run_progress
			.filter(dsl::run_id.eq_any(run_ids))
			.filter(dsl::snapshot_taken_at.is_not_null())
			.distinct_on(dsl::run_id)
			.order_by((dsl::run_id, dsl::observed_at.asc(), dsl::id.asc()))
			.select((dsl::run_id, dsl::snapshot_taken_at.assume_not_null()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.map(|(rid, ts)| (rid, Timestamp::from(ts)))
			.collect())
	}

	/// Samples for a set of runs observed at or after `since`, oldest-first within
	/// each run, grouped by run.
	///
	/// The batch counterpart to [`Self::for_run_since`]: the activity view needs a
	/// trailing window for every in-flight row at once, and issuing one query per
	/// row would be an N+1. Empty input short-circuits.
	pub async fn for_runs_since(
		db: &mut AsyncPgConnection,
		run_ids: &[Uuid],
		since: Timestamp,
	) -> Result<HashMap<Uuid, Vec<Self>>> {
		use crate::schema::backup_run_progress::dsl;

		if run_ids.is_empty() {
			return Ok(HashMap::new());
		}

		let rows: Vec<Self> = dsl::backup_run_progress
			.filter(dsl::run_id.eq_any(run_ids))
			.filter(dsl::observed_at.ge(jiff_diesel::Timestamp::from(since)))
			.order_by((dsl::run_id, dsl::observed_at.asc(), dsl::id.asc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let mut out: HashMap<Uuid, Vec<Self>> = HashMap::new();
		for row in rows {
			out.entry(row.run_id).or_default().push(row);
		}
		Ok(out)
	}

	/// Delete samples observed before `cutoff`, returning how many went. The
	/// series is working data for watching and reviewing runs, not part of a
	/// run's permanent record, so it is pruned wholesale on age.
	pub async fn prune_before(db: &mut AsyncPgConnection, cutoff: Timestamp) -> Result<usize> {
		use crate::schema::backup_run_progress::dsl;

		diesel::delete(
			dsl::backup_run_progress
				.filter(dsl::observed_at.lt(jiff_diesel::Timestamp::from(cutoff))),
		)
		.execute(db)
		.await
		.map_err(AppError::from)
	}
}

// ---------------------------------------------------------------------------
// backup_maintenance_runs — Canopy-owned maintenance outcomes (per-group)
// ---------------------------------------------------------------------------

/// One run of repository maintenance (compaction, garbage collection, etc.)
/// for a group's backup repository.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::backup_maintenance_runs)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupMaintenanceRun {
	/// Unique id of this maintenance run.
	pub id: i64,
	/// ID of the server group whose backup repository was maintained.
	pub group_id: Uuid,
	/// Which maintenance cycle this run performed: a lightweight "quick" pass
	/// or a more thorough "full" pass.
	pub kind: MaintenanceKind,
	/// When this maintenance run started.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub started_at: Timestamp,
	/// When this maintenance run finished. `None` while still running.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub finished_at: Option<Timestamp>,
	/// Outcome of the run. `None` while still running.
	pub outcome: Option<RunOutcome>,
	/// Error message for a failed run, if any.
	pub error: Option<String>,
	/// Number of bytes of unreferenced data reclaimed by this run, if known.
	pub bytes_reclaimed: Option<i64>,
}

/// Filter value for [`BackupMaintenanceRunFilters::outcome`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MaintenanceOutcomeFilter {
	/// Still running (`outcome IS NULL`).
	Running,
	Outcome(RunOutcome),
}

/// Multi-field filter for the fleet-wide maintenance-run history query. Each
/// field is opt-in: `Default` matches every run.
#[derive(Debug, Clone, Default)]
pub struct BackupMaintenanceRunFilters {
	pub group_id: Option<Uuid>,
	pub kind: Option<MaintenanceKind>,
	pub outcome: Option<MaintenanceOutcomeFilter>,
	/// When `Some`, restrict to runs started at or after this time.
	pub since: Option<Timestamp>,
}

impl BackupMaintenanceRun {
	/// Open a maintenance-run row at Job start; returns the new id for the
	/// matching [`finish`](Self::finish).
	pub async fn start(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		kind: MaintenanceKind,
	) -> Result<i64> {
		use crate::schema::backup_maintenance_runs::dsl;

		diesel::insert_into(dsl::backup_maintenance_runs)
			.values((dsl::group_id.eq(group_id), dsl::kind.eq(kind)))
			.returning(dsl::id)
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Close a maintenance run with its outcome.
	pub async fn finish(
		db: &mut AsyncPgConnection,
		id: i64,
		outcome: RunOutcome,
		error: Option<String>,
		bytes_reclaimed: Option<i64>,
	) -> Result<()> {
		use crate::schema::backup_maintenance_runs::dsl;

		diesel::update(dsl::backup_maintenance_runs.filter(dsl::id.eq(id)))
			.set((
				dsl::finished_at.eq(now),
				dsl::outcome.eq(Some(outcome)),
				dsl::error.eq(error),
				dsl::bytes_reclaimed.eq(bytes_reclaimed),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	pub async fn list_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_maintenance_runs::dsl;

		dsl::backup_maintenance_runs
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::started_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Fleet-wide maintenance-run history, newest first, narrowed by whichever
	/// filters are set. Backs the MCP `list_maintenance_runs` tool.
	pub async fn list_filtered(
		db: &mut AsyncPgConnection,
		filters: BackupMaintenanceRunFilters,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_maintenance_runs::dsl;

		let mut q = dsl::backup_maintenance_runs.into_boxed();
		if let Some(gid) = filters.group_id {
			q = q.filter(dsl::group_id.eq(gid));
		}
		if let Some(kind) = filters.kind {
			q = q.filter(dsl::kind.eq(kind));
		}
		match filters.outcome {
			Some(MaintenanceOutcomeFilter::Running) => {
				q = q.filter(dsl::outcome.is_null());
			}
			Some(MaintenanceOutcomeFilter::Outcome(o)) => {
				q = q.filter(dsl::outcome.eq(Some(o)));
			}
			None => {}
		}
		if let Some(since) = filters.since {
			q = q.filter(dsl::started_at.ge(jiff_diesel::Timestamp::from(since)));
		}
		q.order(dsl::started_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The most recently *finished* maintenance run for the group (any
	/// outcome), ignoring runs still in flight (`outcome IS NULL`). Used by the
	/// detection sweep to decide whether the latest concluded run failed.
	pub async fn latest_completed_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::backup_maintenance_runs::dsl;

		dsl::backup_maintenance_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::outcome.is_not_null())
			.order(dsl::finished_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// When the group's maintenance last completed *successfully*, or `None` if
	/// it never has. Drives prompt post-maintenance inspection: maintenance
	/// (retention pruning, compaction) changes what's actually in the repo, so
	/// the stats/snapshot inventory should freshen soon after — mirroring
	/// [`BackupRun::latest_backup_at_for_group`]'s "freshen after a backup"
	/// role. A failed run doesn't change repo contents and is already surfaced
	/// via the separate `MAINTENANCE_ERROR` alert, so only successes count
	/// here (matching the success-only convention there).
	pub async fn latest_successful_finished_at_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Option<Timestamp>> {
		use crate::schema::backup_maintenance_runs::dsl;

		let row: Option<Self> = dsl::backup_maintenance_runs
			.filter(dsl::group_id.eq(group_id))
			.filter(dsl::outcome.eq(RunOutcome::Success))
			.order_by(dsl::finished_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;
		Ok(row.and_then(|r| r.finished_at))
	}

	/// Whether a run row still exists and is open (`outcome IS NULL`). Used by
	/// the scheduler's crash-detection to mark a run failed when its Job
	/// finished without ever reporting.
	pub async fn is_open(db: &mut AsyncPgConnection, id: i64) -> Result<bool> {
		use crate::schema::backup_maintenance_runs::dsl;

		let open: Option<bool> = dsl::backup_maintenance_runs
			.filter(dsl::id.eq(id))
			.select(dsl::outcome.is_null())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;
		Ok(open.unwrap_or(false))
	}
}

// ---------------------------------------------------------------------------
// backup_repo_snapshots — ground-truth inventory from the inspection Job
// ---------------------------------------------------------------------------

/// Inventory of one backup source found in a group's repository during
/// inspection, independent of what canopy's own run records say. This is the
/// ground-truth view of what's actually stored, used to reconcile against
/// reported runs.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::backup_repo_snapshots)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRepoSnapshot {
	/// ID of the server group this repository belongs to.
	pub group_id: Uuid,
	/// Raw source identifier as recorded in the backup repository (typically
	/// combines a host and path). Used to match this inventory row back to a
	/// server and backup type.
	pub source: String,
	/// Application this source was matched to, if the source identifier could be
	/// parsed as one of the group's applications.
	pub machine_id: Option<Uuid>,
	/// Backup type this source was matched to, if the source identifier could
	/// be parsed as a known backup type.
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = Option<String>)]
	pub r#type: Option<BackupType>,
	/// Timestamp of the most recent snapshot found for this source in the
	/// repository, if any.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub latest_snapshot_at: Option<Timestamp>,
	/// When this source's inventory was last refreshed by inspection.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub observed_at: Timestamp,
}

impl BackupRepoSnapshot {
	/// Upsert the inventory for one kopia source (the inspection Job calls this
	/// per source, refreshing `latest_snapshot_at`/`observed_at` in place).
	pub async fn upsert(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		source: &str,
		machine_id: Option<Uuid>,
		r#type: Option<&BackupType>,
		latest_snapshot_at: Option<Timestamp>,
	) -> Result<()> {
		use crate::schema::backup_repo_snapshots::dsl;

		let type_str = r#type.map(BackupType::as_str);
		let latest = jiff_diesel::NullableTimestamp::from(latest_snapshot_at);

		diesel::insert_into(dsl::backup_repo_snapshots)
			.values((
				dsl::group_id.eq(group_id),
				dsl::source.eq(source),
				dsl::machine_id.eq(machine_id),
				dsl::type_.eq(type_str),
				dsl::latest_snapshot_at.eq(latest),
			))
			.on_conflict((dsl::group_id, dsl::source))
			.do_update()
			.set((
				dsl::machine_id.eq(machine_id),
				dsl::type_.eq(type_str),
				dsl::latest_snapshot_at.eq(latest),
				dsl::observed_at.eq(now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	pub async fn list_for_group(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::backup_repo_snapshots::dsl;

		dsl::backup_repo_snapshots
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::source)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// When the group's repo was last inspected (newest `observed_at` across its
	/// sources), or `None` if never. This table is written *only* by the
	/// inspection Job, so it's the clean "last inspected" signal.
	pub async fn last_inspected_at_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Option<Timestamp>> {
		use crate::schema::backup_repo_snapshots::dsl;

		let row: Option<Self> = dsl::backup_repo_snapshots
			.filter(dsl::group_id.eq(group_id))
			.order_by(dsl::observed_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;
		Ok(row.map(|r| r.observed_at))
	}
}

// ---------------------------------------------------------------------------
// backup_repo_observed_snapshots — every snapshot the last inspection saw
// ---------------------------------------------------------------------------

/// One snapshot found in a group's repository during inspection, identified the
/// way the device that created it identifies it (`backup_runs.snapshot_id`).
///
/// Where [`BackupRepoSnapshot`] summarises a source ("the newest snapshot for
/// this source is from 04:00"), this is the itemised set behind that summary, so
/// a run's reported snapshot can be looked up rather than inferred from
/// timestamps.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::backup_repo_observed_snapshots)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRepoObservedSnapshot {
	/// ID of the server group whose repository holds the snapshot.
	pub group_id: Uuid,
	/// The snapshot's own id, unique within the repository, as a device reports
	/// it on the run that created it.
	pub snapshot_id: String,
	/// Raw source identifier the snapshot belongs to, as recorded in the
	/// repository.
	pub source: String,
	/// When the snapshot was taken, if the repository records it.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub snapshot_at: Option<Timestamp>,
	/// When the inspection that observed this snapshot ran.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub observed_at: Timestamp,
}

/// One snapshot an inspection observed, for [`BackupRepoObservedSnapshot::replace_for_group`].
#[derive(Debug, Clone)]
pub struct NewObservedSnapshot {
	pub snapshot_id: String,
	pub source: String,
	pub snapshot_at: Option<Timestamp>,
}

impl BackupRepoObservedSnapshot {
	/// Replace a group's observed-snapshot set with what an inspection found.
	///
	/// The rows describe the repository as it stands, not a history: a snapshot
	/// retention has expired is gone from the repository and must be gone from
	/// here too, or reconciliation would keep matching runs against snapshots
	/// that no longer exist. So everything not in this listing is dropped, which
	/// also bounds the table at the size of the repository.
	///
	/// The whole batch carries one `observed_at`, and rows still holding an
	/// earlier one are what the listing didn't mention. Callers must pass a
	/// complete listing.
	pub async fn replace_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		observed: &[NewObservedSnapshot],
	) -> Result<()> {
		use crate::schema::backup_repo_observed_snapshots::dsl;
		use diesel::upsert::excluded;

		let stamp = jiff_diesel::Timestamp::from(Timestamp::now());

		// Chunked so a repository with tens of thousands of snapshots doesn't
		// build one statement past Postgres's bind-parameter limit.
		for chunk in observed.chunks(500) {
			let values: Vec<_> = chunk
				.iter()
				.map(|s| {
					(
						dsl::group_id.eq(group_id),
						dsl::snapshot_id.eq(&s.snapshot_id),
						dsl::source.eq(&s.source),
						dsl::snapshot_at.eq(jiff_diesel::NullableTimestamp::from(s.snapshot_at)),
						dsl::observed_at.eq(stamp),
					)
				})
				.collect();
			diesel::insert_into(dsl::backup_repo_observed_snapshots)
				.values(values)
				.on_conflict((dsl::group_id, dsl::snapshot_id))
				.do_update()
				.set((
					dsl::source.eq(excluded(dsl::source)),
					dsl::snapshot_at.eq(excluded(dsl::snapshot_at)),
					dsl::observed_at.eq(stamp),
				))
				.execute(db)
				.await
				.map_err(AppError::from)?;
		}

		diesel::delete(
			dsl::backup_repo_observed_snapshots
				.filter(dsl::group_id.eq(group_id))
				.filter(dsl::observed_at.lt(stamp)),
		)
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	/// The snapshot ids the last inspection observed in a group's repository,
	/// with when it observed them.
	///
	/// The timestamp is what makes the set usable as evidence: an id's absence
	/// only means something if the observation is newer than the run being
	/// reconciled. `None` (no rows) means there is no observation to reason
	/// from — either the group has never been inspected, or an inspection found
	/// the repository empty, which are indistinguishable here.
	pub async fn observed_ids_for_group(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<(HashSet<String>, Option<Timestamp>)> {
		use crate::schema::backup_repo_observed_snapshots::dsl;

		let rows: Vec<(String, jiff_diesel::Timestamp)> = dsl::backup_repo_observed_snapshots
			.filter(dsl::group_id.eq(group_id))
			.select((dsl::snapshot_id, dsl::observed_at))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let mut ids = HashSet::with_capacity(rows.len());
		let mut latest: Option<Timestamp> = None;
		for (id, observed_at) in rows {
			let observed_at = Timestamp::from(observed_at);
			if latest.is_none_or(|l| observed_at > l) {
				latest = Some(observed_at);
			}
			ids.insert(id);
		}
		Ok((ids, latest))
	}
}

// ---------------------------------------------------------------------------
// backup_repo_stats — cached repo + bucket stats (per-group, two writers)
// ---------------------------------------------------------------------------

/// Cached size and count statistics for a group's backup repository and its
/// underlying bucket. Populated by two independent processes: repository
/// inspection fills the snapshot/source/logical/physical figures, and a
/// separate bucket-metrics collector fills `bucket_bytes`; either can be
/// stale relative to the other.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::backup_repo_stats)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRepoStats {
	/// ID of the server group these stats describe.
	pub group_id: Uuid,
	/// Total number of snapshots currently in the repository, if known.
	pub snapshot_count: Option<i32>,
	/// Number of distinct backup sources (applications/types) currently in the
	/// repository, if known.
	pub source_count: Option<i32>,
	/// Total logical (uncompressed, pre-dedup) size of all data in the
	/// repository, in bytes, if known.
	pub logical_bytes: Option<i64>,
	/// Total physical (as actually stored, after compression/dedup) size of
	/// the repository, in bytes, if known.
	pub physical_bytes: Option<i64>,
	/// Total size of the underlying S3 bucket, in bytes, as reported by cloud
	/// storage metrics. Can differ from `physical_bytes` (e.g. it includes
	/// content outside the repository, or reflects a different point in
	/// time).
	pub bucket_bytes: Option<i64>,
	/// When the repo-derived figures (snapshot/source counts, logical/physical
	/// bytes) were last refreshed by the inspection job.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub observed_at: Timestamp,
	/// When `bucket_bytes` was last refreshed by the (daily) S3-metrics
	/// collector, if ever. Tracked separately from `observed_at` because the
	/// two figures are collected on independent cadences.
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub bucket_bytes_observed_at: Option<Timestamp>,
}

impl BackupRepoStats {
	pub async fn get(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Option<Self>> {
		use crate::schema::backup_repo_stats::dsl;

		dsl::backup_repo_stats
			.filter(dsl::group_id.eq(group_id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Writer 1: the read-only inspection Job. Touches only the repo-derived
	/// fields (+ `observed_at`), never `bucket_bytes`.
	pub async fn upsert_repo_fields(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		snapshot_count: Option<i32>,
		source_count: Option<i32>,
		logical_bytes: Option<i64>,
		physical_bytes: Option<i64>,
	) -> Result<()> {
		use crate::schema::backup_repo_stats::dsl;

		diesel::insert_into(dsl::backup_repo_stats)
			.values((
				dsl::group_id.eq(group_id),
				dsl::snapshot_count.eq(snapshot_count),
				dsl::source_count.eq(source_count),
				dsl::logical_bytes.eq(logical_bytes),
				dsl::physical_bytes.eq(physical_bytes),
			))
			.on_conflict(dsl::group_id)
			.do_update()
			.set((
				dsl::snapshot_count.eq(snapshot_count),
				dsl::source_count.eq(source_count),
				dsl::logical_bytes.eq(logical_bytes),
				dsl::physical_bytes.eq(physical_bytes),
				dsl::observed_at.eq(now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Writer 2: the S3-metrics task. Touches only `bucket_bytes`
	/// (+ `bucket_bytes_observed_at`), never the repo-derived fields or their
	/// `observed_at`.
	pub async fn upsert_bucket_bytes(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		bucket_bytes: Option<i64>,
	) -> Result<()> {
		use crate::schema::backup_repo_stats::dsl;

		diesel::insert_into(dsl::backup_repo_stats)
			.values((
				dsl::group_id.eq(group_id),
				dsl::bucket_bytes.eq(bucket_bytes),
				dsl::bucket_bytes_observed_at.eq(now),
			))
			.on_conflict(dsl::group_id)
			.do_update()
			.set((
				dsl::bucket_bytes.eq(bucket_bytes),
				dsl::bucket_bytes_observed_at.eq(now),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}

// ---------------------------------------------------------------------------
// backup_requests — pending operator one-off "backup now" flags
// ---------------------------------------------------------------------------

/// A pending one-off "run now" request for a server, outside its normal
/// schedule. Cleared once the requested run is reported.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::backup_requests)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRequest {
	/// ID of the server the one-off run is requested for.
	pub machine_id: Uuid,
	/// The backup type to run (e.g. `tamanu-postgres`).
	#[diesel(column_name = type_)]
	#[serde(rename = "type")]
	#[schema(value_type = String)]
	pub r#type: BackupType,
	/// Whether the requested run is a backup or a restore.
	pub purpose: BackupPurpose,
	/// When this request was made (or last refreshed, if re-requested).
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub requested_at: Timestamp,
	/// Who made the request, if known.
	pub requested_by: Option<String>,
}

impl BackupRequest {
	/// Enqueue (or refresh) a one-off request for a `(server, type, purpose)`.
	pub async fn enqueue(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
		purpose: BackupPurpose,
		requested_by: Option<&str>,
	) -> Result<()> {
		use crate::schema::backup_requests::dsl;

		diesel::insert_into(dsl::backup_requests)
			.values((
				dsl::machine_id.eq(machine_id),
				dsl::type_.eq(r#type.as_str()),
				dsl::purpose.eq(purpose),
				dsl::requested_by.eq(requested_by),
			))
			.on_conflict((dsl::machine_id, dsl::type_, dsl::purpose))
			.do_update()
			.set((
				dsl::requested_at.eq(now),
				dsl::requested_by.eq(requested_by),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Clear a pending request (called when the run is reported).
	pub async fn clear(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
		purpose: BackupPurpose,
	) -> Result<()> {
		use crate::schema::backup_requests::dsl;

		diesel::delete(
			dsl::backup_requests
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::type_.eq(r#type.as_str()))
				.filter(dsl::purpose.eq(purpose)),
		)
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	pub async fn pending_for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::backup_requests::dsl;

		dsl::backup_requests
			.filter(dsl::machine_id.eq(machine_id))
			.order((dsl::type_, dsl::purpose))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Whether a one-off request is pending for `(server, type, purpose)`.
	pub async fn exists(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		r#type: &BackupType,
		purpose: BackupPurpose,
	) -> Result<bool> {
		use crate::schema::backup_requests::dsl;
		use diesel::dsl::{exists, select};

		select(exists(
			dsl::backup_requests
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::type_.eq(r#type.as_str()))
				.filter(dsl::purpose.eq(purpose)),
		))
		.get_result(db)
		.await
		.map_err(AppError::from)
	}
}

// ---------------------------------------------------------------------------
// backup_recovery_verifications — the recovery vault verification-ceremony log
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::backup_recovery_verifications)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct BackupRecoveryVerification {
	pub id: i64,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub verified_at: Timestamp,
	/// The recipient fingerprints (age1… keys) this ceremony covered, as a JSON
	/// array of strings.
	pub recipients: JsonValue,
}

impl BackupRecoveryVerification {
	/// Record a successful ceremony against the given recipient fingerprints.
	pub async fn record(db: &mut AsyncPgConnection, recipients: &[String]) -> Result<Self> {
		use crate::schema::backup_recovery_verifications::dsl;

		let recipients =
			serde_json::to_value(recipients).unwrap_or_else(|_| JsonValue::Array(vec![]));
		diesel::insert_into(dsl::backup_recovery_verifications)
			.values(dsl::recipients.eq(recipients))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// The most recent verification, if any.
	pub async fn latest(db: &mut AsyncPgConnection) -> Result<Option<Self>> {
		use crate::schema::backup_recovery_verifications::dsl;

		dsl::backup_recovery_verifications
			.order(dsl::verified_at.desc())
			.select(Self::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The recipient fingerprints this verification covered.
	pub fn recipient_list(&self) -> Vec<String> {
		serde_json::from_value(self.recipients.clone()).unwrap_or_default()
	}
}
