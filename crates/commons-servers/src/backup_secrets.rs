//! Shared reader/writer for the per-group repo-password k8s Secrets.
//!
//! Canopy owns every repo passphrase Secret. This narrow store exposes exactly
//! what the components need: read one key (public-server `/backup-target`,
//! jobs maintenance), create one Secret (private-server onboarding),
//! create-or-replace one (the jobs rotation loop), and delete one (private-server
//! config decommission). Real Canopy instances use the
//! `Kube` variant; tests and the e2e binary use the in-memory `Memory` store
//! (gated by `CANOPY_BACKUP_SECRETS_MEMORY`) so these paths are exercised
//! without a cluster.

use std::{
	collections::BTreeMap,
	sync::{Arc, Mutex},
};

use commons_errors::{AppError, Result};

/// Env var that forces the in-memory secret store (no cluster needed). Set by the
/// e2e fixture so onboarding works against the real binary; tests use
/// [`BackupSecrets::memory`] directly.
const MEMORY_ENV: &str = "CANOPY_BACKUP_SECRETS_MEMORY";

type MemoryStore = Arc<Mutex<BTreeMap<String, BTreeMap<String, String>>>>;

/// Reader/writer for the per-group repo-password Secrets. `Kube` is a namespaced
/// [`kube::Client`]; `Memory` is an in-process map for tests + the e2e binary.
#[derive(Clone)]
pub enum BackupSecrets {
	Kube {
		client: kube::Client,
		namespace: String,
	},
	Memory(MemoryStore),
}

impl std::fmt::Debug for BackupSecrets {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Kube { namespace, .. } => f
				.debug_struct("BackupSecrets::Kube")
				.field("namespace", namespace)
				.finish_non_exhaustive(),
			Self::Memory(_) => f.write_str("BackupSecrets::Memory"),
		}
	}
}

impl BackupSecrets {
	pub fn new(client: kube::Client, namespace: String) -> Self {
		Self::Kube { client, namespace }
	}

	/// An in-memory store for tests / the e2e binary. **Debug-only**: the
	/// constructor — and therefore any way to reach the `Memory` variant — does
	/// not exist in release builds, so production secrets can never be backed by a
	/// non-persistent, process-local map, no matter the environment.
	#[cfg(debug_assertions)]
	pub fn memory() -> Self {
		Self::Memory(Arc::new(Mutex::new(BTreeMap::new())))
	}

	/// Build a secret store from the ambient cluster config (in-cluster the pod's
	/// service account; locally `~/.kube/config`), reading from the pod's own
	/// namespace (see [`pod_namespace`]). If `MEMORY_ENV` is set, returns the
	/// in-memory store instead. Returns `None` (logged) when no cluster config is
	/// available, so callers degrade to 502 rather than failing startup.
	pub async fn try_default() -> Option<Self> {
		if std::env::var_os(MEMORY_ENV).is_some() {
			// The in-memory store is debug-only (tests, e2e, local dev). `memory()`
			// doesn't exist in release builds (attribute-`cfg`, not a runtime check),
			// so it can't be swapped in by accident in production — where backing
			// repo passphrases with a non-persistent, process-local map would be
			// catastrophic.
			#[cfg(debug_assertions)]
			{
				tracing::warn!("{MEMORY_ENV} set; using in-memory backup Secret store");
				return Some(Self::memory());
			}
			#[cfg(not(debug_assertions))]
			tracing::error!(
				"{MEMORY_ENV} is set but IGNORED: the in-memory Secret store is debug-only and is never used in release builds"
			);
		}
		match kube::Client::try_default().await {
			Ok(client) => {
				let namespace = pod_namespace(&client);
				Some(Self::Kube { client, namespace })
			}
			Err(err) => {
				tracing::warn!(error = ?err, "kube client unavailable; backup Secret ops will 502");
				None
			}
		}
	}

	/// Read one key out of the named Secret. Maps every failure (missing Secret,
	/// missing key, non-utf8, API error) to [`AppError::Upstream`] so handlers
	/// return 502 with a generic body.
	pub async fn read_password(&self, secret_name: &str, key: &str) -> Result<String> {
		match self {
			Self::Kube { client, namespace } => {
				use k8s_openapi::api::core::v1::Secret;
				use kube::Api;

				let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
				let secret = api
					.get(secret_name)
					.await
					.map_err(|e| AppError::Upstream(format!("secret get failed: {e}")))?;
				let data = secret
					.data
					.ok_or_else(|| AppError::Upstream("secret has no data".into()))?;
				let bytes = data
					.get(key)
					.ok_or_else(|| AppError::Upstream(format!("secret has no key {key}")))?;
				String::from_utf8(bytes.0.clone())
					.map_err(|_| AppError::Upstream("secret value is not valid utf-8".into()))
			}
			Self::Memory(store) => store
				.lock()
				.unwrap()
				.get(secret_name)
				.and_then(|m| m.get(key))
				.cloned()
				.ok_or_else(|| AppError::Upstream(format!("secret get failed: {secret_name}"))),
		}
	}

