//! The QUIC transport and the identity it carries.
//!
//! Both ends configure TLS from here. Duplicating this on the two sides would
//! be the same drift risk as duplicating the message enum, and here it would
//! be a drift in what authenticates a relay.
//!
//! ## What authenticates what
//!
//! A relay presents a **client certificate carrying its device key**. Canopy
//! reads the certificate's `SubjectPublicKeyInfo`, looks it up in
//! `device_keys`, and checks the device carries the relay role — the same SPKI
//! lookup the HTTP mTLS path performs, against the same column, so one store
//! answers for both paths.
//!
//! There is no CA and no chain in either direction. What makes that sound is
//! that the certificate is not what is trusted: the *key* is, and it is a
//! first-class record with a provisioning workflow behind it.
//!
//! **The handshake is proof of possession.** Canopy terminates TLS itself, so
//! the peer must sign with the private key to complete the handshake. This is
//! stronger than the HTTP mTLS path, where TLS terminates at an ingress proxy
//! and authentication rests on a header carrying a certificate that is not
//! secret. That property lives or dies by the signature callbacks on the
//! verifiers below being real verification: a verifier that waves those
//! through would accept anyone presenting an enrolled device's public
//! certificate, which is exactly the weakness this path does not have.
//!
//! Canopy is verified the same way, in the other direction: the relay pins
//! canopy's public key. Symmetric, one verification path whether or not an
//! overlay network is in the way, and nothing that becomes unsafe if a
//! deployment later drops the overlay — which matters because a peer a relay
//! mistakes for canopy could tell it which image to run.

use std::sync::Arc;

use quinn::{
	ClientConfig, ServerConfig,
	crypto::rustls::{QuicClientConfig, QuicServerConfig},
};
use rustls::{
	DigitallySignedStruct, DistinguishedName, SignatureScheme,
	client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
	crypto::{CryptoProvider, aws_lc_rs, verify_tls12_signature, verify_tls13_signature},
	pki_types::{CertificateDer, PrivateKeyDer, ServerName, UnixTime},
	server::danger::{ClientCertVerified, ClientCertVerifier},
};
use x509_parser::prelude::*;

use crate::alpn::{ProtocolVersion, UnknownProtocol, alpn_protocols};

#[derive(Debug, thiserror::Error)]
pub enum TransportError {
	#[error("relay identity key is not usable: {0}")]
	Key(String),

	#[error("relay identity certificate could not be built: {0}")]
	Certificate(String),

	#[error("TLS configuration failed: {0}")]
	Tls(#[from] rustls::Error),

	#[error("QUIC does not accept this TLS configuration: {0}")]
	Quic(String),

	#[error("the peer presented no certificate")]
	NoPeerCertificate,

	#[error("the peer's certificate could not be parsed: {0}")]
	UnparseablePeerCertificate(String),

	#[error("the connection negotiated no application protocol")]
	NoProtocol,

	#[error(transparent)]
	UnknownProtocol(#[from] UnknownProtocol),
}

/// An end's own identity: the keypair it holds and the throwaway certificate
/// that presents it.
///
/// The certificate is self-signed and means nothing on its own. Its whole
/// purpose is to carry the public key into the handshake, where possession of
/// the private key is proven.
pub struct Identity {
	certificate: CertificateDer<'static>,
	key: PrivateKeyDer<'static>,
	spki: Vec<u8>,
}

impl std::fmt::Debug for Identity {
	/// Hand-written so the private key cannot reach a log through a derived
	/// `Debug`.
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Identity")
			.field("spki", &hex(&self.spki))
			.finish_non_exhaustive()
	}
}

impl Identity {
	/// Build an identity from a PKCS#8 PEM private key — for a relay, the key
	/// canopy minted for its device at provisioning and handed over once.
	///
	/// The certificate is generated here from that key, so the SPKI presented
	/// in the handshake is derived from the same keypair canopy stored at
	/// provisioning. The two match by construction rather than by care: canopy
	/// derives the stored bytes the same way, by self-signing and reading
	/// `subject_pki.raw`.
	pub fn from_pkcs8_pem(key_pem: &str) -> Result<Self, TransportError> {
		let key = rcgen::KeyPair::from_pem(key_pem)
			.map_err(|e| TransportError::Key(format!("not a usable PKCS#8 PEM key: {e}")))?;

		let mut params = rcgen::CertificateParams::default();
		params.is_ca = rcgen::IsCa::NoCa;
		params.key_usages = vec![rcgen::KeyUsagePurpose::DigitalSignature];
		// Both purposes, because one function serves both ends: a relay is the
		// client and canopy is the server, and the certificate is otherwise
		// identical.
		params.extended_key_usages = vec![
			rcgen::ExtendedKeyUsagePurpose::ClientAuth,
			rcgen::ExtendedKeyUsagePurpose::ServerAuth,
		];
		params.distinguished_name = rcgen::DistinguishedName::new();

		let certificate = params
			.self_signed(&key)
			.map_err(|e| TransportError::Certificate(e.to_string()))?;
		let certificate_der = certificate.der().clone();
		let spki = spki_of(&certificate_der)?;

		Ok(Self {
			certificate: certificate_der,
			key: PrivateKeyDer::try_from(key.serialize_der())
				.map_err(|e| TransportError::Key(e.to_string()))?,
			spki,
		})
	}

