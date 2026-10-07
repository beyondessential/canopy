use std::time::Duration;

use commons_types::backoff::Backoff;
use diesel::ConnectionError;
use diesel_async::{
	AsyncPgConnection,
	pooled_connection::{AsyncDieselConnectionManager, PoolError, PoolableConnection},
};
use jiff::SignedDuration;
use tokio::time::{Instant, timeout};

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
/// failure (see [`Db::get`]) without changing any of the call sites across
/// the workspace, which only ever call `.get()` or `.clone()` on this type.
#[derive(Clone, Debug)]
pub struct Db(mobc::Pool<ConnectionManager>);

/// A connection checked out of a [`Db`].
pub type PooledConnection = mobc::Connection<ConnectionManager>;

/// diesel-async's mobc manager, with the check-in validation corrected and a
/// time limit on each round trip a checkout makes to the server.
///
/// diesel-async 0.9's `validate` returns `is_broken()`, but mobc keeps a
/// connection when `validate` returns true. So it discards every healthy
/// connection on check-in and returns the broken ones to the idle list,
/// including any left inside a transaction by a cancelled request.
#[derive(Debug)]
pub struct ConnectionManager(AsyncDieselConnectionManager<AsyncPgConnection>);

/// Limits each connect and each check-out health check. A dropped node
/// doesn't refuse connections, it just never answers, and without this an
/// attempt against one waits for the operating system's TCP timeout.
const SERVER_TIMEOUT: Duration = Duration::from_secs(3);

#[mobc::async_trait]
impl mobc::Manager for ConnectionManager {
	type Connection = AsyncPgConnection;
	type Error = PoolError;

	async fn connect(&self) -> Result<Self::Connection, Self::Error> {
		timeout(SERVER_TIMEOUT, mobc::Manager::connect(&self.0))
			.await
			.unwrap_or_else(|_| {
				Err(PoolError::ConnectionError(ConnectionError::BadConnection(
					format!("timed out connecting after {SERVER_TIMEOUT:?}"),
				)))
			})
	}

	async fn check(&self, conn: Self::Connection) -> Result<Self::Connection, Self::Error> {
		timeout(SERVER_TIMEOUT, mobc::Manager::check(&self.0, conn))
			.await
			.unwrap_or_else(|_| {
				Err(PoolError::ConnectionError(ConnectionError::BadConnection(
					format!("health check timed out after {SERVER_TIMEOUT:?}"),
				)))
			})
	}

	fn validate(&self, conn: &mut Self::Connection) -> bool {
		!std::thread::panicking() && !conn.is_broken()
	}
}

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
	pool(url, env_u64(max_open_key, 5), env_u64(max_idle_key, 2))
}

fn pool(url: &str, max_open: u64, max_idle: u64) -> Db {
	Db(mobc::Pool::builder()
		.max_open(max_open)
		.max_idle(max_idle)
		.max_lifetime(Some(Duration::from_secs(30 * 60)))
		.max_idle_lifetime(Some(Duration::from_secs(10 * 60)))
		// Each checkout passes its own limit (see Db::get); this is only the
		// fallback for anything reaching the pool another way.
		.get_timeout(Some(CHECKOUT.timeout))
		.build(ConnectionManager(AsyncDieselConnectionManager::new(url))))
}

fn env_u64(key: &str, default: u64) -> u64 {
	std::env::var(key)
		.ok()
		.and_then(|v| v.parse().ok())
		.unwrap_or(default)
}

/// How a checkout waits and retries.
#[derive(Clone, Copy, Debug)]
struct Checkout {
	/// Wait between attempts after a failure to reach the server.
	backoff: Backoff,
	/// No retry starts once this long has passed since the first attempt.
	retry_window: Duration,
	/// The longest a checkout takes in total, retries and waiting for a free
	/// connection included.
	timeout: Duration,
}

/// The retry window covers the primary failovers seen in production, which
/// take up to about 20s. The timeout is mobc's default, so a request waiting
/// for a connection another request holds waits as long as it always has.
const CHECKOUT: Checkout = Checkout {
	backoff: Backoff::new(
		SignedDuration::from_millis(250),
		SignedDuration::from_secs(2),
	),
	retry_window: Duration::from_secs(20),
	timeout: Duration::from_secs(30),
};