	/// Create the named Secret holding `value` under `key` (fails if it already
	/// exists). Used by the private-server onboarding path.
	pub async fn create_password(&self, secret_name: &str, key: &str, value: &str) -> Result<()> {
		match self {
			Self::Kube { client, namespace } => {
				use k8s_openapi::api::core::v1::Secret;
				use kube::{Api, api::PostParams};

				let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
				api.create(
					&PostParams::default(),
					&secret_object(secret_name, key, value),
				)
				.await
				.map_err(|e| AppError::Upstream(format!("secret create failed: {e}")))?;
				Ok(())
			}
			Self::Memory(store) => {
				use std::collections::btree_map::Entry;

				match store.lock().unwrap().entry(secret_name.to_string()) {
					// Kube answers 409 here. The double has to as well, or the
					// double-create path passes in tests and 502s in production
					// — and the callers' rollback of a failed create can't be
					// exercised at all.
					Entry::Occupied(_) => Err(AppError::Upstream(format!(
						"secret create failed: {secret_name} already exists"
					))),
					Entry::Vacant(slot) => {
						slot.insert(BTreeMap::from([(key.to_string(), value.to_string())]));
						Ok(())
					}
				}
			}
		}
	}

	/// Delete the named Secret — used when a config is decommissioned (the
	/// Canopy-owned passphrase should not outlive its config). Idempotent: an
	/// already-absent Secret is a success.
	pub async fn delete_password(&self, secret_name: &str) -> Result<()> {
		match self {
			Self::Kube { client, namespace } => {
				use k8s_openapi::api::core::v1::Secret;
				use kube::{Api, api::DeleteParams};

				let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
				match api.delete(secret_name, &DeleteParams::default()).await {
					Ok(_) => Ok(()),
					// Already gone — decommission is idempotent.
					Err(kube::Error::Api(e)) if e.code == 404 => Ok(()),
					Err(e) => Err(AppError::Upstream(format!("secret delete failed: {e}"))),
				}
			}
			Self::Memory(store) => {
				store.lock().unwrap().remove(secret_name);
				Ok(())
			}
		}
	}

	/// Read all string keys of the named Secret — for the rotation dual-key state
	/// machine (`password` + `password_next`). Missing Secret → Err; absent keys
	/// are simply not in the map.
	pub async fn read_keys(&self, secret_name: &str) -> Result<BTreeMap<String, String>> {
		match self {
			Self::Kube { client, namespace } => {
				use k8s_openapi::api::core::v1::Secret;
				use kube::Api;

				let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
				let secret = api
					.get(secret_name)
					.await
					.map_err(|e| AppError::Upstream(format!("secret get failed: {e}")))?;
				let mut out = BTreeMap::new();
				for (k, v) in secret.data.unwrap_or_default() {
					let s = String::from_utf8(v.0).map_err(|_| {
						AppError::Upstream(format!("secret {secret_name} key {k} not utf-8"))
					})?;
					out.insert(k, s);
				}
				Ok(out)
			}
			Self::Memory(store) => store
				.lock()
				.unwrap()
				.get(secret_name)
				.cloned()
				.ok_or_else(|| AppError::Upstream(format!("secret get failed: {secret_name}"))),
		}
	}

	/// Read all string keys, answering `None` for a Secret that does not exist.
	/// A caller that reads-modifies-writes needs an absent Secret told apart
	/// from an API failure: treating a failure as empty would have the write
	/// back drop every key already there.
	pub async fn try_read_keys(
		&self,
		secret_name: &str,
	) -> Result<Option<BTreeMap<String, String>>> {
		match self {
			Self::Kube { .. } => match self.read_keys(secret_name).await {
				Ok(keys) => Ok(Some(keys)),
				Err(_) if !self.exists(secret_name).await? => Ok(None),
				Err(err) => Err(err),
			},
			Self::Memory(store) => Ok(store.lock().unwrap().get(secret_name).cloned()),
		}
	}

	/// Whether the named Secret exists.
	async fn exists(&self, secret_name: &str) -> Result<bool> {
		match self {
			Self::Kube { client, namespace } => {
				use k8s_openapi::api::core::v1::Secret;
				use kube::Api;

				let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
				match api.get_opt(secret_name).await {
					Ok(found) => Ok(found.is_some()),
					Err(e) => Err(AppError::Upstream(format!("secret get failed: {e}"))),
				}
			}
			Self::Memory(store) => Ok(store.lock().unwrap().contains_key(secret_name)),
		}
	}

