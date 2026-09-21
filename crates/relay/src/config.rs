//! What a relay has to be told.
//!
//! Three things, all of them deployment configuration mounted into the pod:
//! its own device key, canopy's public key, and where canopy is. Nothing here
//! is discovered and nothing is defaulted to something plausible — a relay
//! that guessed any of the three would either fail to authenticate or, worse,
//! authenticate to the wrong peer.

use std::{net::SocketAddr, path::PathBuf};

use relay_protocol::transport::Identity;

use crate::version::{FloorError, VersionFloor};

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
	#[error("reading {path}: {source}")]
	Unreadable {
		path: PathBuf,
		#[source]
		source: std::io::Error,
	},

	#[error("the relay's device key is not usable: {0}")]
	Key(#[from] relay_protocol::TransportError),

	#[error("canopy's pinned key is not valid hex: {0}")]
	Pin(String),

	#[error(transparent)]
	Floor(#[from] FloorError),
}

/// A relay's settled configuration.
pub struct Config {
	/// This relay's identity, from the device key canopy minted for it.
	pub identity: Identity,
	/// Canopy's public key, which this relay verifies on every connection.
	pub canopy_spki: Vec<u8>,
	/// Where canopy listens. Address-agnostic on purpose: the same code dials
	/// an overlay address from a remote cluster and a cluster-DNS address from
	/// canopy's own, which is a configuration difference and not a code path.
	pub canopy_addr: SocketAddr,
	/// The name presented in the TLS handshake. Not verified — the pin is what
	/// identifies canopy — but a name is required by the handshake.
	pub server_name: String,
	/// The lowest version this relay will accept being told to run.
	pub floor: VersionFloor,
}

impl std::fmt::Debug for Config {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Config")
			.field("identity", &self.identity)
			.field(
				"canopy_spki",
				&relay_protocol::transport::hex(&self.canopy_spki),
			)
			.field("canopy_addr", &self.canopy_addr)
			.field("floor", &self.floor)
			.finish()
	}
}

impl Config {
	/// Assemble a configuration from the files and values a deployment
	/// supplies.
	pub fn load(
		key_file: &PathBuf,
		canopy_key_hex: &str,
		canopy_addr: SocketAddr,
		server_name: String,
	) -> Result<Self, ConfigError> {
		let key_pem =
			std::fs::read_to_string(key_file).map_err(|source| ConfigError::Unreadable {
				path: key_file.clone(),
				source,
			})?;

		Ok(Self {
			identity: Identity::from_pkcs8_pem(&key_pem)?,
			canopy_spki: unhex(canopy_key_hex)?,
			canopy_addr,
			server_name,
			floor: VersionFloor::compiled()?,
		})
	}
}

/// Parse the pinned key, which is configured as hex because that is how canopy
/// displays a device key's bytes.
fn unhex(hex: &str) -> Result<Vec<u8>, ConfigError> {
	let cleaned: String = hex.split_whitespace().collect::<String>().to_lowercase();
	if cleaned.is_empty() {
		return Err(ConfigError::Pin("no key was configured".into()));
	}
	if !cleaned.len().is_multiple_of(2) {
		return Err(ConfigError::Pin(format!(
			"{} hex digits is not a whole number of bytes",
			cleaned.len(),
		)));
	}

	(0..cleaned.len())
		.step_by(2)
		.map(|i| {
			u8::from_str_radix(&cleaned[i..i + 2], 16).map_err(|_| {
				ConfigError::Pin(format!("{:?} is not a hex byte", &cleaned[i..i + 2]))
			})
		})
		.collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_pinned_key_round_trips_through_hex() {
		let bytes = vec![0x00, 0x1f, 0xa0, 0xff];
		let rendered = relay_protocol::transport::hex(&bytes);
		assert_eq!(unhex(&rendered).unwrap(), bytes);
	}

	/// Configuring the pin is the one chance to get canopy's identity right, so
	/// a value that is not a key is refused rather than truncated into one.
	#[test]
	fn a_malformed_pin_is_refused() {
		for bad in ["", "   ", "abc", "not hex at all", "00ff0g"] {
			assert!(unhex(bad).is_err(), "{bad:?} must not parse as a key");
		}
	}

	#[test]
	fn whitespace_and_case_in_a_pin_are_tolerated() {
		assert_eq!(unhex("00 1F a0\nFF").unwrap(), vec![0x00, 0x1f, 0xa0, 0xff]);
	}

	fn a_pin() -> String {
		relay_protocol::transport::hex(&[0u8; 32])
	}

	fn anywhere() -> SocketAddr {
		"127.0.0.1:8443".parse().unwrap()
	}

	/// A relay with no usable key must refuse to start. Carrying on would mean
	/// a relay that cannot authenticate, which reads as a cluster canopy cannot
	/// read rather than as the missing secret it is.
	#[test]
	fn a_missing_key_file_is_refused_rather_than_worked_around() {
		let missing = std::env::temp_dir().join("relay-key-that-does-not-exist.pem");
		let err = Config::load(&missing, &a_pin(), anywhere(), "canopy".into())
			.expect_err("a relay without its key must not start");
		assert!(matches!(err, ConfigError::Unreadable { .. }), "got {err:?}");
	}

	#[test]
	fn a_key_file_that_is_not_a_key_is_refused() {
		let path = std::env::temp_dir().join("relay-key-not-a-key.pem");
		std::fs::write(&path, "this is not a PKCS#8 PEM key").unwrap();

		let err = Config::load(&path, &a_pin(), anywhere(), "canopy".into())
			.expect_err("garbage must not be accepted as a key");
		assert!(matches!(err, ConfigError::Key(_)), "got {err:?}");

		let _ = std::fs::remove_file(&path);
	}

	/// The pin is checked as part of loading, so a relay cannot start having
	/// silently failed to learn which canopy it is talking to.
	#[test]
	fn a_relay_will_not_start_without_a_usable_pin() {
		let path = std::env::temp_dir().join("relay-key-for-pin-check.pem");
		let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
		std::fs::write(&path, key.serialize_pem()).unwrap();

		let err = Config::load(&path, "not a key", anywhere(), "canopy".into())
			.expect_err("an unusable pin must not start a relay");
		assert!(matches!(err, ConfigError::Pin(_)), "got {err:?}");

		// And with both in order, it loads.
		assert!(Config::load(&path, &a_pin(), anywhere(), "canopy".into()).is_ok());

		let _ = std::fs::remove_file(&path);
	}
}