impl Db {
	/// Checks out a connection, retrying a failure to reach the server with
	/// a growing wait instead of surfacing it on the first failure.
	///
	/// Covers a brief loss of the primary during a failover: a request that
	/// can get a connection within the retry window proceeds normally. A
	/// database that's still unreachable once the window is over fails
	/// exactly as it would have without retrying, so `AppError::DatabasePool`
	/// and everything downstream of it is unaffected either way.
	///
	/// Only connect failures are retried. A checkout that timed out waiting
	/// for a connection other requests hold has already waited its full
	/// limit, and a closed pool won't reopen.
	pub async fn get(&self) -> Result<PooledConnection, mobc::Error<PoolError>> {
		self.get_with(CHECKOUT).await
	}

	async fn get_with(
		&self,
		checkout: Checkout,
	) -> Result<PooledConnection, mobc::Error<PoolError>> {
		let start = Instant::now();
		let deadline = start + checkout.timeout;
		let retry_until = start + checkout.retry_window;
		let mut attempt = 1;
		loop {
			let remaining = deadline.saturating_duration_since(Instant::now());
			match self.0.get_timeout(remaining).await {
				Ok(conn) => return Ok(conn),
				Err(err @ mobc::Error::Inner(_)) => {
					let wait = checkout.backoff.after(attempt).unsigned_abs();
					if Instant::now() + wait >= retry_until {
						return Err(err);
					}
					tracing::warn!(attempt, %err, "db checkout failed, retrying");
					tokio::time::sleep(wait).await;
					attempt += 1;
				}
				Err(err) => return Err(err),
			}
		}
	}
}

#[cfg(test)]
mod tests {
	use diesel_async::{AsyncConnection, TransactionManager};

	use super::*;

	/// A schedule small enough that exhausting it costs the suite nothing.
	const QUICK: Checkout = Checkout {
		backoff: Backoff::new(
			SignedDuration::from_millis(1),
			SignedDuration::from_millis(10),
		),
		retry_window: Duration::from_millis(200),
		timeout: Duration::from_secs(2),
	};

	/// Nothing listens here, so every connect is refused straight away.
	const UNREACHABLE: &str = "postgres://127.0.0.1:1/nonexistent";