	/// Create-or-replace the named Secret to hold **exactly** `keys` (server-side
	/// apply with force). Keys this manager owns but that are omitted from `keys`
	/// are removed — so a rotation "promote" that writes only `{password}` cleans
	/// up the leftover `password_next`. Used by the rotation dual-key dance.
	pub async fn put_keys(&self, secret_name: &str, keys: &BTreeMap<String, String>) -> Result<()> {
		match self {
			Self::Kube { client, namespace } => {
				use k8s_openapi::api::core::v1::Secret;
				use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
				use kube::{
					Api,
					api::{Patch, PatchParams},
				};

				let api: Api<Secret> = Api::namespaced(client.clone(), namespace);
				let secret = Secret {
					metadata: ObjectMeta {
						name: Some(secret_name.to_string()),
						..Default::default()
					},
					string_data: Some(keys.clone()),
					..Default::default()
				};
				api.patch(
					secret_name,
					&PatchParams::apply("canopy-backups").force(),
					&Patch::Apply(secret),
				)
				.await
				.map_err(|e| AppError::Upstream(format!("secret apply failed: {e}")))?;
				Ok(())
			}
			Self::Memory(store) => {
				store
					.lock()
					.unwrap()
					.insert(secret_name.to_string(), keys.clone());
				Ok(())
			}
		}
	}
}

/// Build a `Secret` carrying `value` under `key` named `secret_name`.
fn secret_object(secret_name: &str, key: &str, value: &str) -> k8s_openapi::api::core::v1::Secret {
	use k8s_openapi::api::core::v1::Secret;
	use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

	Secret {
		metadata: ObjectMeta {
			name: Some(secret_name.to_string()),
			..Default::default()
		},
		string_data: Some(BTreeMap::from([(key.to_string(), value.to_string())])),
		..Default::default()
	}
}

/// Namespace the repo-password Secrets live in. `POD_NAMESPACE` (injected via the
/// downward API) wins if set; otherwise the pod's **own** namespace from the
/// in-cluster config — i.e. the ServiceAccount's namespace. Never a hardcoded
/// guess: defaulting to `canopy` meant a pod deployed elsewhere (e.g.
/// `tamanu-meta-prod`) read/created Secrets in `canopy`, where its SA has no RBAC
/// (403 Forbidden), so onboarding's Secret and the backups pod's read landed in
/// different namespaces than the grants.
fn pod_namespace(client: &kube::Client) -> String {
	std::env::var("POD_NAMESPACE").unwrap_or_else(|_| client.default_namespace().to_string())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[tokio::test]
	async fn memory_create_read_delete_roundtrip() {
		let secrets = BackupSecrets::memory();
		secrets
			.create_password("backup-repo-x", "password", "hunter2")
			.await
			.unwrap();
		assert_eq!(
			secrets
				.read_password("backup-repo-x", "password")
				.await
				.unwrap(),
			"hunter2"
		);

		// Decommission removes the Secret entirely.
		secrets.delete_password("backup-repo-x").await.unwrap();
		assert!(
			secrets
				.read_password("backup-repo-x", "password")
				.await
				.is_err()
		);

		// Deleting an already-absent Secret is a no-op success.
		secrets.delete_password("backup-repo-x").await.unwrap();
	}

	/// `create_password` is create-if-absent (Kube answers 409). The double
	/// has to reject too — otherwise a double-create passes in tests and 502s
	/// in production, and no test can reach a caller's rollback path.
	#[tokio::test]
	async fn memory_create_password_rejects_an_existing_secret() {
		let secrets = BackupSecrets::memory();
		secrets
			.create_password("backup-repo-y", "password", "first")
			.await
			.unwrap();
		assert!(
			secrets
				.create_password("backup-repo-y", "password", "second")
				.await
				.is_err(),
			"creating over an existing secret must fail",
		);
		assert_eq!(
			secrets
				.read_password("backup-repo-y", "password")
				.await
				.unwrap(),
			"first",
			"the rejected create must not have overwritten anything",
		);
	}
}

/// Generate a strong repo passphrase: 8 words from the EFF large wordlist
/// (~103 bits), hyphen-separated. Canopy owns it (no human copy) and rotates it
/// regularly; used by onboarding (from-birth) and the rotation loop.
pub fn generate_passphrase() -> String {
	use chbs::{config::BasicConfig, prelude::*, probability::Probability, word::WordList};

	let config = BasicConfig {
		words: 8,
		word_provider: WordList::builtin_eff_large().sampler(),
		separator: "-".into(),
		capitalize_first: Probability::Never,
		capitalize_words: Probability::Never,
	};
	config.to_scheme().generate()
}
