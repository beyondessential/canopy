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
//! configuration or when no configured key can sign.

use std::time::Duration;

use database::{
	identity::{self, ConfiguredKey, IdentityKey},
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

/// How many passes in a row must find the documents unreachable before the
/// alert is raised for it, so one dropped fetch does not raise and clear it.
const UNREACHABLE_PASSES: u32 = 3;

/// The client the issuer is fetched with. It follows no redirects: the
/// documents are expected at the issuer itself, and one that redirects is a
/// finding rather than somewhere to go.
pub fn client() -> reqwest::Client {
	reqwest::Client::builder()
		.redirect(reqwest::redirect::Policy::none())
		.build()
		.expect("building the identity provider's HTTP client")
}

/// Fetch the documents served at `issuer`. The key set is fetched only from the
/// issuer's own address for it, whatever the discovery document names, so a
/// document naming somewhere else is a finding and never a request.
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
	let expected = oidc::jwks_uri(issuer);
	match discovery["jwks_uri"].as_str() {
		Some(named) if named == expected => {}
		Some(named) => {
			return IssuerDocuments::Unreachable(format!(
				"the discovery document names the key set at {named}, not {expected}"
			));
		}
		None => {
			return IssuerDocuments::Unreachable("the discovery document names no jwks_uri".into());
		}
	}
	let jwks = match json(client, &expected).await {
		Ok(jwks) => jwks,
		Err(err) => return IssuerDocuments::Unreachable(err),
	};
	match serde_json::from_value::<Jwks>(jwks) {
		Ok(jwks) => IssuerDocuments::Served(jwks.keys.into_iter().map(|key| key.kid).collect()),
		Err(err) => IssuerDocuments::Unreachable(format!("{expected}: not a key set: {err}")),
	}
}

/// What the watch carries from one pass to the next.
#[derive(Debug, Default)]
pub struct Watch {
	unreachable_passes: u32,
}

impl Watch {
	/// Note what this pass found, returning whether to evaluate the alert on
	/// it: not while the documents have been unreachable for fewer than
	/// [`UNREACHABLE_PASSES`] passes in a row.
	pub fn found(&mut self, documents: &IssuerDocuments) -> bool {
		if matches!(documents, IssuerDocuments::Unreachable(_)) {
			self.unreachable_passes += 1;
			self.unreachable_passes >= UNREACHABLE_PASSES
		} else {
			self.unreachable_passes = 0;
			true
		}
	}
}

/// One pass.
pub async fn tick(
	db: &mut database::diesel_async::AsyncPgConnection,
	client: &reqwest::Client,
	issuer: &Issuer,
	watch: &mut Watch,
) {
	let keys = issuer.keyring.keys().await;
	for (key_ref, key) in &keys {
		if let Some(key) = key
			&& let Err(err) = IdentityKey::know(db, key.kid(), key_ref).await
		{
			error!(kid = key.kid(), %err, "identity: recording a configured key failed");
		}
	}
	if keys.iter().all(|(_, key)| key.is_none()) {
		warn!("identity: no configured signing key could be read");
	}

	let documents = fetch(client, &issuer.issuer).await;
	let evaluate = watch.found(&documents);

	let observed = match IdentityKey::all(db).await {
		Ok(observed) => observed,
		Err(err) => {
			error!(%err, "identity: reading observed keys failed");
			return;
		}
	};
	if let IssuerDocuments::Served(served) = &documents {
		let now = Timestamp::now();
		for (key_ref, key) in &keys {
			// A key whose public half cannot be read now is still known by the
			// identifier it was last read as.
			let kid = match key {
				Some(key) => Some(key.kid()),
				None => IdentityKey::by_ref(&observed, key_ref).map(|row| row.kid.as_str()),
			};
			if let Some(kid) = kid
				&& served.contains(kid)
				&& let Err(err) = IdentityKey::observe_served(db, kid, key_ref, now).await
			{
				error!(%kid, %err, "identity: recording a served key failed");
			}
		}
	}
	if !evaluate {
		debug!("identity: documents unreachable; waiting before raising");
		return;
	}

	let observed = match IdentityKey::all(db).await {
		Ok(observed) => observed,
		Err(err) => {
			error!(%err, "identity: reading observed keys failed");
			return;
		}
	};
	let configured: Vec<ConfiguredKey> = keys
		.iter()
		.map(|(key_ref, key)| ConfiguredKey {
			key_ref,
			kid: key.as_ref().map(|k| k.kid()),
		})
		.collect();
	let signing = identity::signing(&configured, &observed, Timestamp::now());
	let signing_kid = signing
		.as_ref()
		.map(|&i| configured[i].kid.expect("a signing key is read"));
	let known: Vec<String> = keys
		.iter()
		.filter_map(|(key_ref, key)| match key {
			Some(key) => Some(key.kid().to_owned()),
			None => IdentityKey::by_ref(&observed, key_ref).map(|row| row.kid.clone()),
		})
		.collect();

	match sweep_identity_discovery(db, &issuer.issuer, &documents, &known, signing_kid).await {
		Ok(_) => debug!(?signing_kid, "identity: documents checked"),
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
		let client = client();
		let mut watch = Watch::default();
		loop {
			match pool.get().await {
				Ok(mut db) => tick(&mut db, &client, &issuer, &mut watch).await,
				Err(err) => error!(%err, "identity: no database connection"),
			}
			sleep(TICK).await;
		}
	})
}

