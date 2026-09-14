#[cfg(feature = "ui")]
use std::sync::Arc;

use axum::extract::FromRef;
use commons_errors::Result;
use commons_servers::tailnet_directory::TailnetDirectory;
use database::Db;
#[cfg(feature = "ui")]
use tera::Tera;

/// The per-group repo-password Secret store now lives in `commons-applications`;
/// re-exported so existing `public_server::state::BackupSecrets` consumers (and
/// the `AppState.kube` field) keep working.
pub use commons_servers::backup_secrets::BackupSecrets;

#[derive(Clone, Debug)]
pub struct AppState {
	pub db: Db,
	/// Pool for read-only workloads: the RO pool when `RO_DATABASE_URL` is
	/// configured, otherwise a clone of `db`. Used by the internet-facing
	/// read-only MCP mount (`crate::mcp`); the device-facing endpoints under
	/// `crate::routes` all write (status/event ingestion, credential
	/// issuance) and stay on `db`.
	pub db_read: Db,
	/// Which client-certificate header this server's ingress sets, and so the
	/// only one device auth may believe. Read from the environment at
	/// startup; tests construct it directly.
	pub client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader,
	#[cfg(feature = "ui")]
	pub tera: Arc<Tera>,
	/// Gates the public reads that carry fleet information to an audience the
	/// tailnet cannot reach: the server-versions page and the planned-upgrades
	/// calendar feed. Unset ⇒ both 404 rather than serving ungated.
	pub server_versions_secret: Option<String>,
	/// Populated only when the public-server's router is nested into
	/// the private-server's `/public/...` mount and the private-server
	/// has a directory configured. The internet-facing public-server
	/// binary's `init()` leaves this `None`, so the tailnet path of
	/// the device-auth extractor can never fire on the open internet.
	pub tailnet_directory: Option<TailnetDirectory>,
	/// In-process rate limiter backing the unauthenticated enrollment
	/// endpoints (per source IP and per target server).
	pub rate_limiter: crate::ratelimit::RateLimiter,
	/// STS client built from the pod's IRSA web-identity. `None` when no AWS
	/// environment is configured (tests, the nested private mount); absent ⇒
	/// `POST /backup-credentials` returns 502 ("issuer not configured").
	pub sts: Option<aws_sdk_sts::Client>,
	/// Kube client for reading repo-password Secrets in canopy's namespace.
	/// `None` in tests / non-cluster runs ⇒ `GET /backup-target` returns 502.
	pub kube: Option<BackupSecrets>,
	/// Where the bytes of the artifacts Canopy holds rest. `None` where no
	/// bucket is configured, in which case serving one reports that rather than
	/// the edge failing to start.
	// spec: ART#where-an-artifact-rests
	pub artifacts: Option<commons_servers::artifact_store::ArtifactStore>,
	/// The DNS zones Canopy may write records in, from its instance
	/// configuration. Empty when none are configured, in which case no name can
	/// be acted on — read once at startup, so a change takes effect on restart.
	// spec: CRT
	pub dns_zones: Vec<commons_types::dns::ManagedZone>,
}

/// Read the managed DNS zones from the environment, logging (not failing) a
/// malformed list: the edge should still come up, and the name endpoints report
/// the misconfiguration as "no zone covers this name".
// spec: CRT
fn dns_zones_from_env() -> Vec<commons_types::dns::ManagedZone> {
	match commons_types::dns::ManagedZone::list_from_env() {
		Ok(zones) => zones,
		Err(e) => {
			tracing::warn!("ignoring malformed {}: {e}", commons_types::dns::ZONES_ENV);
			Vec::new()
		}
	}
}

impl AppState {
	#[cfg(feature = "ui")]
	pub fn init_tera() -> Result<Arc<Tera>> {
		let mut tera = Tera::default();

		macro_rules! embed_template {
			($name:expr) => {
				tera.add_raw_template(
					$name,
					include_str!(concat!("../templates/", $name, ".html.tera")),
				)
				.unwrap();
			};
		}

		embed_template!("artifacts");
		embed_template!("mobile");
		embed_template!("password");
		embed_template!("server_versions");
		embed_template!("versions");

		Ok(Arc::new(tera))
	}

