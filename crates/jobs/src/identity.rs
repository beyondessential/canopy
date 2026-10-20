//! Watches what the identity provider's issuer serves.
//!
//! Spec: `.workhorse/specs/private-server/identity-provider.md` (id `IDP`),
//! "Signing keys".
//!
//! Canopy judges a key's maturity from the key set as actually served at the
//! issuer, because the discovery component and the device API take up a
//! configuration change separately. So each pass fetches the discovery document
//! and the key set it names, records when each configured key is first seen
//! served, and raises the self-alert when what is served differs from the
//! configuration.

use std::time::Duration;

use database::{
	identity::{self, IdentityKey},
	self_alerts::{IssuerDocuments, sweep_identity_discovery},
};
use jiff::Timestamp;
use oidc::{Issuer, Jwks};
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, warn};

const TICK: Duration = Duration::from_secs(60);
const FETCH_TIMEOUT: Duration = Duration::from_secs(10);

/// Fetch the documents served at `issuer`.
pub async fn fetch(client: &reqwest::Client, issuer: &str) -> IssuerDocuments {
	async fn json(client: &reqwest::Client, url: &str) -> Result<serde_json::Value, String> {
		client
			.get(url)
			.timeout(FETCH_TIMEOUT)
			.send()
			.await
			.and_then(reqwest::Response::error_for_status)
			.map_err(|err| format!("{url}: {err}"))?
			.json()
			.await
			.map_err(|err| format!("{url}: {err}"))
	}

	let discovery = match json(client, &format!("{issuer}{}", oidc::DISCOVERY_PATH)).await {
		Ok(doc) => doc,
		Err(err) => return IssuerDocuments::Unreachable(err),
	};
	let named = discovery["issuer"].as_str().unwrap_or_default();
	if named != issuer {
		return IssuerDocuments::OtherIssuer(named.to_owned());
	}
	let Some(jwks_uri) = discovery["jwks_uri"].as_str() else {
		return IssuerDocuments::Unreachable("the discovery document names no jwks_uri".into());
	};
	let jwks = match json(client, jwks_uri).await {
		Ok(jwks) => jwks,
		Err(err) => return IssuerDocuments::Unreachable(err),
	};
	match serde_json::from_value::<Jwks>(jwks) {
		Ok(jwks) => IssuerDocuments::Served(jwks.keys.into_iter().map(|key| key.kid).collect()),
		Err(err) => IssuerDocuments::Unreachable(format!("{jwks_uri}: not a key set: {err}")),
	}
}

/// One pass.
pub async fn tick(
	db: &mut database::diesel_async::AsyncPgConnection,
	client: &reqwest::Client,
	issuer: &Issuer,
) {
	let keys = issuer.keyring.keys().await;
	let configured: Vec<(String, String)> = keys
		.iter()
		.filter_map(|(key_ref, key)| Some((key.as_ref()?.kid().to_owned(), key_ref.clone())))
		.collect();
	if configured.is_empty() {
		// Nothing to compare against until a key's public half can be read; the
		// device API refuses to sign for the same reason, and says so.
		warn!("identity: no configured signing key could be read");
		return;
	}
	for (kid, key_ref) in &configured {
		if let Err(err) = IdentityKey::know(db, kid, key_ref).await {
			error!(%kid, %err, "identity: recording a configured key failed");
		}
	}

	let documents = fetch(client, &issuer.issuer).await;
	if let IssuerDocuments::Served(served) = &documents {
		let now = Timestamp::now();
		for (kid, key_ref) in &configured {
			if served.contains(kid)
				&& let Err(err) = IdentityKey::observe_served(db, kid, key_ref, now).await
			{
				error!(%kid, %err, "identity: recording a served key failed");
			}
		}
	}

	let observed = match IdentityKey::all(db).await {
		Ok(observed) => observed,
		Err(err) => {
			error!(%err, "identity: reading observed keys failed");
			return;
		}
	};
	let kids: Vec<Option<&str>> = keys
		.iter()
		.map(|(_, key)| key.as_ref().map(|k| k.kid()))
		.collect();
	let signing = identity::signing_index(&kids, &observed, Timestamp::now()).and_then(|i| kids[i]);
	let configured: Vec<String> = configured.into_iter().map(|(kid, _)| kid).collect();

	match sweep_identity_discovery(db, &issuer.issuer, &documents, &configured, signing).await {
		Ok(_) => debug!(?signing, "identity: documents checked"),
		Err(err) => error!(%err, "identity: discovery self-alert sweep failed"),
	}
}

/// Watch the issuer, when this Canopy instance has one configured.
pub fn spawn() -> JoinHandle<()> {
	task::spawn(async move {
		let Some(issuer) = Issuer::from_env().await else {
			debug!("identity: no issuer configured; not watching");
			return;
		};
		let pool = database::init();
		let client = reqwest::Client::new();
		loop {
			match pool.get().await {
				Ok(mut db) => tick(&mut db, &client, &issuer).await,
				Err(err) => error!(%err, "identity: no database connection"),
			}
			sleep(TICK).await;
		}
	})
}
