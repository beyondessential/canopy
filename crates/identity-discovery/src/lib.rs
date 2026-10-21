//! The identity provider's discovery component (IDP, "Discovery"): serves the
//! discovery document and the key set at the issuer, and nothing else.
//!
//! It is unauthenticated, holds no access to Canopy's database and no ability
//! to sign, and builds the key set from the public halves of the configured
//! keys, so the only internet-facing part of the identity provider holds
//! nothing secret.
// spec: IDP#discovery

use std::{sync::Arc, time::Duration};

use axum::{Json, Router, extract::State, http::StatusCode, response::IntoResponse, routing::get};
use oidc::{Issuer, Jwks};
use serde_json::Value;
use tokio::sync::RwLock;

/// How often the key set is rebuilt, so a key whose public half could not be
/// read at startup is published once it can be.
pub const REFRESH: Duration = Duration::from_secs(60);

/// How long startup waits for the key service before serving without the keys
/// it could not read, which the refresh publishes once it can.
pub const STARTUP_WAIT: Duration = Duration::from_secs(10);

#[derive(Clone)]
struct Served {
	discovery: Arc<Value>,
	jwks: Arc<RwLock<Jwks>>,
}

/// The configured keys' public halves, oldest first, leaving out any that
/// cannot be read yet. A key once read stays in the set however the key service
/// fares afterwards, since the keyring keeps it.
pub async fn jwks(issuer: &Issuer) -> Jwks {
	Jwks {
		keys: issuer
			.keyring
			.keys()
			.await
			.into_iter()
			.filter_map(|(_, key)| Some(key?.jwk))
			.collect(),
	}
}

/// The path the issuer's own URL carries, beneath which the documents are
/// served: empty for an issuer at the root of its name.
fn issuer_path(issuer: &str) -> &str {
	let rest = issuer.strip_prefix("https://").unwrap_or(issuer);
	rest.find('/').map(|at| &rest[at..]).unwrap_or("")
}

/// The router serving the two documents, with a task keeping the key set
/// current. Waits for the key set no longer than [`STARTUP_WAIT`], so a key
/// service that is slow to answer does not hold back the discovery document.
pub async fn router(issuer: Issuer) -> Router {
	let initial = tokio::time::timeout(STARTUP_WAIT, jwks(&issuer))
		.await
		.unwrap_or_else(|_| {
			tracing::warn!("the key service did not answer in time; serving no keys until it does");
			Jwks::default()
		});
	let served = Served {
		discovery: Arc::new(oidc::discovery_document(&issuer.issuer)),
		jwks: Arc::new(RwLock::new(initial)),
	};

	let refreshed = served.jwks.clone();
	let refreshing = issuer.clone();
	tokio::spawn(async move {
		loop {
			tokio::time::sleep(REFRESH).await;
			let next = jwks(&refreshing).await;
			*refreshed.write().await = next;
		}
	});

	let base = issuer_path(&issuer.issuer);
	Router::new()
		.route(
			&format!("{base}{}", oidc::DISCOVERY_PATH),
			get(async |State(served): State<Served>| Json((*served.discovery).clone())),
		)
		.route(
			&format!("{base}{}", oidc::JWKS_PATH),
			get(async |State(served): State<Served>| {
				let jwks = served.jwks.read().await.clone();
				// While no key has been read, an empty set would be cached by
				// relying parties as Canopy having no keys; unavailable keeps
				// them on what they hold.
				if jwks.keys.is_empty() {
					return StatusCode::SERVICE_UNAVAILABLE.into_response();
				}
				Json(jwks).into_response()
			}),
		)
		.with_state(served)
}

#[cfg(test)]
mod tests {
	use axum_test::TestServer;
	use oidc::{Keyring, SigningKey};

	use super::*;

	fn issuer(url: &str) -> (Issuer, SigningKey) {
		let key = SigningKey::generate_local("k");
		(
			Issuer {
				issuer: url.into(),
				keyring: Keyring::local(vec![key.clone()]),
			},
			key,
		)
	}

	#[tokio::test]
	async fn serves_the_documents_at_the_issuer() {
		let (issuer, key) = issuer("https://identity.example");
		let server = TestServer::new(router(issuer).await);

		let discovery: Value = server.get("/.well-known/openid-configuration").await.json();
		assert_eq!(discovery["issuer"], "https://identity.example");
		assert_eq!(
			discovery["jwks_uri"],
			"https://identity.example/.well-known/jwks.json"
		);
		assert_eq!(
			discovery["id_token_signing_alg_values_supported"][0],
			"ES256"
		);

		let jwks: Jwks = server.get("/.well-known/jwks.json").await.json();
		assert_eq!(jwks.keys, [key.jwk]);
	}

	#[tokio::test]
	async fn serves_beneath_an_issuer_path() {
		let (issuer, _) = issuer("https://identity.example/canopy");
		let server = TestServer::new(router(issuer).await);
		server
			.get("/canopy/.well-known/openid-configuration")
			.await
			.assert_status_ok();
		server
			.get("/.well-known/openid-configuration")
			.await
			.assert_status_not_found();
	}

	#[tokio::test]
	async fn with_no_key_read_the_key_set_is_unavailable() {
		let issuer = Issuer {
			issuer: "https://identity.example".into(),
			keyring: Keyring::unreadable(&["k".into()]),
		};
		let server = TestServer::new(router(issuer).await);
		server
			.get("/.well-known/jwks.json")
			.await
			.assert_status(axum::http::StatusCode::SERVICE_UNAVAILABLE);
		server
			.get("/.well-known/openid-configuration")
			.await
			.assert_status_ok();
	}

	#[tokio::test]
	async fn answers_nothing_else() {
		let (issuer, _) = issuer("https://identity.example");
		let server = TestServer::new(router(issuer).await);
		for path in ["/", "/health", "/identity-tokens", "/.well-known/other"] {
			server.get(path).await.assert_status_not_found();
		}
	}
}
