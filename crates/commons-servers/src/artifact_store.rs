//! Where Canopy keeps the bytes of the artifacts it holds.
//!
//! A group-scoped artifact is carried to Canopy by the registration that
//! publishes it, and Canopy keeps it in storage of its own, apart from any
//! group's backup repo. Objects are addressed by the artifact's id, so a
//! re-registration replaces the one object and a deregistration removes it.
//!
//! No caller addresses the store: the boundary is enforced on the read, which
//! resolves the artifact against the caller's scope first and only then asks
//! for its bytes.
//!
//! `S3` is the real store; `Memory` is an in-process map for tests and the e2e
//! binary, mirroring [`crate::backup_secrets::BackupSecrets`].
// spec: ART#where-an-artifact-rests

use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

use commons_errors::{AppError, Result};
use uuid::Uuid;

/// Bucket the artifacts Canopy holds are kept in. Unset ⇒ no store is
/// configured, and registering or serving held bytes reports that rather than
/// the binary failing to start.
pub const BUCKET_ENV: &str = "CANOPY_ARTIFACT_BUCKET";
/// Region the bucket is in, where it is not the ambient one.
pub const REGION_ENV: &str = "CANOPY_ARTIFACT_REGION";
/// Role to assume for the object calls, where the bucket lives in an account
/// the pod's own identity does not reach.
pub const ROLE_ARN_ENV: &str = "CANOPY_ARTIFACT_ROLE_ARN";
/// Key prefix within the bucket, so artifacts can share a bucket with
/// something else. Defaults to [`DEFAULT_PREFIX`].
pub const PREFIX_ENV: &str = "CANOPY_ARTIFACT_PREFIX";
/// Env var that forces the in-memory store (no bucket needed). Set by the e2e
/// fixture; tests use [`ArtifactStore::memory`] directly.
const MEMORY_ENV: &str = "CANOPY_ARTIFACT_STORE_MEMORY";

pub const DEFAULT_PREFIX: &str = "artifacts/";

type MemoryStore = Arc<Mutex<BTreeMap<String, Vec<u8>>>>;

/// The bytes Canopy holds for one artifact, and how to store, read and drop
/// them.
#[derive(Clone)]
pub enum ArtifactStore {
	S3 {
		client: aws_sdk_s3::Client,
		bucket: String,
		prefix: String,
	},
	Memory(MemoryStore),
}

impl std::fmt::Debug for ArtifactStore {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::S3 { bucket, prefix, .. } => f
				.debug_struct("ArtifactStore::S3")
				.field("bucket", bucket)
				.field("prefix", prefix)
				.finish_non_exhaustive(),
			Self::Memory(_) => f.write_str("ArtifactStore::Memory"),
		}
	}
}

impl ArtifactStore {
	/// An in-process store for tests / the e2e binary. **Debug-only**: the
	/// constructor — and therefore any way to reach the `Memory` variant — does
	/// not exist in release builds, so a real instance can never keep artifacts
	/// in a process-local map that vanishes with the pod.
	#[cfg(debug_assertions)]
	pub fn memory() -> Self {
		Self::Memory(Arc::new(Mutex::new(BTreeMap::new())))
	}

	/// Build the store from the environment. Returns `None` (logged) when no
	/// bucket is configured, so the endpoints that need one report it rather
	/// than the binary failing to start.
	pub async fn try_default() -> Option<Self> {
		if std::env::var_os(MEMORY_ENV).is_some() {
			#[cfg(debug_assertions)]
			{
				tracing::warn!("{MEMORY_ENV} set; holding artifacts in process memory");
				return Some(Self::memory());
			}
			#[cfg(not(debug_assertions))]
			tracing::error!(
				"{MEMORY_ENV} is set but IGNORED: the in-memory artifact store is debug-only"
			);
		}

		let Ok(bucket) = std::env::var(BUCKET_ENV) else {
			tracing::warn!("{BUCKET_ENV} is unset; Canopy can hold no artifact");
			return None;
		};
		let prefix = std::env::var(PREFIX_ENV).unwrap_or_else(|_| DEFAULT_PREFIX.to_owned());

		let sdk = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		let mut builder = aws_sdk_s3::config::Builder::from(&sdk);
		if let Ok(role_arn) = std::env::var(ROLE_ARN_ENV) {
			match assumed_credentials(&sdk, &role_arn).await {
				Ok(creds) => builder = builder.credentials_provider(creds),
				Err(err) => {
					tracing::error!("cannot assume {role_arn} for the artifact store: {err}");
					return None;
				}
			}
		}
		if let Ok(region) = std::env::var(REGION_ENV) {
			builder = builder.region(aws_sdk_s3::config::Region::new(region));
		}

		Some(Self::S3 {
			client: aws_sdk_s3::Client::from_conf(builder.build()),
			bucket,
			prefix,
		})
	}

