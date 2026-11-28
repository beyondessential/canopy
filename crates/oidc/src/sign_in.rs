use std::time::Duration;

use serde::{Deserialize, Serialize};

/// How long a sign-in token is valid for.
// spec: TSI#signed-token
pub const SIGN_IN_LIFETIME: Duration = Duration::from_secs(60);

/// A sign-in token's claims.
// spec: TSI#signed-token
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignInClaims {
	pub iss: String,
	pub iat: i64,
	pub nbf: i64,
	pub exp: i64,
	pub jti: String,
	/// `person:<login>`, the person's tailnet login in lowercase.
	pub sub: String,
	/// The origin of the sign-in address the token is sent to.
	pub origin: String,
	/// The key the application's reporter names it by.
	pub application_key: String,
	/// The type the application's reporter names it by.
	pub application_type: String,
}

impl SignInClaims {
	/// The subject naming a person by their tailnet login.
	pub fn person(login: &str) -> String {
		format!("person:{}", login.to_lowercase())
	}
}
