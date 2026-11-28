//! Canopy as an OpenID Connect issuer: its configuration, its signing keys, and
//! the documents and tokens built from them.
//!
//! Specs: `.workhorse/specs/private-server/identity-provider.md` (id `IDP`) and
//! `.workhorse/specs/public-server/identity-tokens.md` (id `IDT`), and the
//! sign-in tokens of `.workhorse/specs/private-server/tamanu-sign-in.md` (id
//! `TSI`).
//!
//! Nothing here touches Canopy's database, so the discovery component, which
//! holds no access to it, is built from this crate alone.

mod config;
mod der;
mod discovery;
mod issuer;
pub mod jwe;
mod jwk;
mod jwt;
mod keyring;
mod sign_in;

pub use config::{ISSUER_ENV, IdentityConfig, KEYS_ENV};
pub use discovery::{CLAIMS, DISCOVERY_PATH, JWKS_PATH, discovery_document, jwks_uri};
pub use issuer::{IdentityProvider, Issuer};
pub use jwk::{Jwks, PublicJwk};
pub use jwt::{IDENTITY_TOKEN_TYPE, SIGN_IN_TOKEN_TYPE, encode};
pub use keyring::{Keyring, SigningKey};
pub use sign_in::{SIGN_IN_LIFETIME, SignInClaims};

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
	#[error("encryption failed: {0}")]
	Encryption(&'static str),
	#[error("cannot decrypt: {0}")]
	Decryption(&'static str),
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
