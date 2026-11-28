use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::Serialize;

use crate::{Result, SigningKey};

/// The header type of an identity token.
// spec: IDT#the-token
pub const IDENTITY_TOKEN_TYPE: &str = "JWT";

/// The header type of a sign-in token, which keeps one from being taken for an
/// identity token.
// spec: TSI#signed-token
pub const SIGN_IN_TOKEN_TYPE: &str = "canopy-sign-in+jwt";

/// Encode and sign a JSON Web Token with ES256, naming the signing key and the
/// token's type in its header.
pub async fn encode(key: &SigningKey, typ: &str, claims: &impl Serialize) -> Result<String> {
	let header = serde_json::json!({ "alg": "ES256", "typ": typ, "kid": key.kid() });
	let mut token = format!(
		"{}.{}",
		URL_SAFE_NO_PAD.encode(serde_json::to_vec(&header).expect("a header serialises")),
		URL_SAFE_NO_PAD.encode(serde_json::to_vec(claims).expect("claims serialise")),
	);
	let signature = key.sign(token.as_bytes()).await?;
	token.push('.');
	token.push_str(&URL_SAFE_NO_PAD.encode(signature));
	Ok(token)
}

#[cfg(test)]
mod tests {
	use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};

	use super::*;

	#[tokio::test]
	async fn signs_verifiably_and_names_its_key() {
		let key = SigningKey::generate_local("local");
		let token = encode(&key, SIGN_IN_TOKEN_TYPE, &serde_json::json!({ "sub": "x" }))
			.await
			.unwrap();
		let mut parts = token.split('.');
		let (header, claims, signature) = (
			parts.next().unwrap(),
			parts.next().unwrap(),
			parts.next().unwrap(),
		);
		assert!(parts.next().is_none());

		let header: serde_json::Value =
			serde_json::from_slice(&URL_SAFE_NO_PAD.decode(header).unwrap()).unwrap();
		assert_eq!(header["alg"], "ES256");
		assert_eq!(header["typ"], SIGN_IN_TOKEN_TYPE);
		assert_eq!(header["kid"], key.kid());
		let claims: serde_json::Value =
			serde_json::from_slice(&URL_SAFE_NO_PAD.decode(claims).unwrap()).unwrap();
		assert_eq!(claims["sub"], "x");

		let mut point = vec![0x04];
		point.extend(URL_SAFE_NO_PAD.decode(&key.jwk.x).unwrap());
		point.extend(URL_SAFE_NO_PAD.decode(&key.jwk.y).unwrap());
		let signed = &token[..token.rfind('.').unwrap()];
		UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
			.verify(
				signed.as_bytes(),
				&URL_SAFE_NO_PAD.decode(signature).unwrap(),
			)
			.expect("the signature verifies against the published key");
	}
}
