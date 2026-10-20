use std::sync::Arc;

use aws_sdk_kms::{
	primitives::Blob,
	types::{KeySpec, MessageType, SigningAlgorithmSpec},
};
use ring::{
	digest::{SHA256, digest},
	rand::SystemRandom,
	signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair, KeyPair},
};
use tokio::sync::OnceCell;

use crate::{Error, PublicJwk, Result, der::ecdsa_der_to_jose};

/// A key Canopy signs identity tokens with.
#[derive(Clone, Debug)]
pub struct SigningKey {
	/// The key's reference in the configuration.
	pub key_ref: String,
	/// Its public half, which carries its `kid`.
	pub jwk: PublicJwk,
	inner: Inner,
}

#[derive(Clone)]
enum Inner {
	/// Held by the key service, which signs and never releases the private key.
	// spec: IDP#signing-keys
	Kms(aws_sdk_kms::Client),
	/// Held in this process. For tests and local runs only: no configuration
	/// reaches it.
	Local(Arc<EcdsaKeyPair>),
}

impl std::fmt::Debug for Inner {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		match self {
			Self::Kms(_) => f.write_str("Kms"),
			Self::Local(_) => f.write_str("Local"),
		}
	}
}

impl SigningKey {
	/// A fresh in-process key.
	pub fn generate_local(key_ref: impl Into<String>) -> Self {
		let rng = SystemRandom::new();
		let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &rng)
			.expect("generating a P-256 key");
		let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, pkcs8.as_ref(), &rng)
			.expect("reading back a generated P-256 key");
		let jwk = PublicJwk::from_point(pair.public_key().as_ref())
			.expect("a generated P-256 key is a P-256 point");
		Self {
			key_ref: key_ref.into(),
			jwk,
			inner: Inner::Local(Arc::new(pair)),
		}
	}

	/// The key's identifier, as its tokens' headers and the key set name it.
	pub fn kid(&self) -> &str {
		&self.jwk.kid
	}

	/// Sign `message` with ES256, returning the signature in its JOSE form.
	pub async fn sign(&self, message: &[u8]) -> Result<[u8; 64]> {
		match &self.inner {
			Inner::Local(pair) => {
				let signature = pair
					.sign(&SystemRandom::new(), message)
					.map_err(|_| Error::KeyService("in-process signing failed".into()))?;
				signature
					.as_ref()
					.try_into()
					.map_err(|_| Error::MalformedSignature)
			}
			Inner::Kms(client) => {
				let signed = client
					.sign()
					.key_id(&self.key_ref)
					.message(Blob::new(digest(&SHA256, message).as_ref()))
					.message_type(MessageType::Digest)
					.signing_algorithm(SigningAlgorithmSpec::EcdsaSha256)
					.send()
					.await
					.map_err(|err| Error::KeyService(format!("{}", err.into_service_error())))?;
				let der = signed.signature.ok_or(Error::MalformedSignature)?;
				ecdsa_der_to_jose(der.as_ref())
			}
		}
	}
}

/// The configured signing keys, oldest first, each resolved to its public half
/// on first use and kept.
///
/// A key whose public half cannot be read yet, because the key service is
/// unreachable, is retried the next time the keys are asked for, so a key
/// service that was down at startup does not leave the keyring empty for good.
#[derive(Clone, Debug, Default)]
pub struct Keyring {
	entries: Arc<Vec<Entry>>,
}

#[derive(Debug)]
struct Entry {
	key_ref: String,
	kms: Option<aws_sdk_kms::Client>,
	resolved: OnceCell<SigningKey>,
}

impl Keyring {
	/// Keys held by the key service, by their references, oldest first.
	pub fn kms(client: aws_sdk_kms::Client, refs: &[String]) -> Self {
		Self {
			entries: Arc::new(
				refs.iter()
					.map(|key_ref| Entry {
						key_ref: key_ref.clone(),
						kms: Some(client.clone()),
						resolved: OnceCell::new(),
					})
					.collect(),
			),
		}
	}

	/// Keys already in hand, oldest first.
	pub fn local(keys: Vec<SigningKey>) -> Self {
		Self {
			entries: Arc::new(
				keys.into_iter()
					.map(|key| Entry {
						key_ref: key.key_ref.clone(),
						kms: None,
						resolved: OnceCell::new_with(Some(key)),
					})
					.collect(),
			),
		}
	}

	/// The configured references, oldest first.
	pub fn refs(&self) -> impl Iterator<Item = &str> {
		self.entries.iter().map(|entry| entry.key_ref.as_str())
	}

	pub fn is_empty(&self) -> bool {
		self.entries.is_empty()
	}

	/// Every configured key, oldest first, with `None` for one whose public half
	/// could not be read.
	pub async fn keys(&self) -> Vec<(String, Option<SigningKey>)> {
		let mut out = Vec::with_capacity(self.entries.len());
		for entry in self.entries.iter() {
			let resolved = entry
				.resolved
				.get_or_try_init(|| resolve(entry))
				.await
				.inspect_err(|err| {
					tracing::warn!(key = %entry.key_ref, %err, "identity signing key unavailable");
				})
				.ok()
				.cloned();
			out.push((entry.key_ref.clone(), resolved));
		}
		out
	}
}

async fn resolve(entry: &Entry) -> Result<SigningKey> {
	let Some(client) = &entry.kms else {
		return Err(Error::KeyService("no key service for this key".into()));
	};
	let public = client
		.get_public_key()
		.key_id(&entry.key_ref)
		.send()
		.await
		.map_err(|err| Error::KeyService(format!("{}", err.into_service_error())))?;
	if public.key_spec() != Some(&KeySpec::EccNistP256) {
		return Err(Error::NotP256(entry.key_ref.clone()));
	}
	let spki = public.public_key().ok_or_else(|| {
		Error::KeyService(format!("no public key returned for {}", entry.key_ref))
	})?;
	Ok(SigningKey {
		key_ref: entry.key_ref.clone(),
		jwk: PublicJwk::from_spki_der(spki.as_ref(), &entry.key_ref)?,
		inner: Inner::Kms(client.clone()),
	})
}

impl SigningKey {
	/// An in-process key from its PKCS#8 encoding, for tests that need to know
	/// the key they will be issued tokens by.
	pub fn local_from_pkcs8(key_ref: impl Into<String>, pkcs8: &[u8]) -> Result<Self> {
		let pair = EcdsaKeyPair::from_pkcs8(
			&ECDSA_P256_SHA256_FIXED_SIGNING,
			pkcs8,
			&SystemRandom::new(),
		)
		.map_err(|err| Error::Config(format!("not a P-256 PKCS#8 key: {err}")))?;
		let jwk = PublicJwk::from_point(pair.public_key().as_ref())?;
		Ok(Self {
			key_ref: key_ref.into(),
			jwk,
			inner: Inner::Local(Arc::new(pair)),
		})
	}
}
