//! Canopy as an OpenID Connect issuer: its configuration, its signing keys, and
//! the documents and tokens built from them.
//!
//! Specs: `.workhorse/specs/private-server/identity-provider.md` (id `IDP`) and
//! `.workhorse/specs/public-server/identity-tokens.md` (id `IDT`).
//!
//! Nothing here touches Canopy's database, so the discovery component, which
//! holds no access to it, is built from this crate alone.

mod config;
mod der;
mod discovery;
mod jwk;
mod jwt;
mod keyring;

pub use config::{ISSUER_ENV, IdentityConfig, KEYS_ENV};
pub use discovery::{DISCOVERY_PATH, JWKS_PATH, discovery_document, jwks_uri};
pub use jwk::{Jwks, PublicJwk};
pub use jwt::encode;
pub use keyring::{Keyring, SigningKey};

/// What can go wrong building or signing with the identity provider's keys.
#[derive(Debug, thiserror::Error)]
pub enum Error {
	#[error("identity provider configuration: {0}")]
	Config(String),
	#[error("key service: {0}")]
	KeyService(String),
	#[error("key {0} is not a P-256 public key")]
	NotP256(String),
	#[error("malformed signature from the key service")]
	MalformedSignature,
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
