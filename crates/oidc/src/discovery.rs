use serde_json::{Value, json};

/// Where the discovery document is served, beneath the issuer.
pub const DISCOVERY_PATH: &str = "/.well-known/openid-configuration";

/// Where the key set is served, beneath the issuer.
pub const JWKS_PATH: &str = "/.well-known/jwks.json";

/// The claims a token can carry.
// spec: IDT#the-token
const CLAIMS: &[&str] = &[
	"iss",
	"aud",
	"iat",
	"nbf",
	"exp",
	"jti",
	"sub",
	"group_id",
	"group_name",
	"rank",
	"machine_id",
	"application_id",
	"application_type",
];

pub fn jwks_uri(issuer: &str) -> String {
	format!("{issuer}{JWKS_PATH}")
}

/// The discovery document for `issuer`.
// spec: IDP#discovery
pub fn discovery_document(issuer: &str) -> Value {
	json!({
		"issuer": issuer,
		"jwks_uri": jwks_uri(issuer),
		"response_types_supported": ["id_token"],
		"subject_types_supported": ["public"],
		"id_token_signing_alg_values_supported": ["ES256"],
		"claims_supported": CLAIMS,
	})
}
