//! Canopy's end of a relay connection, against a real database and a real
//! QUIC connection.
//!
//! What these cover is the gate: an enrolled relay gets in, and a device key
//! that is unknown, deactivated, or attached to another role does not. With no
//! CA and no chain, that lookup is the whole of canopy's trust in a cluster, so
//! it is worth exercising against the actual store rather than a stand-in.

use std::net::{Ipv4Addr, SocketAddr};
use std::time::Duration;

use commons_servers::device_auth::keygen::{GeneratedDeviceKey, generate_device_key};
use commons_types::device::DeviceRole;
use database::devices::Device;
use jobs::relay::{self, Registry};
use relay_protocol::{
	Filing, HarvestFiling, Hello, Instance, Request, Response,
	frame::{read_required_frame, write_frame},
	transport::{Identity, client_config},
};

/// Mint a device credential the way an operator's provisioning does, so these
/// tests exercise the real minting rather than a stand-in: canopy keeps
/// `spki_der` and hands the private key over once.
///
/// That this key then authenticates over QUIC is itself the assertion —
/// provisioning derives the stored bytes by self-signing and reading
/// `subject_pki.raw`, which is what the relay's TLS stack does, so the two
/// match by construction.
fn provision() -> GeneratedDeviceKey {
	generate_device_key().expect("mint a device key")
}

/// Stand up canopy's listener on loopback, returning its address, its pinned
/// public key, and the registry it fills.
async fn hub(db: database::Db) -> (SocketAddr, Vec<u8>, Registry) {
	let canopy = Identity::from_pkcs8_pem(&provision().private_key_pem).unwrap();
	let pin = canopy.spki().to_vec();

	let endpoint = relay::endpoint(&canopy, SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
		.expect("listen for relays");
	let addr = endpoint.local_addr().unwrap();

	let registry = Registry::new();
	tokio::spawn(relay::listen(db, registry.clone(), endpoint));
	(addr, pin, registry)
}

/// A relay that answers what it is running, which is what canopy asks first.
/// Returns the connection so a test can keep filing on it.
async fn connect_relay(
	key_pem: &str,
	addr: SocketAddr,
	pin: Vec<u8>,
	build: Hello,
) -> Result<quinn::Connection, quinn::ConnectionError> {
	let identity = Identity::from_pkcs8_pem(key_pem).unwrap();
	let mut endpoint = quinn::Endpoint::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).unwrap();
	endpoint.set_default_client_config(client_config(&identity, pin).unwrap());

	let connection = endpoint.connect(addr, "canopy").unwrap().await?;

	// Answer canopy's opening question in the background, for as long as it
	// keeps asking.
	let answering = connection.clone();
	tokio::spawn(async move {
		while let Ok((mut send, mut recv)) = answering.accept_bi().await {
			let Ok(request) = read_required_frame::<_, Request>(&mut recv).await else {
				break;
			};
			let response = match request {
				Request::Build => Response::Build(build.clone()),
				Request::Ping => Response::Pong,
				_ => Response::Failed {
					message: "not under test".into(),
				},
			};
			let _ = write_frame(&mut send, &response).await;
			let _ = send.finish();
		}
	});

	// Keep the endpoint alive for the connection's lifetime: dropping it
	// closes everything on it.
	std::mem::forget(endpoint);
	Ok(connection)
}

fn build() -> Hello {
	Hello {
		suite_version: "2.30.1".into(),
		relay_version: "1.2.3".into(),
		version_floor: "1.0.0".into(),
	}
}

/// Wait for a condition the listener reaches asynchronously, rather than
/// sleeping a fixed time and hoping.
async fn eventually<F, Fut>(what: &str, mut check: F) -> bool
where
	F: FnMut() -> Fut,
	Fut: Future<Output = bool>,
{
	for _ in 0..100 {
		if check().await {
			return true;
		}
		tokio::time::sleep(Duration::from_millis(20)).await;
	}
	eprintln!("timed out waiting for {what}");
	false
}

/// The happy path: a device enrolled at the relay role connects, canopy asks
/// what it is running, and the connection lands in the registry keyed by that
/// device.
#[tokio::test(flavor = "multi_thread")]
async fn an_enrolled_relay_connects_and_is_registered() {
	commons_tests::db::TestDb::run(|mut conn, url| async move {
		let relay_key = provision();
		let device = Device::create_at_role(
			&mut conn,
			relay_key.spki_der.clone(),
			DeviceRole::Relay,
			Some("relay".into()),
		)
		.await
		.expect("enrol the relay device");

		let (addr, pin, registry) = hub(database::init_to(&url)).await;
		let _connection = connect_relay(&relay_key.private_key_pem, addr, pin, build())
			.await
			.expect("an enrolled relay connects");

		assert!(
			eventually("the relay to be registered", || {
				let registry = registry.clone();
				async move { registry.get(device.id).await.is_some() }
			})
			.await,
		);

		let connected = registry.get(device.id).await.unwrap();
		assert_eq!(
			connected.build.relay_version, "1.2.3",
			"canopy records what the relay answered it is running",
		);
		assert_eq!(
			registry.connected().await.len(),
			1,
			"one relay, one connection held",
		);

		// The registry is what answers "connected and answering", so ask
		// through it rather than on the connection.
		assert_eq!(
			registry.request(device.id, Request::Ping).await.unwrap(),
			Response::Pong,
		);
	})
	.await;
}