	/// This identity's `SubjectPublicKeyInfo`, as stored in `device_keys` and
	/// as the other end pins.
	pub fn spki(&self) -> &[u8] {
		&self.spki
	}
}

/// The `SubjectPublicKeyInfo` of a certificate, derived exactly as the HTTP
/// mTLS path derives it, so one lookup answers for both paths.
pub fn spki_of(certificate: &CertificateDer<'_>) -> Result<Vec<u8>, TransportError> {
	let (_, parsed) = parse_x509_certificate(certificate.as_ref())
		.map_err(|e| TransportError::UnparseablePeerCertificate(e.to_string()))?;
	Ok(parsed.tbs_certificate.subject_pki.raw.to_vec())
}

/// The SPKI the peer authenticated with on an established connection.
///
/// On canopy's side this is what the device-key lookup keys on, and it comes
/// from the connection rather than from anything the peer said in a message.
pub fn peer_spki(connection: &quinn::Connection) -> Result<Vec<u8>, TransportError> {
	let identity = connection
		.peer_identity()
		.ok_or(TransportError::NoPeerCertificate)?;
	let chain = identity
		.downcast::<Vec<CertificateDer<'static>>>()
		.map_err(|_| TransportError::NoPeerCertificate)?;
	let end_entity = chain.first().ok_or(TransportError::NoPeerCertificate)?;
	spki_of(end_entity)
}

/// The protocol version the handshake settled on.
///
/// Read from the connection, never from a message: the version two ends agreed
/// to speak is not something either gets to restate afterwards.
pub fn negotiated_version(
	connection: &quinn::Connection,
) -> Result<ProtocolVersion, TransportError> {
	let handshake = connection
		.handshake_data()
		.ok_or(TransportError::NoProtocol)?;
	let handshake = handshake
		.downcast::<quinn::crypto::rustls::HandshakeData>()
		.map_err(|_| TransportError::NoProtocol)?;
	let token = handshake.protocol.ok_or(TransportError::NoProtocol)?;
	Ok(ProtocolVersion::from_alpn(&token)?)
}

/// Canopy's listener configuration: present `identity`, require a client
/// certificate, and accept whatever certificate arrives.
///
/// Accepting any certificate is deliberate and is not the gate. There is no CA
/// to validate a chain against; the gate is the device-key lookup canopy
/// performs on the presented SPKI once the handshake completes. Requiring a
/// certificate matters, though — a peer presenting none is refused here rather
/// than reaching a lookup with nothing to look up.
pub fn server_config(identity: &Identity) -> Result<ServerConfig, TransportError> {
	let provider = Arc::new(aws_lc_rs::default_provider());
	let mut tls = rustls::ServerConfig::builder_with_provider(provider.clone())
		.with_protocol_versions(&[&rustls::version::TLS13])?
		.with_client_cert_verifier(Arc::new(AnyClientCertificate::new(provider)))
		.with_single_cert(vec![identity.certificate.clone()], identity.key.clone_key())?;
	tls.alpn_protocols = alpn_protocols();

	let quic = QuicServerConfig::try_from(tls).map_err(|e| TransportError::Quic(e.to_string()))?;
	Ok(ServerConfig::with_crypto(Arc::new(quic)))
}

/// A relay's dialling configuration: present `identity`, and accept only the
/// canopy whose public key is pinned.
pub fn client_config(
	identity: &Identity,
	canopy_spki: Vec<u8>,
) -> Result<ClientConfig, TransportError> {
	let provider = Arc::new(aws_lc_rs::default_provider());
	let mut tls = rustls::ClientConfig::builder_with_provider(provider.clone())
		.with_protocol_versions(&[&rustls::version::TLS13])?
		.dangerous()
		.with_custom_certificate_verifier(Arc::new(PinnedCanopy::new(provider, canopy_spki)))
		.with_client_auth_cert(vec![identity.certificate.clone()], identity.key.clone_key())?;
	tls.alpn_protocols = alpn_protocols();

	let quic = QuicClientConfig::try_from(tls).map_err(|e| TransportError::Quic(e.to_string()))?;
	Ok(ClientConfig::new(Arc::new(quic)))
}

/// Canopy's client-certificate verifier: accepts any certificate, verifies
/// every signature.
///
/// The asymmetry is the point. Chain validation is skipped because there is no
/// chain to validate and the device-key lookup replaces it. Signature
/// verification is **not** skipped, because it is what makes the handshake
/// proof of possession — without it, presenting an enrolled device's public
/// certificate would be enough to be authenticated as that device.
#[derive(Debug)]
struct AnyClientCertificate {
	provider: Arc<CryptoProvider>,
}

impl AnyClientCertificate {
	fn new(provider: Arc<CryptoProvider>) -> Self {
		Self { provider }
	}
}

impl ClientCertVerifier for AnyClientCertificate {
	fn root_hint_subjects(&self) -> &[DistinguishedName] {
		// No CA, so no hint to offer: a relay's certificate is self-signed and
		// there is nothing for it to choose between.
		&[]
	}

