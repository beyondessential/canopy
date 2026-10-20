use crate::{Error, Result};

/// The issuer this Canopy instance signs as: a URL of its own, fixed in every
/// relying party's trust configuration.
pub const ISSUER_ENV: &str = "CANOPY_IDENTITY_ISSUER";

/// The signing keys, as key-service references (ARNs), oldest first, separated
/// by commas or whitespace.
pub const KEYS_ENV: &str = "CANOPY_IDENTITY_KEYS";

/// The identity provider's configuration, read once at startup.
// spec: IDP#configuration
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct IdentityConfig {
	/// The issuer URL, without a trailing slash. `None` when not configured.
	pub issuer: Option<String>,
	/// Key-service references to the signing keys, oldest first.
	pub keys: Vec<String>,
}

impl IdentityConfig {
	/// Read the configuration from the environment.
	pub fn from_env() -> Result<Self> {
		Self::parse(
			std::env::var(ISSUER_ENV).ok().as_deref(),
			std::env::var(KEYS_ENV).ok().as_deref(),
		)
	}

	/// Parse a configuration. An empty issuer or key list is the same as an
	/// absent one.
	pub fn parse(issuer: Option<&str>, keys: Option<&str>) -> Result<Self> {
		let issuer = issuer
			.map(str::trim)
			.filter(|issuer| !issuer.is_empty())
			.map(normalise_issuer)
			.transpose()?;
		let keys = keys
			.unwrap_or_default()
			.split([',', ' ', '\t', '\n', '\r'])
			.map(str::trim)
			.filter(|key| !key.is_empty())
			.map(str::to_owned)
			.collect::<Vec<_>>();
		Ok(Self { issuer, keys })
	}

	/// Whether this Canopy instance can issue tokens at all: it has an issuer
	/// and at least one key.
	// spec: IDP#configuration
	pub fn issues_tokens(&self) -> bool {
		self.issuer.is_some() && !self.keys.is_empty()
	}
}

/// An issuer is an `https` URL with no userinfo, query or fragment, held without a
/// trailing slash so the documents' paths join onto it one way.
fn normalise_issuer(issuer: &str) -> Result<String> {
	let rest = issuer.strip_prefix("https://").ok_or_else(|| {
		Error::Config(format!("{ISSUER_ENV} must be an https URL, got {issuer:?}"))
	})?;
	let authority = rest.split('/').next().unwrap_or_default();
	if authority.is_empty() || rest.contains(['?', '#']) || authority.contains('@') {
		return Err(Error::Config(format!(
			"{ISSUER_ENV} must be an https URL with a host and no userinfo, query or fragment, got {issuer:?}"
		)));
	}
	Ok(issuer.trim_end_matches('/').to_owned())
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn absent_and_empty_are_unconfigured() {
		assert!(!IdentityConfig::parse(None, None).unwrap().issues_tokens());
		assert!(
			!IdentityConfig::parse(Some(""), Some(" "))
				.unwrap()
				.issues_tokens()
		);
		assert!(
			!IdentityConfig::parse(Some("https://id.example"), None)
				.unwrap()
				.issues_tokens()
		);
		assert!(
			!IdentityConfig::parse(None, Some("arn:a"))
				.unwrap()
				.issues_tokens()
		);
	}

	#[test]
	fn keys_keep_their_order() {
		let config =
			IdentityConfig::parse(Some("https://id.example/"), Some("arn:a, arn:b\narn:c"))
				.unwrap();
		assert_eq!(config.issuer.as_deref(), Some("https://id.example"));
		assert_eq!(config.keys, ["arn:a", "arn:b", "arn:c"]);
		assert!(config.issues_tokens());
	}

	#[test]
	fn issuer_must_be_https() {
		for bad in [
			"http://id.example",
			"id.example",
			"https://",
			"https://id.example?x=1",
			"https://id.example#x",
			"https://user:secret@id.example",
			"https://user@id.example/path",
		] {
			assert!(IdentityConfig::parse(Some(bad), None).is_err(), "{bad}");
		}
		assert!(IdentityConfig::parse(Some("https://id.example/a@b"), None).is_ok());
		assert_eq!(
			IdentityConfig::parse(Some("https://id.example/canopy"), None)
				.unwrap()
				.issuer
				.as_deref(),
			Some("https://id.example/canopy")
		);
	}
}
