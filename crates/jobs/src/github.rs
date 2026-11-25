//! The credential GitHub is read with, so that private repositories can be read.

use commons_servers::backup_secrets::{ExposeSecret as _, SecretString};

/// `CANOPY_GITHUB_TOKEN`, when set. Unset, only public repositories can be read.
pub fn token() -> Option<SecretString> {
	std::env::var("CANOPY_GITHUB_TOKEN")
		.ok()
		.filter(|token| !token.is_empty())
		.map(SecretString::from)
}

// Basic rather than Bearer: git's smart HTTP refuses a Bearer token.
pub fn authenticate(
	request: reqwest::RequestBuilder,
	token: Option<&SecretString>,
) -> reqwest::RequestBuilder {
	match token {
		Some(token) => request.basic_auth("x-access-token", Some(token.expose_secret())),
		None => request,
	}
}
