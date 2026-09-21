//! Both ends of the card, together: the real relay binary's client loop
//! against canopy's real listener, over a real QUIC connection, with the
//! device key in a real database.
//!
//! Everything else in this card's tests exercises one side. This is the one
//! that would catch the two sides disagreeing — a relay that files in a shape
//! canopy will not read, or a canopy whose opening question the relay does not
//! answer.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use commons_servers::device_auth::keygen::generate_device_key;
use commons_types::device::DeviceRole;
use database::devices::Device;
use jobs::relay::{self as hub, Registry};
use relay::duties::Unattached;
use relay_protocol::{
	Filing, FilingTarget, Hello, Instance, Request, Response, SubstrateFiling, transport::Identity,
};

/// Wait for something the two ends reach asynchronously.
async fn eventually<F, Fut>(what: &str, mut check: F) -> bool
where
	F: FnMut() -> Fut,
	Fut: Future<Output = bool>,
{
	for _ in 0..150 {
		if check().await {
			return true;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	eprintln!("timed out waiting for {what}");
	false
}

/// A relay enrolled in canopy, connected to canopy's listener by the real
/// client loop. Everything is what ships: canopy's `listen`, the relay's
/// `run`, and the device key minted by the provisioning path.
#[tokio::test(flavor = "multi_thread")]
async fn a_real_relay_connects_to_the_real_listener_and_answers_it() {
	commons_tests::db::TestDb::run(|mut conn, url| async move {
		// Provision a relay credential the way an operator does, and enrol the
		// device at the relay role.
		let credential = generate_device_key().expect("mint the relay's device key");
		let device = Device::create_at_role(
			&mut conn,
			credential.spki_der.clone(),
			DeviceRole::Relay,
			Some("the cluster's relay".into()),
		)
		.await
		.expect("enrol the relay device");

		// Canopy's own key: what the relay pins.
		let canopy_credential = generate_device_key().expect("mint canopy's transport key");
		let canopy_identity =
			Identity::from_pkcs8_pem(&canopy_credential.private_key_pem).expect("canopy identity");
		let canopy_spki = canopy_identity.spki().to_vec();

		let endpoint = hub::endpoint(&canopy_identity, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
			.expect("canopy listens");
		let canopy_addr = endpoint.local_addr().unwrap();
		let registry = Registry::new();
		tokio::spawn(hub::listen(
			database::init_to(&url),
			registry.clone(),
			endpoint,
		));

		// The relay's configuration, as a deployment supplies it: its own key
		// file, canopy's key in hex, and where canopy is.
		let key_file = std::env::temp_dir().join(format!("relay-key-{}.pem", device.id));
		std::fs::write(&key_file, &credential.private_key_pem).expect("write the key file");
		let config = relay::Config::load(
			&key_file,
			&relay_protocol::transport::hex(&canopy_spki),
			canopy_addr,
			"canopy".into(),
		)
		.expect("the relay's configuration is usable");

		let build = Hello {
			suite_version: "2.30.1".into(),
			relay_version: "9.9.9".into(),
			version_floor: config.floor.to_string(),
		};
		let (filings, filings_rx) = tokio::sync::mpsc::channel(8);
		tokio::spawn(relay::run(
			config,
			Arc::new(Unattached::new(build)),
			filings_rx,
		));

		// Canopy asks what the relay is running as soon as it has authenticated
		// it, so the registry entry appearing means both ends completed that
		// exchange.
		assert!(
			eventually("the relay to connect and be registered", || {
				let registry = registry.clone();
				async move { registry.get(device.id).await.is_some() }
			})
			.await,
			"the relay must connect, authenticate, and answer",
		);

		let connected = registry.get(device.id).await.unwrap();
		assert_eq!(
			connected.build.relay_version, "9.9.9",
			"canopy holds what the relay answered, not something it assumed",
		);

		// Canopy asking, answered by the relay's real dispatch.
		assert_eq!(
			registry.request(device.id, Request::Ping).await.unwrap(),
			Response::Pong,
		);

		// A namespace this relay does not serve: a refusal, and specifically a
		// refusal rather than a transport failure.
		let response = registry
			.request(
				device.id,
				Request::NamespaceRoster {
					namespace: "nauru-demo".into(),
				},
			)
			.await
			.expect("the exchange completes even when the relay declines");
		assert!(
			matches!(response, Response::Failed { .. }),
			"an unattached relay reports it cannot read its cluster, got {response:?}",
		);

		// A downgrade, refused by the relay's floor rather than by canopy.
		let response = registry
			.request(
				device.id,
				Request::RunVersion {
					version: "0.0.1".into(),
				},
			)
			.await
			.expect("the exchange completes");
		let Response::Refused(refusal) = response else {
			panic!("a downgrade must be refused by the relay, got {response:?}");
		};
		assert_eq!(
			refusal.kind,
			relay_protocol::RefusalKind::BelowVersionFloor,
			"the floor is the relay's, so the refusal comes from the relay",
		);

		// And the relay's own filing path: a filing written by the shipped
		// client loop, read by the shipped listener. Canopy cannot place it
		// yet, which must cost the filing and not the connection.
		filings
			.send(Filing::Substrate(SubstrateFiling {
				target: FilingTarget::Instance {
					namespace: "nauru-demo".into(),
					instance: Instance::Central,
				},
				check: "pod-unschedulable".into(),
				observed: commons_types::status::CheckResult::Failed,
				title: Some("A pod cannot be placed".into()),
				message: "no node has capacity".into(),
				detail: None,
				default_ceiling: commons_types::status::CheckResult::Failed,
				default_escalates: false,
				documentation: None,
			}))
			.await
			.expect("the relay accepts a filing to send");

		assert!(
			eventually("the connection to still be answering", || {
				let registry = registry.clone();
				async move {
					registry
						.request(device.id, Request::Ping)
						.await
						.is_ok_and(|r| r == Response::Pong)
				}
			})
			.await,
			"a filing must not cost the connection",
		);

		let _ = std::fs::remove_file(&key_file);
	})
	.await;
}
