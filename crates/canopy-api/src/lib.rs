//! Rust client for canopy's public API.
//!
//! The wire types and the per-endpoint methods in [`schema`] are generated from
//! canopy's OpenAPI document, which lives in the same repository as this crate,
//! so they are the types canopy declares rather than a separate description of
//! them. The document and this crate carry the same version.
//!
//! # Calling canopy
//!
//! [`CanopyClient`] has one method per operation, taking and returning the
//! generated types. The method name comes from the path (`/backup-credentials`
//! becomes `backup_credentials`), with the verb prefixed where a path is served
//! by more than one verb.
//!
//! How a request reaches canopy is the consumer's to decide: this crate depends
//! on no HTTP client, and a consumer supplies a [`CanopyTransport`] which
//! resolves the host, the scheme, and the authentication itself. Any status
//! outside the success range surfaces as [`CanopyHttpError`], since endpoints
//! give particular statuses a meaning only the caller can read.
//!
//! # What the generated types carry
//!
//! Timestamp fields are [`jiff::Timestamp`] rather than text, and credential
//! secrets are wrapped in [`Redacted`] so they stay out of `Debug` output and
//! logs; read them through the inner value. Schemas that carry arbitrary further
//! keys alongside their declared fields generate a map field holding the rest, so
//! those keys can be both sent and read.
//!
//! Generated structs are `#[non_exhaustive]` and carry a builder, so a schema
//! gaining a field leaves construction working for a consumer that does not set
//! it.

use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, Serializer};

mod client;
mod error;
mod transport;

/// Wire types and per-endpoint methods generated from canopy's OpenAPI document.
///
/// Regenerate with `just gen-api` after changing the public API; the generated
/// source is committed, so a change to this surface appears in the change that
/// causes it.
pub mod schema {
	include!("generated.rs");
}

/// Check the values a generated method is about to place in its path.
///
/// A path value is the caller's, and it decides which endpoint is called. A
/// value carrying `?`, `#` or `/` would reroute the request, put parameters of
/// its own ahead of the ones the caller asked for, or cut the query off. A value
/// that is a dot segment does the same thing without carrying a delimiter at
/// all: `..` placed in `/versions/{version}/artifacts` puts `/versions/../artifacts`
/// on the wire, which resolves to `/artifacts`. A `%` is refused with them,
/// because a delimiter spelled as an escape reaches a server that decodes before
/// it resolves, and an empty value collapses a segment away.
///
/// Such a value is refused rather than encoded, because encoding it would change
/// what every call already in the field puts on the wire.
pub(crate) fn segments(path: &str, values: &[(&str, &str)]) -> Result<()> {
	for (name, value) in values {
		if value.is_empty()
			|| matches!(*value, "." | "..")
			|| value.contains(['?', '#', '/', '\\', '%'])
		{
			return Err(Error::PathValue {
				path: path.to_owned(),
				name: (*name).to_owned(),
				value: (*value).to_owned(),
			});
		}
	}
	Ok(())
}

/// Append the query parameters a generated method was given to `path`.
///
/// A parameter given as `None` is left off entirely rather than sent empty,
/// because canopy tells an absent parameter from an empty one. A method whose
/// envelope sets none of its parameters therefore sends the bare path, exactly
/// as it did before the envelope existed.
pub(crate) fn query(path: &str, params: &[(&str, Option<String>)]) -> String {
	let mut out = path.to_owned();
	let mut first = true;
	for (name, value) in params {
		let Some(value) = value else { continue };
		out.push(if first { '?' } else { '&' });
		first = false;
		out.push_str(name);
		out.push('=');
		percent_encode(value, &mut out);
	}
	out
}

/// Percent-encode a query value, keeping RFC 3986's unreserved set as it is.
///
/// The value is whatever the caller passed, so it is encoded rather than
/// trusted: a parameter carrying an `&` would otherwise read as the start of
/// another one.
fn percent_encode(value: &str, out: &mut String) {
	for byte in value.bytes() {
		if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
			out.push(byte as char);
		} else {
			out.push_str(&format!("%{byte:02X}"));
		}
	}
}

pub use async_trait::async_trait;
pub use client::CanopyClient;
pub use error::{CanopyHttpError, Error, Result};
pub use transport::{CanopyRequest, CanopyResponse, CanopyTransport};
pub use {bytes, http};

/// Wraps a sensitive value so its `Debug` output doesn't leak the contents.
///
/// Serialises and deserialises as the inner value, so it is transparent on the
/// wire; only `Debug` is withheld. Read the value through [`Deref`](std::ops::Deref)
/// or the public field.
#[derive(Clone)]
pub struct Redacted<T>(pub T);

impl<T> fmt::Debug for Redacted<T> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str("<redacted>")
	}
}

impl<T> std::ops::Deref for Redacted<T> {
	type Target = T;
	fn deref(&self) -> &T {
		&self.0
	}
}

impl<T> From<T> for Redacted<T> {
	fn from(value: T) -> Self {
		Self(value)
	}
}

impl<T: Serialize> Serialize for Redacted<T> {
	fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
		self.0.serialize(serializer)
	}
}

impl<'de, T: Deserialize<'de>> Deserialize<'de> for Redacted<T> {
	fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
		T::deserialize(deserializer).map(Redacted)
	}
}
