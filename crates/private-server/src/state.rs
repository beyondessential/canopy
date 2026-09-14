use std::sync::{Arc, Mutex};

use axum::extract::FromRef;
use bestool_postgres::pool::PgPool;
use commons_errors::Result;
use commons_servers::acme::Acme;
use commons_servers::artifact_store::ArtifactStore;
use commons_servers::recovery_vault::Recipients;
use commons_servers::tailnet_directory::{TailnetDirectory, TailnetDirectoryConfig};
use commons_types::dns::ManagedZone;
use database::Db;
use public_server::state::BackupSecrets;

use crate::backup_probe::BucketProber;

/// A pending recovery vault verification challenge: the random nonce Canopy issued
/// (encrypted to the recipients) and when, so [`crate::fns::backups::recovery_verify`]
/// can match the operator's decrypted answer and reject stale ones.
#[derive(Clone, Debug)]
pub struct RecoveryChallenge {
	pub nonce: String,
	pub issued_at: jiff::Timestamp,
}

/// At most one in-flight recovery-vault challenge at a time (operator-driven).
pub type RecoveryChallengeStore = Arc<Mutex<Option<RecoveryChallenge>>>;

#[derive(Clone, Debug, FromRef)]
pub struct AppState {
	pub db: Db,
	/// Pool for read-only workloads: the RO pool when `RO_DATABASE_URL` is
	/// configured, otherwise a clone of `db`. Handlers that only ever read
	/// (list/get endpoints, the read-only MCP query interface) use this
	/// instead of `db` so that traffic can be routed to a read replica
	/// without a code change on the ops side.
	#[from_ref(skip)]
	pub db_read: Db,
	/// Which client-certificate header this server's ingress sets, and so the
	/// only one device auth may believe. Read from the environment at
	/// startup; tests construct it directly.
	pub client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader,
	pub ro_pool: Option<PgPool>,
	pub tailnet_directory: Option<TailnetDirectory>,
	/// Secret store for the per-group repo-password Secrets (onboarding creates
	/// them here). `None` in non-cluster runs ⇒ onboarding returns 502. Reuses
	/// the public-server's `BackupSecrets` (kube or in-memory).
	pub kube: Option<BackupSecrets>,
	/// STS client passed to the nested `/public` mount so device callers reaching
	/// it can issue backup credentials (`AssumeRole`) like the standalone public
	/// server. `None` in non-AWS/test runs.
	pub sts: Option<aws_sdk_sts::Client>,
	/// Bucket prober for the setup wizard (assume role + inspect S3). `Aws` in
	/// prod; a `Fake` canned result in tests / the e2e binary.
	pub prober: BucketProber,
	/// Where the bytes of the artifacts Canopy holds rest. `None` where no
	/// bucket is configured, in which case registering or serving one reports
	/// that rather than the server failing to start.
	// spec: ART#where-an-artifact-rests
	pub artifacts: Option<ArtifactStore>,
	/// recovery vault recipient public keys (`CANOPY_RECOVERY_VAULT_KEYS`), for the
	/// verification ceremony. `None` ⇒ the ceremony endpoints 502 (the backups
	/// pod is what hard-requires them, not this admin server).
	pub recovery_recipients: Option<Recipients>,
	/// The single in-flight recovery verification challenge, if any.
	pub recovery_challenge: RecoveryChallengeStore,
	/// The DNS zones Canopy may write records in, from its instance
	/// configuration. Empty when none are configured, in which case no group
	/// domain can be claimed — read once at startup, so a configuration change
	/// takes effect on restart.
	// spec: DOM#managed-zones
	#[from_ref(skip)]
	pub dns_zones: Vec<ManagedZone>,
	/// Canopy's account at the certificate authority, where one is configured.
	/// The admin server needs it for two things only: the profiles the authority
	/// advertises, and revoking a certificate on an operator's say-so. Everything
	/// else about issuance is the domains pod's.
	///
	/// `None` where no account key is configured, or where the account could not
	/// be reached at startup — in which case revocation reports that rather than
	/// pretending to have worked.
	// spec: CRT#revocation
	#[from_ref(skip)]
	pub acme: Option<Acme>,
	/// The authority's directory URL as configured, kept even when the account
	/// could not be built: an operator looking at a broken authority wants to see
	/// which one Canopy was trying to use.
	#[from_ref(skip)]
	pub acme_directory: Option<String>,
}

/// Environment variable that swaps in the in-process fake certificate authority,
/// for the e2e binary. Debug-only, like [`crate::backup_probe::FAKE_ENV`].
pub const FAKE_ACME_ENV: &str = "CANOPY_FAKE_ACME";

/// Build Canopy's ACME account, logging (not failing) a configuration that does
/// not work: the admin server should still come up, and the settings panel
/// surfaces the problem. Returns the configured directory URL either way.
async fn acme_from_env() -> (Option<Acme>, Option<String>) {
	if std::env::var_os(FAKE_ACME_ENV).is_some() {
		// Debug-only. The fake authority signs with a throwaway root, so a
		// release build must never be able to serve one: a certificate nothing
		// trusts, presented as valid, is worse than none.
		#[cfg(debug_assertions)]
		{
			tracing::warn!("{FAKE_ACME_ENV} set; using the in-process fake certificate authority");
			return (
				Some(Acme::fake()),
				Some("https://acme.test.invalid/directory".into()),
			);
		}
		#[cfg(not(debug_assertions))]
		tracing::error!(
			"{FAKE_ACME_ENV} is set but IGNORED: the fake certificate authority is debug-only and \
			 is never used in release builds"
		);
	}

	let directory = std::env::var("CANOPY_ACME_DIRECTORY").ok();
	match Acme::from_env().await {
		Ok(acme) => (acme, directory),
		Err(err) => {
			tracing::warn!("Canopy's certificate authority account is unusable: {err}");
			(None, directory)
		}
	}
}

