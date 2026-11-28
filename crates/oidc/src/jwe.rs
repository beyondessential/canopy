//! Compact JSON Web Encryption to a device's P-256 key, with `ECDH-ES` key
//! agreement and `A256GCM` content encryption (RFC 7516, RFC 7518).
//!
//! Only what a sign-in token needs: direct key agreement, so the encrypted key
//! is empty, no `apu` or `apv`, and a key derived in one round of the Concat
//! KDF.
// spec: TSI#encryption

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use ring::{
	aead::{AES_256_GCM, Aad, LessSafeKey, NONCE_LEN, Nonce, UnboundKey},
	agreement::{ECDH_P256, EphemeralPrivateKey, UnparsedPublicKey, agree_ephemeral},
	digest::{SHA256, digest},
	rand::{SecureRandom, SystemRandom},
};

use crate::{Error, Result, jwk::PublicJwk};

const ALG: &str = "ECDH-ES";
const ENC: &str = "A256GCM";
const KEY_BITS: u32 = 256;

/// The JWK thumbprint of a P-256 public key held as a DER
/// `SubjectPublicKeyInfo`, which names it in a JWE's `kid`.
pub fn thumbprint(spki: &[u8]) -> Result<String> {
	Ok(PublicJwk::from_spki_der(spki, "device key")?.kid)
}

/// Encrypt `plaintext` to the P-256 public key held as a DER
/// `SubjectPublicKeyInfo`, as a compact JWE whose header names the key by its
/// thumbprint and carries `cty`.
pub fn encrypt(spki: &[u8], cty: &str, plaintext: &[u8]) -> Result<String> {
	let recipient = PublicJwk::from_spki_der(spki, "device key")?;
	let point = point(&recipient)?;

	let rng = SystemRandom::new();
	let ephemeral = EphemeralPrivateKey::generate(&ECDH_P256, &rng)
		.map_err(|_| Error::Encryption("generating an ephemeral key"))?;
	let epk = PublicJwk::from_point(
		ephemeral
			.compute_public_key()
			.map_err(|_| Error::Encryption("computing the ephemeral public key"))?
			.as_ref(),
	)?;
	let cek = agree_ephemeral(ephemeral, &UnparsedPublicKey::new(&ECDH_P256, point), |z| {
		concat_kdf(z, ENC.as_bytes(), b"", b"", KEY_BITS)
	})
	.map_err(|_| Error::Encryption("agreeing a key with the device key"))?;

	let header = serde_json::json!({
		"alg": ALG,
		"enc": ENC,
		"cty": cty,
		"kid": recipient.kid,
		"epk": { "kty": "EC", "crv": "P-256", "x": epk.x, "y": epk.y },
	});
	let header = URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("a header serialises"));

	let mut iv = [0u8; NONCE_LEN];
	rng.fill(&mut iv)
		.map_err(|_| Error::Encryption("drawing an initialisation vector"))?;
	let key = LessSafeKey::new(
		UnboundKey::new(&AES_256_GCM, &cek).expect("a 256-bit key is an AES-256 key"),
	);
	let mut ciphertext = plaintext.to_vec();
	let tag = key
		.seal_in_place_separate_tag(
			Nonce::assume_unique_for_key(iv),
			Aad::from(header.as_bytes()),
			&mut ciphertext,
		)
		.map_err(|_| Error::Encryption("sealing the content"))?;

	Ok(format!(
		"{header}..{}.{}.{}",
		URL_SAFE_NO_PAD.encode(iv),
		URL_SAFE_NO_PAD.encode(ciphertext),
		URL_SAFE_NO_PAD.encode(tag.as_ref()),
	))
}

/// The uncompressed point of a P-256 public key.
fn point(jwk: &PublicJwk) -> Result<Vec<u8>> {
	let mut point = vec![0x04];
	for coordinate in [&jwk.x, &jwk.y] {
		point.extend(
			URL_SAFE_NO_PAD
				.decode(coordinate)
				.map_err(|_| Error::NotP256(jwk.kid.clone()))?,
		);
	}
	Ok(point)
}

/// The Concat KDF of RFC 7518 §4.6.2, for a key of at most one SHA-256 round.
fn concat_kdf(z: &[u8], algorithm: &[u8], apu: &[u8], apv: &[u8], key_bits: u32) -> Vec<u8> {
	assert!(key_bits <= 256, "one round derives at most 256 bits");
	let mut input = Vec::with_capacity(4 + z.len() + 24 + algorithm.len() + apu.len() + apv.len());
	input.extend(1u32.to_be_bytes());
	input.extend(z);
	for field in [algorithm, apu, apv] {
		input.extend(
			u32::try_from(field.len())
				.expect("a KDF field fits a length prefix")
				.to_be_bytes(),
		);
		input.extend(field);
	}
	input.extend(key_bits.to_be_bytes());
	digest(&SHA256, &input).as_ref()[..(key_bits / 8) as usize].to_vec()
}

