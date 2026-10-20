use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::digest::{SHA256, digest};
use serde::{Deserialize, Serialize};

use crate::{Error, Result};

/// The `SubjectPublicKeyInfo` header every P-256 public key shares, ahead of its
/// uncompressed point: the `id-ecPublicKey` and `prime256v1` identifiers.
const P256_SPKI_PREFIX: [u8; 26] = [
	0x30, 0x59, 0x30, 0x13, 0x06, 0x07, 0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01, 0x06, 0x08, 0x2a,
	0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07, 0x03, 0x42, 0x00,
];

/// A P-256 public key, as published in the key set.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PublicJwk {
	pub kty: String,
	pub crv: String,
	pub x: String,
	pub y: String,
	pub kid: String,
	pub alg: String,
	#[serde(rename = "use")]
	pub use_: String,
}

impl PublicJwk {
	/// From an uncompressed point, `0x04 || x || y`.
	pub fn from_point(point: &[u8]) -> Result<Self> {
		let [0x04, coords @ ..] = point else {
			return Err(Error::NotP256("an uncompressed point".into()));
		};
		if coords.len() != 64 {
			return Err(Error::NotP256("an uncompressed point".into()));
		}
		let x = URL_SAFE_NO_PAD.encode(&coords[..32]);
		let y = URL_SAFE_NO_PAD.encode(&coords[32..]);
		let kid = thumbprint(&x, &y);
		Ok(Self {
			kty: "EC".into(),
			crv: "P-256".into(),
			x,
			y,
			kid,
			alg: "ES256".into(),
			use_: "sig".into(),
		})
	}

	/// From a DER `SubjectPublicKeyInfo`, the form the key service returns.
	pub fn from_spki_der(spki: &[u8], key: &str) -> Result<Self> {
		spki.strip_prefix(&P256_SPKI_PREFIX[..])
			.ok_or_else(|| Error::NotP256(key.to_owned()))
			.and_then(Self::from_point)
	}
}

/// The RFC 7638 thumbprint, which is the key's `kid`: derived from the key
/// itself, so every component that publishes or signs with it names it the same
/// way without being told.
fn thumbprint(x: &str, y: &str) -> String {
	let canonical = format!(r#"{{"crv":"P-256","kty":"EC","x":"{x}","y":"{y}"}}"#);
	URL_SAFE_NO_PAD.encode(digest(&SHA256, canonical.as_bytes()))
}

/// A key set, as served at the issuer.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Jwks {
	pub keys: Vec<PublicJwk>,
}

#[cfg(test)]
mod tests {
	use super::*;

	/// RFC 7638 does not give a P-256 example, so this is checked against an
	/// independent computation of the same canonical form.
	#[test]
	fn thumbprint_is_of_the_canonical_members() {
		let x = "f83OJ3D2xF1Bg8vub9tLe1gHMzV76e8Tus9uPHvRVEU";
		let y = "x_FEzRu9m36HLN_tue659LNpXW6pCyStikYjKIWI5a0";
		assert_eq!(
			thumbprint(x, y),
			"oKIywvGUpTVTyxMQ3bwIIeQUudfr_CkLMjCE19ECD-U"
		);
	}

	#[test]
	fn spki_round_trips_to_the_same_point() {
		let mut point = vec![0x04];
		point.extend((0u8..64).collect::<Vec<_>>());
		let mut spki = P256_SPKI_PREFIX.to_vec();
		spki.extend(&point);
		assert_eq!(
			PublicJwk::from_spki_der(&spki, "k").unwrap(),
			PublicJwk::from_point(&point).unwrap()
		);
	}

	#[test]
	fn other_curves_are_refused() {
		let mut spki = P256_SPKI_PREFIX.to_vec();
		spki[22] = 0x22;
		spki.extend([0x04; 65]);
		assert!(PublicJwk::from_spki_der(&spki, "k").is_err());
		assert!(PublicJwk::from_point(&[0x02; 33]).is_err());
	}
}
