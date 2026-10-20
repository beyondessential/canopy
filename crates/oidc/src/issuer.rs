use crate::{IdentityConfig, Keyring};

/// What a Canopy instance issues tokens as: its issuer and its signing keys.
#[derive(Clone, Debug)]
pub struct Issuer {
	pub issuer: String,
	pub keyring: Keyring,
}

impl Issuer {
	/// From the instance's configuration in the environment, with keys held by
	/// the key service. `None` when the configuration lacks an issuer or keys,
	/// or does not parse, in which case no tokens are issued.
	// spec: IDP#configuration
	pub async fn from_env() -> Option<Self> {
		match IdentityConfig::from_env() {
			Ok(config) => Self::from_config(&config).await,
			Err(err) => {
				tracing::warn!(%err, "identity provider configuration ignored");
				None
			}
		}
	}

	/// From a configuration already read, with keys held by the key service.
	/// `None` when it lacks an issuer or keys.
	pub async fn from_config(config: &IdentityConfig) -> Option<Self> {
		let issuer = config.issuer.clone().filter(|_| config.issues_tokens())?;
		let aws = aws_config::load_defaults(aws_config::BehaviorVersion::latest()).await;
		Some(Self {
			issuer,
			keyring: Keyring::kms(aws_sdk_kms::Client::new(&aws), &config.keys),
		})
	}
}

/// The identity provider as a server holds it: the configuration it presents,
/// and the issuer built from that configuration when it can sign. Built only
/// from a configuration or from an issuer, so the two never disagree.
// spec: IDP#configuration
#[derive(Clone, Debug, Default)]
pub struct IdentityProvider {
	config: IdentityConfig,
	issuer: Option<Issuer>,
}

impl IdentityProvider {
	/// From the instance's configuration in the environment. A configuration
	/// that does not parse is logged and presented as none, so the server still
	/// comes up and the Canopy instance issues no tokens.
	pub async fn from_env() -> Self {
		match IdentityConfig::from_env() {
			Ok(config) => Self::from_config(config).await,
			Err(err) => {
				tracing::warn!(%err, "identity provider configuration ignored");
				Self::default()
			}
		}
	}

	/// From a configuration already read, signing with the key service where
	/// it names an issuer and keys.
	pub async fn from_config(config: IdentityConfig) -> Self {
		let issuer = Issuer::from_config(&config).await;
		Self { config, issuer }
	}

	/// Presenting `config` while signing nothing, for a server whose issuance
	/// is not exercised.
	pub fn without_signing(config: IdentityConfig) -> Self {
		Self {
			config,
			issuer: None,
		}
	}

	/// Signing as `issuer`, presenting the configuration it was built from.
	pub fn from_issuer(issuer: Issuer) -> Self {
		Self {
			config: IdentityConfig {
				issuer: Some(issuer.issuer.clone()),
				keys: issuer.keyring.refs().map(str::to_owned).collect(),
			},
			issuer: Some(issuer),
		}
	}

	/// The configuration presented.
	pub fn config(&self) -> &IdentityConfig {
		&self.config
	}

	/// The issuer tokens are signed as, when this server signs.
	pub fn issuer(&self) -> Option<&Issuer> {
		self.issuer.as_ref()
	}
}