	fn key(&self, artifact: Uuid) -> String {
		match self {
			Self::S3 { prefix, .. } => format!("{prefix}{artifact}"),
			Self::Memory(_) => artifact.to_string(),
		}
	}

	/// Store an artifact's bytes, replacing whatever was under its id.
	pub async fn put(&self, artifact: Uuid, bytes: Vec<u8>) -> Result<()> {
		let key = self.key(artifact);
		match self {
			Self::S3 { client, bucket, .. } => {
				client
					.put_object()
					.bucket(bucket)
					.key(&key)
					.body(bytes.into())
					.send()
					.await
					.map_err(|err| {
						AppError::custom(format!("storing the artifact failed: {err}"))
					})?;
			}
			Self::Memory(store) => {
				store.lock().expect("artifact store").insert(key, bytes);
			}
		}
		Ok(())
	}

	/// An artifact's bytes, or `None` where the store holds none under that id.
	pub async fn get(&self, artifact: Uuid) -> Result<Option<Vec<u8>>> {
		let key = self.key(artifact);
		match self {
			Self::S3 { client, bucket, .. } => {
				let object = match client.get_object().bucket(bucket).key(&key).send().await {
					Ok(object) => object,
					Err(err) if is_missing(&err) => return Ok(None),
					Err(err) => {
						return Err(AppError::custom(format!(
							"reading the artifact failed: {err}"
						)));
					}
				};
				let bytes = object.body.collect().await.map_err(|err| {
					AppError::custom(format!("reading the artifact failed: {err}"))
				})?;
				Ok(Some(bytes.to_vec()))
			}
			Self::Memory(store) => Ok(store.lock().expect("artifact store").get(&key).cloned()),
		}
	}

	/// Drop an artifact's bytes. Deleting what is not there is not an error:
	/// Canopy keeps none of what it has stopped serving either way.
	pub async fn delete(&self, artifact: Uuid) -> Result<()> {
		let key = self.key(artifact);
		match self {
			Self::S3 { client, bucket, .. } => {
				client
					.delete_object()
					.bucket(bucket)
					.key(&key)
					.send()
					.await
					.map_err(|err| {
						AppError::custom(format!("dropping the artifact failed: {err}"))
					})?;
			}
			Self::Memory(store) => {
				store.lock().expect("artifact store").remove(&key);
			}
		}
		Ok(())
	}
}

/// What an endpoint reports when Canopy is asked to hold an artifact and has
/// nowhere to put it.
pub fn unconfigured() -> AppError {
	AppError::custom(format!(
		"Canopy holds no artifacts: {BUCKET_ENV} is not configured"
	))
}

/// Whether a read found nothing there. A store that speaks S3 without modelling
/// `NoSuchKey` answers a plain `NotFound`, so both are an absent object.
fn is_missing<R>(
	err: &aws_sdk_s3::error::SdkError<aws_sdk_s3::operation::get_object::GetObjectError, R>,
) -> bool {
	use aws_sdk_s3::error::ProvideErrorMetadata as _;

	matches!(
		err.as_service_error(),
		Some(aws_sdk_s3::operation::get_object::GetObjectError::NoSuchKey(_))
	) || matches!(err.code(), Some("NoSuchKey" | "NotFound"))
}

async fn assumed_credentials(
	sdk: &aws_config::SdkConfig,
	role_arn: &str,
) -> std::result::Result<aws_sdk_s3::config::Credentials, String> {
	let assumed = aws_sdk_sts::Client::new(sdk)
		.assume_role()
		.role_arn(role_arn)
		.role_session_name("canopy-artifacts")
		.send()
		.await
		.map_err(|err| format!("{err}"))?;
	let creds = assumed
		.credentials()
		.ok_or_else(|| "AssumeRole returned no credentials".to_owned())?;
	Ok(aws_sdk_s3::config::Credentials::new(
		creds.access_key_id(),
		creds.secret_access_key(),
		Some(creds.session_token().to_string()),
		None,
		"canopy-artifacts",
	))
}
