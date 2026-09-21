//! The transport's security properties, over a real QUIC connection on
//! loopback.
//!
//! These are the tests that matter most in this crate. With no CA and no chain,
//! the whole gate is: the peer proves possession of a key, and each end checks
//! the key it cares about. A regression here would not fail loudly — it would
//! quietly accept the wrong peer.

use std::net::{Ipv4Addr, SocketAddr};

use relay_protocol::{
	Filing, HarvestFiling, Instance, ProtocolVersion,
	frame::{read_required_frame, write_frame},
	transport::{Identity, client_config, negotiated_version, peer_spki, server_config},
};

/// A fresh device key, as canopy mints one at provisioning: a P-256 keypair
/// serialised as PKCS#8 PEM.
fn device_key_pem() -> String {
	rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256)
		.expect("generate a P-256 keypair")
		.serialize_pem()
}

fn identity() -> Identity {
	Identity::from_pkcs8_pem(&device_key_pem()).expect("a minted key builds an identity")
}

/// Canopy's listener on an ephemeral loopback port.
fn listener(canopy: &Identity) -> (quinn::Endpoint, SocketAddr) {
	let endpoint = quinn::Endpoint::server(
		server_config(canopy).expect("server config"),
		SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
	)
	.expect("bind the listener");
	let addr = endpoint.local_addr().expect("bound address");
	(endpoint, addr)
}

/// A relay's endpoint, dialling with `pin` as the canopy key it expects.
fn dialler(relay: &Identity, pin: Vec<u8>) -> quinn::Endpoint {
	let mut endpoint = quinn::Endpoint::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
		.expect("bind the dialler");
	endpoint.set_default_client_config(client_config(relay, pin).expect("client config"));
	endpoint
}

/// A client endpoint assembled by hand, so a test can vary exactly one thing
/// about it. `key_pem` absent means it presents no certificate at all; `alpn`
/// is what it offers. The server pin is stood down here because these tests
/// are about what canopy accepts, not about what the relay checks.
fn client_offering(key_pem: Option<&str>, alpn: Vec<Vec<u8>>) -> quinn::Endpoint {
	let provider = std::sync::Arc::new(rustls::crypto::aws_lc_rs::default_provider());
	let builder = rustls::ClientConfig::builder_with_provider(provider)
		.with_protocol_versions(&[&rustls::version::TLS13])
		.unwrap()
		.dangerous()
		.with_custom_certificate_verifier(std::sync::Arc::new(AcceptAnyServer));

	let mut tls = match key_pem {
		None => builder.with_no_client_auth(),
		Some(pem) => {
			// One keypair for both the certificate and the private key: two
			// would be a key mismatch, which rustls refuses outright.
			let key = rcgen::KeyPair::from_pem(pem).expect("a usable key");
			let certificate = rcgen::CertificateParams::default()
				.self_signed(&key)
				.expect("self-sign");
			builder
				.with_client_auth_cert(
					vec![certificate.der().clone()],
					rustls::pki_types::PrivateKeyDer::try_from(key.serialize_der()).unwrap(),
				)
				.expect("client auth cert")
		}
	};
	tls.alpn_protocols = alpn;

	let mut endpoint =
		quinn::Endpoint::client(SocketAddr::from((Ipv4Addr::LOCALHOST, 0))).expect("bind");
	endpoint.set_default_client_config(quinn::ClientConfig::new(std::sync::Arc::new(
		quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap(),
	)));
	endpoint
}

/// The device key canopy stores at provisioning is derived by self-signing and
/// reading `subject_pki.raw`. The relay's TLS stack presents a certificate
/// built from the same keypair, so the bytes must match — that equality is
/// what makes the lookup work at all, and it holds by construction rather
/// than by care.
#[tokio::test(flavor = "multi_thread")]
async fn the_spki_canopy_sees_is_the_spki_it_provisioned() {
	let key_pem = device_key_pem();
	let relay = Identity::from_pkcs8_pem(&key_pem).unwrap();
	let provisioned = relay.spki().to_vec();

	let canopy = identity();
	let (server, addr) = listener(&canopy);
	let client = dialler(&relay, canopy.spki().to_vec());

	let accepting = tokio::spawn(async move {
		let connection = server
			.accept()
			.await
			.expect("an incoming connection")
			.await
			.expect("the handshake completes");
		peer_spki(&connection).expect("the peer presented a certificate")
	});

	let connection = client
		.connect(addr, "canopy")
		.expect("dial")
		.await
		.expect("the handshake completes");
	assert_eq!(
		negotiated_version(&connection).unwrap(),
		ProtocolVersion::V1,
	);

	let seen = accepting.await.unwrap();
	assert_eq!(
		seen, provisioned,
		"canopy must see exactly the key it stored, or no relay can authenticate",
	);
}