#[cfg(test)]
mod tests {
	use std::sync::{
		Arc,
		atomic::{AtomicU32, Ordering},
	};

	use axum::{Json, Router, response::Redirect, routing::get};

	use super::*;

	/// Serve `router` on loopback, returning its address as an issuer.
	async fn serve(router: Router) -> String {
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let addr = listener.local_addr().unwrap();
		tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
		format!("http://{addr}")
	}

	#[tokio::test]
	async fn a_key_set_named_elsewhere_is_never_fetched() {
		let elsewhere = Arc::new(AtomicU32::new(0));
		let hits = elsewhere.clone();
		let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
		let issuer = format!("http://{}", listener.local_addr().unwrap());
		let named = issuer.clone();
		let router = Router::new()
			.route(
				oidc::DISCOVERY_PATH,
				get(async move || {
					Json(serde_json::json!({
						"issuer": named,
						"jwks_uri": format!("{named}/elsewhere"),
					}))
				}),
			)
			.route(
				"/elsewhere",
				get(async move || {
					hits.fetch_add(1, Ordering::SeqCst);
					Json(serde_json::json!({ "keys": [] }))
				}),
			);
		tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

		let documents = fetch(&client(), &issuer).await;
		assert!(
			matches!(&documents, IssuerDocuments::Unreachable(err) if err.contains("names the key set at")),
			"{documents:?}"
		);
		assert_eq!(elsewhere.load(Ordering::SeqCst), 0);
	}

	#[test]
	fn unreachable_documents_are_evaluated_only_once_they_stay_so() {
		let down = IssuerDocuments::Unreachable("timed out".into());
		let up = IssuerDocuments::Served(Default::default());
		let mut watch = Watch::default();
		assert!(!watch.found(&down));
		assert!(!watch.found(&down));
		assert!(watch.found(&down));
		assert!(watch.found(&up));
		assert!(!watch.found(&down), "a recovery starts the count again");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn with_no_key_readable_the_alert_fails() {
		commons_tests::db::TestDb::run(async |mut conn, _url| {
			let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
			let url = format!("http://{}", listener.local_addr().unwrap());
			let named = url.clone();
			let router = Router::new()
				.route(
					oidc::DISCOVERY_PATH,
					get(async move || {
						Json(serde_json::json!({
							"issuer": named,
							"jwks_uri": oidc::jwks_uri(&named),
						}))
					}),
				)
				.route(
					oidc::JWKS_PATH,
					get(async || Json(serde_json::json!({ "keys": [] }))),
				);
			tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

			let issuer = Issuer {
				issuer: url,
				keyring: oidc::Keyring::unreadable(&["ref-a".into()]),
			};
			tick(&mut conn, &client(), &issuer, &mut Watch::default()).await;
			let alert = database::self_alerts::current(
				&mut conn,
				database::self_alerts::IDENTITY_DISCOVERY_REF,
			)
			.await
			.unwrap()
			.expect("the alert is raised");
			assert!(alert.active);
			assert!(alert.message.contains("cannot sign"), "{}", alert.message);
		})
		.await;
	}

	#[tokio::test]
	async fn a_redirect_is_not_followed() {
		let issuer = serve(Router::new().route(
			oidc::DISCOVERY_PATH,
			get(async || Redirect::temporary("http://169.254.169.254/")),
		))
		.await;
		assert!(matches!(
			fetch(&client(), &issuer).await,
			IssuerDocuments::Unreachable(_)
		));
	}
}
