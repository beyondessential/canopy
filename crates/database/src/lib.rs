use std::time::Duration;

use commons_types::backoff::Backoff;
use diesel_async::{
	AsyncPgConnection,
	pooled_connection::{
		AsyncDieselConnectionManager, PoolError,
		mobc::{Pool, PooledConnection},
	},
};
use jiff::SignedDuration;

pub mod admins;
pub mod application_certificates;
pub mod application_names;
pub mod applications;
pub mod artifacts;
pub mod backup;
pub mod backups;
pub mod bestool_snippets;
pub mod certificate_alerts;
pub mod check_instances;
pub mod check_policies;
pub mod chrome_releases;
pub mod devices;
pub mod dns_name_dispositions;
pub mod inventory_leases;
pub mod inventory_variables;
pub mod issues;
pub mod kubernetes_clusters;
pub mod machine_enrollment_challenges;
pub mod machine_enrollment_tokens;
pub mod machines;
pub mod maintenance_windows;
pub mod mcp_tokens;
pub mod migration_tests;
pub mod notes;
pub mod operator_sessions;
pub mod partitions;
pub mod pg_duration;
pub mod recovery_vault;
pub mod reported_detail;
pub mod reporting_schemas;
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
pub use dns_name_dispositions::{AskedFor, DeniedDnsName, UndeclaredDnsName};
pub use kubernetes_clusters::KubernetesCluster;
pub use machines::{Machine, MachineUpdate, NewMachine};
pub use operator_sessions::OperatorSession;
pub use recovery_vault::RecoveryVaultWrite;
pub use restore::{
	BackupRestoreCheck, NewBackupRestoreCheck, NewRestoreReplica, RestoreConsumerCapability,
	RestoreReplica, RestoreReplicaUpdate,
};
pub use server_domains::ServerGroupDomain;

/// A pool of connections to one database role (primary or read-only).
///
/// Wraps the bare `mobc` pool so checkout can retry a transient connect
/// failure (see [`Db::get`]) without changing any of the ~230 call sites
/// across the workspace that only ever call `.get()` or `.clone()` on this
/// type — both derives below are free, since `mobc::Pool` already
/// implements them.
#[derive(Clone, Debug)]
pub struct Db(Pool<AsyncPgConnection>);

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
	Db(Pool::builder()
		.max_open(max_open)
		.max_idle(max_idle)
		.max_lifetime(Some(Duration::from_secs(30 * 60)))
		.max_idle_lifetime(Some(Duration::from_secs(10 * 60)))
		// Bounds a single checkout attempt, so a connect that hangs rather
		// than fails cleanly can't make Db::get's retry loop run for minutes
		// instead of the ~20s it's sized for.
		.get_timeout(Some(Duration::from_secs(3)))
		.build(AsyncDieselConnectionManager::<AsyncPgConnection>::new(url)))
}

fn env_u64(key: &str, default: u64) -> u64 {
	std::env::var(key)
		.ok()
		.and_then(|v| v.parse().ok())
		.unwrap_or(default)
}

/// Doubling backoff between pool checkout attempts: 250ms, 500ms, 1s, then
/// held at the 2s cap (itself below the pool's 3s per-attempt
/// `get_timeout` above, so the two bounds don't fight each other) for the
/// rest of [`CHECKOUT_MAX_ATTEMPTS`].
const CHECKOUT_RETRY: Backoff = Backoff::new(
	SignedDuration::from_millis(250),
	SignedDuration::from_secs(2),
);

/// Chosen so the cumulative wait across the schedule above lands just under
/// the ~20s primary-failover window confirmed in prod (see the
/// `checkout_backoff_window` test for the pinned total).
const CHECKOUT_MAX_ATTEMPTS: u32 = 13;

impl Db {
	/// Checks out a connection, retrying a failed attempt with a growing
	/// wait instead of surfacing it on the first failure.
	///
	/// Covers a brief loss of the primary during a failover: a request that
	/// can get a connection within the retry window proceeds normally. A
	/// database that's still unreachable once the window is exhausted fails
	/// exactly as it would have without retrying — same error, same shape —
	/// so `AppError::DatabasePool` and everything downstream of it is
	/// unaffected either way.
	pub async fn get(&self) -> Result<PooledConnection<AsyncPgConnection>, mobc::Error<PoolError>> {
		self.get_retrying(CHECKOUT_RETRY, CHECKOUT_MAX_ATTEMPTS)
			.await
	}

	async fn get_retrying(
		&self,
		retry: Backoff,
		max_attempts: u32,
	) -> Result<PooledConnection<AsyncPgConnection>, mobc::Error<PoolError>> {
		let mut attempt = 1;
		loop {
			match self.0.get().await {
				Ok(conn) => return Ok(conn),
				Err(err) if attempt < max_attempts => {
					tracing::warn!(attempt, %err, "db checkout failed, retrying");
					tokio::time::sleep(retry.after(attempt).unsigned_abs()).await;
					attempt += 1;
				}
				Err(err) => return Err(err),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	// Pins the arithmetic behind CHECKOUT_RETRY / CHECKOUT_MAX_ATTEMPTS, so a
	// change to either shows up here rather than only in behaviour nobody
	// can see until a real failover happens.
	#[test]
	fn checkout_backoff_window() {
		assert_eq!(CHECKOUT_RETRY.after(1), SignedDuration::from_millis(250));
		assert_eq!(CHECKOUT_RETRY.after(2), SignedDuration::from_millis(500));
		assert_eq!(CHECKOUT_RETRY.after(3), SignedDuration::from_secs(1));
		// Capped from the fourth attempt on (250ms * 2^3 == the 2s cap).
		assert_eq!(CHECKOUT_RETRY.after(4), SignedDuration::from_secs(2));
		assert_eq!(
			CHECKOUT_RETRY.after(CHECKOUT_MAX_ATTEMPTS),
			SignedDuration::from_secs(2)
		);

		let total: Duration = (1..CHECKOUT_MAX_ATTEMPTS)
			.map(|attempt| CHECKOUT_RETRY.after(attempt).unsigned_abs())
			.sum();
		// Just under the ~20s failover window observed in prod (DBR), with
		// enough margin below it that an operator watching logs can still
		// tell a passing switchover from a database that's actually down.
		assert_eq!(total, Duration::from_millis(19_750));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn get_succeeds_on_a_healthy_pool_without_retry() {
		commons_tests::db::TestDb::run(async |_conn, url| {
			let db = init_to(&url);
			db.get()
				.await
				.expect("a healthy pool should check out a connection on the first attempt");
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn get_exhausts_retries_and_fails_like_a_non_retrying_pool() {
		// Nothing listens on this port, so every attempt fails fast
		// (connection refused). Shrink the schedule so the test doesn't
		// spend the real ~20s the production constants are sized for.
		let db = init_to("postgres://127.0.0.1:1/nonexistent");
		let tiny = Backoff::new(
			SignedDuration::from_millis(1),
			SignedDuration::from_millis(5),
		);

		// `PooledConnection` isn't `Debug`, so `expect_err` (which requires
		// the `Ok` side to be) doesn't fit here — match it out instead.
		let no_retry = match db.0.get().await {
			Err(err) => err,
			Ok(_) => panic!("unexpectedly connected to nothing listening on this port"),
		};
		let retried = match db.get_retrying(tiny, 3).await {
			Err(err) => err,
			Ok(_) => panic!("unexpectedly connected after retrying against nothing listening"),
		};

		// Same error shape either way: retrying only changes when the
		// failure is reported once the window is exhausted, not what gets
		// reported.
		assert_eq!(no_retry.to_string(), retried.to_string());
	}
}