/// Read the managed DNS zones from the environment, logging (not failing) a
/// malformed list: the admin server should still come up, and the domain
/// endpoints surface the misconfiguration as "no zones configured".
fn dns_zones_from_env() -> Vec<ManagedZone> {
	match ManagedZone::list_from_env() {
		Ok(zones) => zones,
		Err(e) => {
			tracing::warn!("ignoring malformed {}: {e}", commons_types::dns::ZONES_ENV);
			Vec::new()
		}
	}
}

/// Read the recovery recipients from the environment, logging (not failing) a malformed
/// list — the admin server should still come up; the ceremony endpoints surface
/// the misconfiguration.
fn recovery_recipients_from_env() -> Option<Recipients> {
	match Recipients::from_env() {
		Ok(r) => r,
		Err(e) => {
			tracing::warn!(
				"ignoring malformed {}: {e}",
				commons_servers::recovery_vault::RECIPIENTS_ENV
			);
			None
		}
	}
}

/// Put canopy's own always-present checks in the catalog at startup, so
/// the reachability check presents (and can be silenced) on a fleet where
/// nothing has ever gone wrong. Best-effort: a database that isn't up yet
/// shouldn't stop the server from starting, and the monitor registers the
/// same row the first time it files.
async fn seed_own_checks(db: &database::Db) {
	let seeded = match db.get().await {
		Ok(mut conn) => database::check_policies::CheckPolicy::seed_own_checks(&mut conn).await,
		Err(err) => {
			tracing::warn!("seeding canopy's own checks: no database connection: {err}");
			return;
		}
	};
	if let Err(err) = seeded {
		tracing::warn!("seeding canopy's own checks failed: {err}");
	}
}

impl AppState {
	pub async fn init() -> Result<Self> {
		let ro_pool = if let Ok(url) = std::env::var("RO_DATABASE_URL") {
			(bestool_postgres::pool::create_pool(&url, "canopy-playground").await).ok()
		} else {
			None
		};

		let tailnet_directory = match TailnetDirectoryConfig::from_env()? {
			Some(config) => Some(TailnetDirectory::new(config).await?),
			None => None,
		};

		let kube = BackupSecrets::try_default().await;
		let prober = BucketProber::try_default().await;
		let artifacts = ArtifactStore::try_default().await;
		// For the nested `/public` mount's backup-credential issuance. Building
		// the client needs no creds (they resolve per-call from the pod's IRSA
		// identity), so this is always `Some` in a real run.
		let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		let sts = Some(aws_sdk_sts::Client::new(&aws));

		let (acme, acme_directory) = acme_from_env().await;

		let db = database::init();
		let db_read = database::init_ro().unwrap_or_else(|| db.clone());
		seed_own_checks(&db).await;

		Ok(Self {
			client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::from_env(),
			db,
			db_read,
			ro_pool,
			tailnet_directory,
			kube,
			sts,
			prober,
			artifacts,
			recovery_recipients: recovery_recipients_from_env(),
			recovery_challenge: Arc::new(Mutex::new(None)),
			dns_zones: dns_zones_from_env(),
			acme,
			acme_directory,
		})
	}

	/// Test/e2e-only AppState builder. Debug-only because it constructs the
	/// debug-only in-memory Secret store and fake prober (`BackupSecrets::memory`
	/// / `BucketProber::fake`), which don't exist in release builds.
	#[cfg(debug_assertions)]
	pub async fn from_db_url(url: &str) -> Result<Self> {
		let db = database::init_to(url);
		seed_own_checks(&db).await;
		Ok(Self {
			client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::from_env(),
			db_read: db.clone(),
			db,
			ro_pool: None,
			tailnet_directory: None,
			// In-memory secret store so onboarding (Secret creation) is exercised
			// in tests without a cluster.
			kube: Some(BackupSecrets::memory()),
			// No real STS in tests; the nested-mount issuance path isn't exercised.
			sts: None,
			// Bucket-name-derived fake prober (matches the e2e fixture): a test
			// drives each probe state by naming the bucket — `…existing…` → kopia
			// repo, `…other…` → other content, `…denied…` → inaccessible, else empty.
			prober: BucketProber::Fake(None),
			// In-process artifact store so upload and download are exercised in
			// tests and the e2e fixture without a bucket.
			artifacts: Some(ArtifactStore::memory()),
			// Read from env so the e2e fixture can exercise the recovery ceremony.
			recovery_recipients: recovery_recipients_from_env(),
			recovery_challenge: Arc::new(Mutex::new(None)),
			dns_zones: dns_zones_from_env(),
			// A fake authority in tests and the e2e fixture: it advertises profiles
			// and accepts revocations, so both paths are exercisable without a
			// network.
			acme: Some(Acme::fake()),
			acme_directory: Some("https://acme.test.invalid/directory".into()),
		})
	}
}
