//! Protocol version negotiation, carried in the QUIC ALPN token.
//!
//! Riding on ALPN means an incompatible pair fails at the TLS handshake with
//! "no application protocol" rather than connecting and then failing to parse
//! a message. Both ends advertise the range they support, newest first, and
//! the handshake settles on one — after which each end knows the version from
//! the connection itself and never has to ask.

/// The first protocol version.
pub const ALPN_V1: &[u8] = b"canopy-relay/1";

/// Every version this build speaks, newest first. Advertised by both ends:
/// canopy accepts any relay in this set, and a relay offers every version it
/// can speak so an older canopy can still settle on one.
pub const SUPPORTED_ALPN: &[&[u8]] = &[ALPN_V1];

/// A settled protocol version.
///
/// Deliberately an enum rather than a number: a version this build does not
/// know is not representable, so a negotiated token that fell outside
/// [`SUPPORTED_ALPN`] cannot be carried around as if it were understood.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProtocolVersion {
	V1,
}

/// A negotiated ALPN token that is not a version this build speaks. Reaching
/// this means the TLS layer settled on something outside [`SUPPORTED_ALPN`],
/// which it should not be able to do — so it is an error rather than a
/// tolerated unknown.
#[derive(Debug, Clone, thiserror::Error)]
#[error("negotiated application protocol {0:?} is not a canopy-relay version")]
pub struct UnknownProtocol(pub String);

impl ProtocolVersion {
	/// The version a negotiated ALPN token names.
	pub fn from_alpn(token: &[u8]) -> Result<Self, UnknownProtocol> {
		match token {
			ALPN_V1 => Ok(Self::V1),
			other => Err(UnknownProtocol(String::from_utf8_lossy(other).into_owned())),
		}
	}

	/// The ALPN token for this version.
	pub fn alpn(self) -> &'static [u8] {
		match self {
			Self::V1 => ALPN_V1,
		}
	}
}

/// [`SUPPORTED_ALPN`] as rustls wants it.
pub fn alpn_protocols() -> Vec<Vec<u8>> {
	SUPPORTED_ALPN.iter().map(|v| v.to_vec()).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn every_supported_token_round_trips() {
		for token in SUPPORTED_ALPN {
			let version = ProtocolVersion::from_alpn(token).expect("supported");
			assert_eq!(version.alpn(), *token);
		}
	}

	#[test]
	fn an_unknown_token_is_an_error_not_a_version() {
		assert!(ProtocolVersion::from_alpn(b"canopy-relay/99").is_err());
		assert!(ProtocolVersion::from_alpn(b"h3").is_err());
		assert!(ProtocolVersion::from_alpn(b"").is_err());
	}

	/// The advertised order is what the handshake prefers, so a newer version
	/// must come first. With one version this only guards the invariant for
	/// when a second is added.
	#[test]
	fn the_newest_version_is_advertised_first() {
		let versions: Vec<ProtocolVersion> = SUPPORTED_ALPN
			.iter()
			.map(|t| ProtocolVersion::from_alpn(t).expect("supported"))
			.collect();
		let mut sorted = versions.clone();
		sorted.sort_by(|a, b| b.cmp(a));
		assert_eq!(versions, sorted, "SUPPORTED_ALPN must be newest-first");
	}
}