	/// Binary entry point. Async because the AWS/kube clients are built from
	/// async provider/cluster discovery. Builds the STS + kube clients from the
	/// pod's IRSA / in-cluster environment; a missing or broken AWS/kube
	/// environment degrades to `None` (the backup endpoints then 502) rather
	/// than failing startup.
	pub async fn init() -> Result<Self> {
		let mut state = Self::from_db(database::init())?;
		state.sts = Some(Self::init_sts().await);
		state.kube = Self::init_kube().await;
		state.artifacts = commons_servers::artifact_store::ArtifactStore::try_default().await;
		Ok(state)
	}

	/// Build the STS client from the default credential/region provider chain
	/// (in-cluster: the pod's IRSA web-identity).
	async fn init_sts() -> aws_sdk_sts::Client {
		let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		aws_sdk_sts::Client::new(&aws)
	}

	/// Build the kube-backed secret reader. Returns `None` (logged) when no
	/// cluster config is available, so `/backup-target` 502s until fixed
	/// rather than the binary failing to start.
	async fn init_kube() -> Option<BackupSecrets> {
		BackupSecrets::try_default().await
	}

	/// Sync constructor with `None` AWS/kube clients — used by the private
	/// server's nested `/public/...` mount, the test harness, and any
	/// non-AWS Canopy instance.
	pub fn from_db(db: Db) -> Result<Self> {
		Self::from_db_with_directory(db, None)
	}

	pub fn from_db_with_directory(
		db: Db,
		tailnet_directory: Option<TailnetDirectory>,
	) -> Result<Self> {
		let db_read = database::init_ro().unwrap_or_else(|| db.clone());
		Ok(Self {
			client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::from_env(),
			db,
			db_read,
			#[cfg(feature = "ui")]
			tera: Self::init_tera()?,
			server_versions_secret: std::env::var("SERVER_VERSIONS_SECRET").ok(),
			tailnet_directory,
			rate_limiter: crate::ratelimit::RateLimiter::default(),
			sts: None,
			kube: None,
			artifacts: None,
			dns_zones: dns_zones_from_env(),
		})
	}

	/// Like [`from_db_with_directory`](Self::from_db_with_directory) but with the
	/// backup-credential clients wired in. private-server's nested `/public`
	/// mount uses this so device callers reaching it can issue backup credentials
	/// (STS `AssumeRole`) and fetch the repo target/password (kube Secret store)
	/// exactly like the standalone public server — the bare `from_db*` base leaves
	/// both `None`, which 502s the whole backup API.
	pub fn for_nested_mount(
		db: Db,
		tailnet_directory: Option<TailnetDirectory>,
		sts: Option<aws_sdk_sts::Client>,
		kube: Option<BackupSecrets>,
		artifacts: Option<commons_servers::artifact_store::ArtifactStore>,
	) -> Result<Self> {
		Ok(Self {
			client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::from_env(),
			sts,
			kube,
			artifacts,
			..Self::from_db_with_directory(db, tailnet_directory)?
		})
	}
}

impl FromRef<AppState> for crate::ratelimit::RateLimiter {
	fn from_ref(state: &AppState) -> Self {
		state.rate_limiter.clone()
	}
}

impl FromRef<AppState> for Db {
	fn from_ref(state: &AppState) -> Self {
		state.db.clone()
	}
}

/// So a handler can take just the zone list, the way it takes just the pool.
// spec: CRT
impl FromRef<AppState> for Vec<commons_types::dns::ManagedZone> {
	fn from_ref(state: &AppState) -> Self {
		state.dns_zones.clone()
	}
}

impl FromRef<AppState> for Option<TailnetDirectory> {
	fn from_ref(state: &AppState) -> Self {
		state.tailnet_directory.clone()
	}
}

impl FromRef<AppState> for Option<aws_sdk_sts::Client> {
	fn from_ref(state: &AppState) -> Self {
		state.sts.clone()
	}
}

impl FromRef<AppState> for Option<BackupSecrets> {
	fn from_ref(state: &AppState) -> Self {
		state.kube.clone()
	}
}

#[cfg(feature = "ui")]
impl FromRef<AppState> for Arc<Tera> {
	fn from_ref(state: &AppState) -> Self {
		state.tera.clone()
	}
}

impl FromRef<AppState> for commons_servers::device_auth::mtls::ClientCertHeader {
	fn from_ref(state: &AppState) -> Self {
		state.client_cert_header
	}
}

#[cfg(all(test, feature = "ui"))]
mod tests {
	/// The templates are embedded, so a syntax error in one is a startup
	/// panic rather than a compile error. Without this, the first thing to
	/// notice is every server-backed integration test failing at once.
	#[test]
	fn every_embedded_template_parses() {
		super::AppState::init_tera().unwrap();
	}
}