/// Open a compact JWE made by [`encrypt`] with the recipient's private key,
/// given as PKCS#8 DER. Returns the content type and the plaintext.
#[cfg(any(test, feature = "decrypt"))]
pub fn decrypt(pkcs8: &[u8], jwe: &str) -> Result<(String, Vec<u8>)> {
	use p256::{PublicKey, SecretKey, ecdh::diffie_hellman, pkcs8::DecodePrivateKey};

	let malformed = |what| Error::Decryption(what);
	let mut parts = jwe.split('.');
	let (Some(header_b64), Some(""), Some(iv), Some(ciphertext), Some(tag), None) = (
		parts.next(),
		parts.next(),
		parts.next(),
		parts.next(),
		parts.next(),
		parts.next(),
	) else {
		return Err(malformed("not a compact JWE with an empty encrypted key"));
	};
	let decode = |part: &str| {
		URL_SAFE_NO_PAD
			.decode(part)
			.map_err(|_| malformed("a part is not base64url"))
	};
	let header: serde_json::Value = serde_json::from_slice(&decode(header_b64)?)
		.map_err(|_| malformed("the header is not JSON"))?;
	if header["alg"] != ALG || header["enc"] != ENC {
		return Err(malformed("not ECDH-ES with A256GCM"));
	}

	let secret = SecretKey::from_pkcs8_der(pkcs8).map_err(|_| malformed("not a P-256 key"))?;
	let own = PublicJwk::from_point(
		p256::elliptic_curve::sec1::ToEncodedPoint::to_encoded_point(&secret.public_key(), false)
			.as_bytes(),
	)?;
	if header["kid"] != own.kid {
		return Err(malformed("encrypted to another key"));
	}
	let epk = PublicJwk {
		x: header["epk"]["x"].as_str().unwrap_or_default().into(),
		y: header["epk"]["y"].as_str().unwrap_or_default().into(),
		..own
	};
	let epk = PublicKey::from_sec1_bytes(&point(&epk)?)
		.map_err(|_| malformed("the ephemeral key is not on P-256"))?;
	let shared = diffie_hellman(secret.to_nonzero_scalar(), epk.as_affine());
	let cek = concat_kdf(
		shared.raw_secret_bytes(),
		ENC.as_bytes(),
		b"",
		b"",
		KEY_BITS,
	);

	let iv: [u8; NONCE_LEN] = decode(iv)?
		.try_into()
		.map_err(|_| malformed("the initialisation vector is not 96 bits"))?;
	let mut sealed = decode(ciphertext)?;
	sealed.extend(decode(tag)?);
	let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, &cek).expect("a 256-bit key"));
	let plaintext = key
		.open_in_place(
			Nonce::assume_unique_for_key(iv),
			Aad::from(header_b64.as_bytes()),
			&mut sealed,
		)
		.map_err(|_| malformed("the content does not authenticate"))?;
	Ok((
		header["cty"].as_str().unwrap_or_default().to_owned(),
		plaintext.to_vec(),
	))
}

#[cfg(test)]
mod tests {
	use p256::{
		SecretKey,
		pkcs8::{DecodePrivateKey, EncodePublicKey},
	};
	use ring::signature::{ECDSA_P256_SHA256_FIXED_SIGNING, EcdsaKeyPair};

	use super::*;

	/// RFC 7518 Appendix C.
	#[test]
	fn concat_kdf_matches_the_rfc_example() {
		let z = [
			158, 86, 217, 29, 129, 113, 53, 211, 114, 131, 66, 131, 191, 132, 38, 156, 251, 49,
			110, 163, 218, 128, 106, 72, 246, 218, 167, 121, 140, 254, 144, 196,
		];
		assert_eq!(
			URL_SAFE_NO_PAD.encode(concat_kdf(&z, b"A128GCM", b"Alice", b"Bob", 128)),
			"VqqN6vgjbSBcIijNcacQGg"
		);
	}

	fn device_key() -> (Vec<u8>, Vec<u8>) {
		let pkcs8 =
			EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_FIXED_SIGNING, &SystemRandom::new())
				.unwrap()
				.as_ref()
				.to_vec();
		let spki = SecretKey::from_pkcs8_der(&pkcs8)
			.unwrap()
			.public_key()
			.to_public_key_der()
			.unwrap()
			.as_bytes()
			.to_vec();
		(spki, pkcs8)
	}

	#[test]
	fn round_trips_to_the_key_it_names() {
		let (spki, pkcs8) = device_key();
		let jwe = encrypt(&spki, "JWT", b"a.signed.token").unwrap();

		let header: serde_json::Value = serde_json::from_slice(
			&URL_SAFE_NO_PAD
				.decode(jwe.split('.').next().unwrap())
				.unwrap(),
		)
		.unwrap();
		assert_eq!(header["alg"], "ECDH-ES");
		assert_eq!(header["enc"], "A256GCM");
		assert_eq!(header["cty"], "JWT");
		assert_eq!(header["kid"], thumbprint(&spki).unwrap());
		assert_eq!(jwe.split('.').nth(1), Some(""));

		assert_eq!(
			decrypt(&pkcs8, &jwe).unwrap(),
			("JWT".to_owned(), b"a.signed.token".to_vec())
		);
	}

	#[test]
	fn another_key_cannot_open_it() {
		let (spki, _) = device_key();
		let (_, other) = device_key();
		let jwe = encrypt(&spki, "JWT", b"x").unwrap();
		assert!(decrypt(&other, &jwe).is_err());
	}

	#[test]
	fn a_tampered_header_does_not_authenticate() {
		let (spki, pkcs8) = device_key();
		let jwe = encrypt(&spki, "JWT", b"x").unwrap();
		let (header, rest) = jwe.split_once('.').unwrap();
		let mut decoded: serde_json::Value =
			serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).unwrap()).unwrap();
		decoded["cty"] = "other".into();
		let tampered = format!(
			"{}.{rest}",
			URL_SAFE_NO_PAD.encode(serde_json::to_vec(&decoded).unwrap())
		);
		assert!(decrypt(&pkcs8, &tampered).is_err());
	}

	#[test]
	fn a_key_off_p256_is_refused() {
		let (mut spki, _) = device_key();
		spki[22] = 0x22;
		assert!(encrypt(&spki, "JWT", b"x").is_err());
	}
}
