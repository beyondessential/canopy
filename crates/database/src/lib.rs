use std::time::Duration;

use diesel_async::{
	AsyncPgConnection,
	pooled_connection::{AsyncDieselConnectionManager, mobc::Pool},
};

pub mod admins;
pub mod application_certificates;
pub mod application_names;
pub mod applications;
pub mod artifacts;
pub mod backup;
pub mod backups;
pub mod bestool_snippets;
pub mod certificate_alerts;
pub mod check_policies;
pub mod chrome_releases;
pub mod devices;
pub mod inventory_leases;
pub mod inventory_variables;
pub mod issues;
pub mod machine_enrollment_challenges;
pub mod machine_enrollment_tokens;
pub mod machines;
pub mod maintenance_windows;
pub mod mcp_tokens;
pub mod migration_tests;
pub mod notes;
pub mod partitions;
pub mod pg_duration;
pub mod recovery_vault;
pub mod reported_detail;
pub mod restore;
pub mod schema;
pub mod self_alerts;
pub mod server_domains;
pub mod server_groups;
pub mod silenced_refs;
pub mod slack_outbox;
pub mod source_policies;
pub mod sql_playground_history;
pub mod stability;
pub mod statuses;
pub mod tags;
pub mod tailscale_users;
pub mod upgrade_plans;
pub mod url_field;
pub mod version_known_issues;
pub mod versions;
pub mod views;

pub use application_certificates::{ApplicationCertificate, OrderState, RevocationReason, Risk};
pub use application_names::ApplicationName;
pub use backups::{
	BackupCredentialIssuance, BackupMaintenanceRun, BackupMaintenanceRunFilters,
	BackupRecoveryVerification, BackupRepoObservedSnapshot, BackupRepoSnapshot, BackupRepoStats,
	BackupRequest, BackupRun, BackupRunFilters, BackupRunProgress, BackupTypeDefault,
	MachineBackupCapability, MaintenanceOutcomeFilter, NewBackupCredentialIssuance, NewBackupRun,
	NewBackupRunProgress, NewBackupTypeDefault, NewObservedSnapshot, NewServerGroupBackupConfig,
	NewServerGroupBackupSchedule, RetentionPolicy, ServerGroupBackupConfig,
	ServerGroupBackupSchedule,
};
pub use bestool_snippets::{BestoolSnippet, NewBestoolSnippet};
pub use commons_types::backup::{
	BackupConfigStatus, BackupPurpose, BackupRepoMode, BackupType, MaintenanceKind, RestoreIntent,
	RunOutcome,
};
pub use devices::{Device, DeviceConnection, DeviceKey, DeviceWithInfo};
pub use machines::{Machine, MachineUpdate, NewMachine};
pub use recovery_vault::RecoveryVaultWrite;
pub use restore::{
	BackupRestoreCheck, NewBackupRestoreCheck, NewRestoreReplica, RestoreConsumerCapability,
	RestoreReplica, RestoreReplicaUpdate,
};
pub use server_domains::ServerGroupDomain;

pub type Db = Pool<AsyncPgConnection>;

// Re-export for use in other crates
pub use diesel_async;

pub fn init() -> Db {
	init_to(&std::env::var("DATABASE_URL").expect("DATABASE_URL must be set"))
}

pub fn init_to(url: &str) -> Db {
	build_pool(url, "DB_MAX_OPEN_CONNECTIONS", "DB_MAX_IDLE_CONNECTIONS")
}

/// A second pool for workloads that only ever read, built against
/// `RO_DATABASE_URL` when it's set. Routing reads off the primary pool keeps
/// read traffic from starving writers of connections, and lets ops later
/// point the var at an actual read replica without a code change. `None`
/// when the var is unset — callers fall back to the primary pool.
pub fn init_ro() -> Option<Db> {
	std::env::var("RO_DATABASE_URL")
		.ok()
		.map(|url| init_ro_to(&url))
}

pub fn init_ro_to(url: &str) -> Db {
	build_pool(
		url,
		"DB_RO_MAX_OPEN_CONNECTIONS",
		"DB_RO_MAX_IDLE_CONNECTIONS",
	)
}

// Bound the pool. Every pod that links this crate (the two applications plus each
// job) runs its own pool against the same backend; mobc's defaults
// (max_open=10, idle and lifetimes uncapped) let the fleet's aggregate demand
// exceed the server's max_connections and pin backends indefinitely. Size per
// role via env, and recycle connections so a failover doesn't leave the pool
// holding dead backends.
fn build_pool(url: &str, max_open_key: &str, max_idle_key: &str) -> Db {
	let max_open = env_u64(max_open_key, 5);
	let max_idle = env_u64(max_idle_key, 2);
	Pool::builder()
		.max_open(max_open)
		.max_idle(max_idle)
		.max_lifetime(Some(Duration::from_secs(30 * 60)))
		.max_idle_lifetime(Some(Duration::from_secs(10 * 60)))
		.build(AsyncDieselConnectionManager::<AsyncPgConnection>::new(url))
}

fn env_u64(key: &str, default: u64) -> u64 {
	std::env::var(key)
		.ok()
		.and_then(|v| v.parse().ok())
		.unwrap_or(default)
}