/// A key canopy has never seen authenticates nothing. The handshake itself
/// succeeds — canopy accepts any certificate, deliberately — so what refuses
/// this is the device-key lookup, and the connection must not survive it.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_canopy_never_enrolled_is_refused() {
	commons_tests::db::TestDb::run(|conn, url| async move {
		drop(conn);
		let stranger = provision();

		let (addr, pin, registry) = hub(database::init_to(&url)).await;
		let connection = connect_relay(&stranger.private_key_pem, addr, pin, build())
			.await
			.expect("the handshake completes; the lookup is what refuses");

		assert!(
			eventually("the connection to be closed", || {
				let connection = connection.clone();
				async move { connection.close_reason().is_some() }
			})
			.await,
			"an unenrolled key must not hold a connection",
		);
		assert!(registry.connected().await.is_empty());
	})
	.await;
}

/// Revocation is the existing path: deactivate the key, and it stops
/// resolving. A relay holding a deactivated key is a stranger.
#[tokio::test(flavor = "multi_thread")]
async fn a_deactivated_key_stops_authenticating() {
	commons_tests::db::TestDb::run(|mut conn, url| async move {
		let relay_key = provision();
		let device = Device::create_at_role(
			&mut conn,
			relay_key.spki_der.clone(),
			DeviceRole::Relay,
			Some("relay".into()),
		)
		.await
		.expect("enrol the relay device");
		Device::deactivate_keys(&mut conn, device.id)
			.await
			.expect("revoke the key");

		let (addr, pin, registry) = hub(database::init_to(&url)).await;
		let connection = connect_relay(&relay_key.private_key_pem, addr, pin, build())
			.await
			.expect("the handshake completes");

		assert!(
			eventually("the connection to be closed", || {
				let connection = connection.clone();
				async move { connection.close_reason().is_some() }
			})
			.await,
			"a revoked key must not hold a connection",
		);
		assert!(registry.connected().await.is_empty());
	})
	.await;
}

/// The role is load-bearing, not decoration: a device enrolled as a server
/// holds a perfectly good device key, and must still not be able to file for a
/// whole cluster.
#[tokio::test(flavor = "multi_thread")]
async fn a_device_at_another_role_is_refused() {
	commons_tests::db::TestDb::run(|mut conn, url| async move {
		let server_key = provision();
		Device::create_at_role(
			&mut conn,
			server_key.spki_der.clone(),
			DeviceRole::Machine,
			Some("a server, not a relay".into()),
		)
		.await
		.expect("enrol a server device");

		let (addr, pin, registry) = hub(database::init_to(&url)).await;
		let connection = connect_relay(&server_key.private_key_pem, addr, pin, build())
			.await
			.expect("the handshake completes");

		assert!(
			eventually("the connection to be closed", || {
				let connection = connection.clone();
				async move { connection.close_reason().is_some() }
			})
			.await,
			"a server device must not be able to act as a relay",
		);
		assert!(registry.connected().await.is_empty());
	})
	.await;
}

/// A filing whose coordinates canopy cannot place is dropped with a warning,
/// and does not take the connection down with it: here, a relay whose identity
/// names no registered cluster.
#[tokio::test(flavor = "multi_thread")]
async fn an_unplaceable_filing_does_not_break_the_connection() {
	commons_tests::db::TestDb::run(|mut conn, url| async move {
		let relay_key = provision();
		let device = Device::create_at_role(
			&mut conn,
			relay_key.spki_der.clone(),
			DeviceRole::Relay,
			Some("relay".into()),
		)
		.await
		.expect("enrol the relay device");

		let (addr, pin, registry) = hub(database::init_to(&url)).await;
		let connection = connect_relay(&relay_key.private_key_pem, addr, pin, build())
			.await
			.expect("connect");

		assert!(
			eventually("the relay to be registered", || {
				let registry = registry.clone();
				async move { registry.get(device.id).await.is_some() }
			})
			.await,
		);

		let mut stream = connection.open_uni().await.expect("a filing stream");
		write_frame(
			&mut stream,
			&Filing::Harvest(HarvestFiling {
				namespace: "nowhere-canopy-knows".into(),
				instance: Instance::Central,
				push: serde_json::json!({"source": "alertd", "health": []}),
			}),
		)
		.await
		.expect("write the filing");
		stream.finish().expect("finish");

		// The connection must still be good afterwards, and still answering.
		assert_eq!(
			registry.request(device.id, Request::Ping).await.unwrap(),
			Response::Pong,
			"a filing canopy cannot place must not cost the connection",
		);
	})
	.await;
}
