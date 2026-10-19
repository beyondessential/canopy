use crate::{IdentityConfig, Keyring};

/// What a Canopy instance issues tokens as: its issuer and its signing keys.
#[derive(Clone, Debug)]
pub struct Issuer {
	pub issuer: String,
	pub keyring: Keyring,
}

impl Issuer {
	/// From the instance's configuration, with keys held by the key service.
	/// `None` when the configuration lacks an issuer or keys, or does not parse,
	/// in which case no tokens are issued.
	// spec: IDP#configuration
	pub async fn from_env() -> Option<Self> {
		let config = match IdentityConfig::from_env() {
			Ok(config) => config,
			Err(err) => {
				tracing::warn!(%err, "identity provider configuration ignored");
				return None;
			}
		};
		let issuer = config.issuer.filter(|_| !config.keys.is_empty())?;
		let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		Some(Self {
			issuer,
			keyring: Keyring::kms(aws_sdk_kms::Client::new(&aws), &config.keys),
		})
	}
}