	fn verify_client_cert(
		&self,
		_end_entity: &CertificateDer<'_>,
		_intermediates: &[CertificateDer<'_>],
		_now: UnixTime,
	) -> Result<ClientCertVerified, rustls::Error> {
		Ok(ClientCertVerified::assertion())
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		verify_tls12_signature(
			message,
			cert,
			dss,
			&self.provider.signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		verify_tls13_signature(
			message,
			cert,
			dss,
			&self.provider.signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		self.provider
			.signature_verification_algorithms
			.supported_schemes()
	}
}

/// A relay's server-certificate verifier: the presented certificate must carry
/// the canopy public key this relay was configured with.
///
/// No CA, no chain, no hostname. The pin is the whole check, and it is checked
/// on every transport — over an overlay network or not — because a relay that
/// accepted an unverified peer would take instructions from it.
#[derive(Debug)]
struct PinnedCanopy {
	provider: Arc<CryptoProvider>,
	canopy_spki: Vec<u8>,
}

impl PinnedCanopy {
	fn new(provider: Arc<CryptoProvider>, canopy_spki: Vec<u8>) -> Self {
		Self {
			provider,
			canopy_spki,
		}
	}
}

impl ServerCertVerifier for PinnedCanopy {
	fn verify_server_cert(
		&self,
		end_entity: &CertificateDer<'_>,
		_intermediates: &[CertificateDer<'_>],
		_server_name: &ServerName<'_>,
		_ocsp_response: &[u8],
		_now: UnixTime,
	) -> Result<ServerCertVerified, rustls::Error> {
		let presented = spki_of(end_entity).map_err(|e| {
			rustls::Error::General(format!("canopy's certificate could not be read: {e}"))
		})?;

		// Constant-time comparison is not needed — both values are public keys
		// — but the failure has to be flat refusal rather than anything a peer
		// can learn from.
		if presented == self.canopy_spki {
			Ok(ServerCertVerified::assertion())
		} else {
			Err(rustls::Error::General(format!(
				"peer is not the pinned canopy: presented {}, expected {}",
				hex(&presented),
				hex(&self.canopy_spki),
			)))
		}
	}

	fn verify_tls12_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		verify_tls12_signature(
			message,
			cert,
			dss,
			&self.provider.signature_verification_algorithms,
		)
	}

	fn verify_tls13_signature(
		&self,
		message: &[u8],
		cert: &CertificateDer<'_>,
		dss: &DigitallySignedStruct,
	) -> Result<HandshakeSignatureValid, rustls::Error> {
		verify_tls13_signature(
			message,
			cert,
			dss,
			&self.provider.signature_verification_algorithms,
		)
	}

	fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
		self.provider
			.signature_verification_algorithms
			.supported_schemes()
	}
}

/// Lowercase hex, for naming a key in an error or a log line.
pub fn hex(bytes: &[u8]) -> String {
	use std::fmt::Write as _;
	let mut s = String::with_capacity(bytes.len() * 2);
	for b in bytes {
		let _ = write!(s, "{b:02x}");
	}
	s
}