/// A relay pinned to one canopy must refuse another. This is what stops an
/// endpoint a relay mistakes for canopy from telling it which image to run.
#[tokio::test(flavor = "multi_thread")]
async fn a_relay_refuses_a_canopy_it_did_not_pin() {
	let canopy = identity();
	let impostor = identity();
	let relay = identity();

	let (server, addr) = listener(&impostor);
	// The pin names the real canopy; the listener is somebody else.
	let client = dialler(&relay, canopy.spki().to_vec());

	// Drive the server side so the handshake actually proceeds to the point of
	// being rejected, rather than stalling.
	tokio::spawn(async move {
		if let Some(incoming) = server.accept().await {
			let _ = incoming.await;
		}
	});

	let outcome = client.connect(addr, "canopy").expect("dial").await;
	assert!(
		outcome.is_err(),
		"a relay that accepts an unpinned peer takes instructions from it",
	);
}

/// A client presenting no certificate has nothing for canopy to look up, so it
/// is refused at the handshake rather than reaching the device-key lookup.
#[tokio::test(flavor = "multi_thread")]
async fn a_peer_presenting_no_certificate_is_refused() {
	let canopy = identity();
	let (server, addr) = listener(&canopy);

	// A client configured as a relay's is, minus the client certificate, so
	// what differs is only the identity it presents.
	let client = client_offering(None, vec![relay_protocol::ALPN_V1.to_vec()]);

	let accepting = tokio::spawn(async move {
		match server.accept().await {
			Some(incoming) => incoming.await.is_ok(),
			None => false,
		}
	});

	let dialled = client.connect(addr, "canopy").expect("dial").await;
	let accepted = accepting.await.unwrap();
	assert!(
		dialled.is_err() || !accepted,
		"an anonymous peer must not reach canopy's device-key lookup",
	);
}

/// An incompatible pair fails at the handshake with no application protocol,
/// rather than connecting and then failing to parse a message.
#[tokio::test(flavor = "multi_thread")]
async fn a_relay_speaking_another_protocol_never_connects() {
	let canopy = identity();
	let (server, addr) = listener(&canopy);

	// Identical to a relay's configuration in every respect except the ALPN
	// token, so what is under test is the version and nothing else.
	let client = client_offering(Some(&device_key_pem()), vec![b"canopy-relay/99".to_vec()]);

	tokio::spawn(async move {
		if let Some(incoming) = server.accept().await {
			let _ = incoming.await;
		}
	});

	assert!(
		client.connect(addr, "canopy").expect("dial").await.is_err(),
		"a version canopy does not speak must fail the handshake, not the parse",
	);
}

/// The whole shape of a filing: the relay opens a unidirectional stream,
/// writes one frame, finishes. Canopy reads it off the accepted stream.
#[tokio::test(flavor = "multi_thread")]
async fn a_filing_crosses_a_unidirectional_stream() {
	let canopy = identity();
	let relay = identity();
	let (server, addr) = listener(&canopy);
	let client = dialler(&relay, canopy.spki().to_vec());

	let accepting = tokio::spawn(async move {
		let connection = server.accept().await.unwrap().await.unwrap();
		let mut stream = connection.accept_uni().await.expect("a filing stream");
		read_required_frame::<_, Filing>(&mut stream)
			.await
			.expect("one filing frame")
	});

	let connection = client.connect(addr, "canopy").unwrap().await.unwrap();
	let mut stream = connection.open_uni().await.expect("open a filing stream");
	write_frame(
		&mut stream,
		&Filing::Harvest(HarvestFiling {
			namespace: "nauru-demo".into(),
			instance: Instance::Central,
			push: serde_json::json!({"source": "alertd", "health": []}),
		}),
	)
	.await
	.expect("write the filing");
	stream.finish().expect("finish the stream");

	let filing = accepting.await.unwrap();
	let Filing::Harvest(harvest) = filing else {
		panic!("the family changed in flight");
	};
	assert_eq!(harvest.namespace, "nauru-demo");
	assert_eq!(harvest.instance, Instance::Central);
}

/// A verifier for the test clients above that are exercising something other
/// than the pin. Signature verification stays real, so these still prove
/// possession — only the identity check is stood down.
#[derive(Debug)]
struct AcceptAnyServer;

impl rustls::client::danger::ServerCertVerifier for AcceptAnyServer {
	fn verify_server_cert(
		&self,
		_end_entity: &rustls::pki_types::CertificateDer<'_>,
		_intermediates: &[rustls::pki_types::CertificateDer<'_>],
		_server_name: &rustls::pki_types::ServerName<'_>,
		_ocsp_response: &[u8],
		_now: rustls::pki_types::UnixTime,
	) -> Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
		Ok(rustls::client::danger::ServerCertVerified::assertion())
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &rustls::pki_types::CertificateDer<'_>,
		dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls12_signature(
			message,
			cert,
			dss,
			&rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &rustls::pki_types::CertificateDer<'_>,
		dss: &rustls::DigitallySignedStruct,
	) -> Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
		rustls::crypto::verify_tls13_signature(
			message,
			cert,
			dss,
			&rustls::crypto::aws_lc_rs::default_provider().signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
		rustls::crypto::aws_lc_rs::default_provider()
			.signature_verification_algorithms
			.supported_schemes()
	}
}