	/// Check-in runs on a spawned task, so give it a moment to land.
	async fn settled_state(db: &Db) -> mobc::State {
		let give_up = Instant::now() + Duration::from_secs(2);
		loop {
			let state = db.0.state().await;
			if state.in_use == 0 || Instant::now() > give_up {
				return state;
			}
			tokio::time::sleep(Duration::from_millis(10)).await;
		}
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn get_succeeds_on_a_healthy_pool() {
		commons_tests::db::TestDb::run(async |_conn, url| {
			let db = init_to(&url);
			db.get()
				.await
				.expect("a healthy pool should check out a connection on the first attempt");
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_returned_connection_is_reused() {
		commons_tests::db::TestDb::run(async |_conn, url| {
			let db = pool(&url, 5, 2);
			drop(db.get().await.expect("first checkout"));
			let state = settled_state(&db).await;
			assert_eq!((state.connections, state.idle), (1, 1), "{state:?}");

			drop(db.get().await.expect("second checkout"));
			let state = settled_state(&db).await;
			assert_eq!(
				state.connections, 1,
				"second checkout reused the first: {state:?}"
			);
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_connection_left_in_a_transaction_is_discarded() {
		commons_tests::db::TestDb::run(async |_conn, url| {
			let db = pool(&url, 5, 2);
			let mut conn = db.get().await.expect("checkout");
			<AsyncPgConnection as AsyncConnection>::TransactionManager::begin_transaction(
				&mut *conn,
			)
			.await
			.expect("begin");
			drop(conn);

			let state = settled_state(&db).await;
			assert_eq!((state.connections, state.idle), (0, 0), "{state:?}");
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn an_unreachable_server_is_retried_for_the_window_then_fails_as_without_retry() {
		let db = pool(UNREACHABLE, 5, 2);

		// `PooledConnection` isn't `Debug`, so `expect_err` (which requires
		// the `Ok` side to be) doesn't fit here; match it out instead.
		let no_retry = match db.0.get().await {
			Err(err) => err,
			Ok(_) => panic!("unexpectedly connected to nothing listening on this port"),
		};

		let start = Instant::now();
		let retried = match db.get_with(QUICK).await {
			Err(err) => err,
			Ok(_) => panic!("unexpectedly connected after retrying against nothing listening"),
		};
		let elapsed = start.elapsed();

		assert!(
			elapsed >= QUICK.retry_window - QUICK.backoff.cap().unsigned_abs(),
			"gave up after {elapsed:?}, before the retry window was used"
		);
		assert!(elapsed < QUICK.timeout, "ran on for {elapsed:?}");
		assert_eq!(no_retry.to_string(), retried.to_string());
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_server_that_never_answers_fails_the_attempt_at_its_limit() {
		// Accepts connections and never says anything, like a node that has
		// dropped off the network mid-handshake.
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.expect("bind");
		let url = format!("postgres://{}/silent", listener.local_addr().expect("addr"));
		let silent = tokio::spawn(async move {
			let mut held = Vec::new();
			while let Ok((socket, _)) = listener.accept().await {
				held.push(socket);
			}
		});

		let one_attempt = Checkout {
			retry_window: Duration::ZERO,
			timeout: Duration::from_secs(30),
			..QUICK
		};
		let start = Instant::now();
		let err = match pool(&url, 5, 2).get_with(one_attempt).await {
			Err(err) => err,
			Ok(_) => panic!("connected to a server that never answers"),
		};
		let elapsed = start.elapsed();
		silent.abort();

		assert!(matches!(err, mobc::Error::Inner(_)), "{err}");
		assert!(
			elapsed >= SERVER_TIMEOUT && elapsed < SERVER_TIMEOUT * 2,
			"attempt ended after {elapsed:?}"
		);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_server_that_comes_back_within_the_window_is_reached() {
		commons_tests::db::TestDb::run(async |_conn, url| {
			let target = url::Url::parse(&url).expect("test url");
			let upstream = format!(
				"{}:{}",
				target.host_str().expect("host"),
				target.port().unwrap_or(5432)
			);

			// Reserve a port, then leave it closed so connects are refused
			// until the proxy below starts listening on it.
			let port = std::net::TcpListener::bind("127.0.0.1:0")
				.expect("reserve")
				.local_addr()
				.expect("addr")
				.port();
			let mut via_proxy = target.clone();
			via_proxy.set_host(Some("127.0.0.1")).expect("host");
			via_proxy.set_port(Some(port)).expect("port");

			let proxy = tokio::spawn(async move {
				tokio::time::sleep(Duration::from_millis(100)).await;
				let listener = tokio::net::TcpListener::bind(("127.0.0.1", port))
					.await
					.expect("proxy bind");
				while let Ok((mut client, _)) = listener.accept().await {
					let upstream = upstream.clone();
					tokio::spawn(async move {
						let mut server = tokio::net::TcpStream::connect(upstream)
							.await
							.expect("upstream");
						let _ = tokio::io::copy_bidirectional(&mut client, &mut server).await;
					});
				}
			});

			let comes_back = Checkout {
				retry_window: Duration::from_secs(5),
				..QUICK
			};
			let db = pool(via_proxy.as_str(), 5, 2);
			let mut conn = match db.get_with(comes_back).await {
				Ok(conn) => conn,
				Err(err) => panic!("server came back but checkout failed: {err}"),
			};
			diesel_async::SimpleAsyncConnection::batch_execute(&mut *conn, "SELECT 1")
				.await
				.expect("query over the recovered connection");
			drop(conn);
			proxy.abort();
		})
		.await;
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn waiting_for_a_busy_pool_is_not_retried() {
		commons_tests::db::TestDb::run(async |_conn, url| {
			let db = pool(&url, 1, 1);
			let _held = db.get().await.expect("take the only connection");

			let busy = Checkout {
				timeout: Duration::from_millis(200),
				retry_window: Duration::from_secs(10),
				..QUICK
			};
			let start = Instant::now();
			let err = match db.get_with(busy).await {
				Err(err) => err,
				Ok(_) => panic!("checked out a second connection from a pool of one"),
			};

			assert!(matches!(err, mobc::Error::Timeout), "{err}");
			assert!(
				start.elapsed() < Duration::from_secs(1),
				"{:?}",
				start.elapsed()
			);
		})
		.await;
	}
}
