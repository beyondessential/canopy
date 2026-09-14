use ::time::OffsetDateTime;
use axum_client_ip::ClientIpSource;
use axum_test::TestServer;
use std::collections::BTreeMap;
use std::sync::Mutex;

use commons_servers::artifact_store::ArtifactStore;
use commons_servers::device_auth::mtls::ClientCertHeader;
use commons_servers::router;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::AsyncPgConnection;
use diesel_async::RunQueryDsl;
use percent_encoding::utf8_percent_encode;
use rcgen::{
	CertificateParams, DistinguishedName, ExtendedKeyUsagePurpose, IsCa, KeyPair, KeyUsagePurpose,
	PKCS_ECDSA_P256_SHA256,
};
use uuid::Uuid;
use x509_parser::prelude::*;

use crate::db::TestDb;

#[derive(QueryableByName)]
struct Device {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

pub fn make_certificate() -> (Vec<u8>, String) {
	let key = KeyPair::generate_for(&PKCS_ECDSA_P256_SHA256).expect("keygen");
	let mut cert = CertificateParams::default();
	cert.is_ca = IsCa::NoCa;
	cert.not_before = OffsetDateTime::now_utc();
	cert.key_usages = vec![KeyUsagePurpose::DigitalSignature];
	cert.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
	cert.use_authority_key_identifier_extension = true;
	cert.distinguished_name = DistinguishedName::new();
	let cert = cert.self_signed(&key).expect("sign cert");

	let cert_pem = cert.pem();
	let cert = utf8_percent_encode(&cert_pem, percent_encoding::NON_ALPHANUMERIC).to_string();

	let (_, pem_parsed) = parse_x509_pem(cert_pem.as_bytes()).expect("parse pem");
	let (_, x509_cert) = parse_x509_certificate(&pem_parsed.contents).expect("parse cert");
	let key_data = x509_cert.tbs_certificate.subject_pki.raw.to_vec();

	(key_data, cert)
}

/// Like [`make_certificate`] but also returns a ring ECDSA key pair that can
/// sign arbitrary messages with the same key as the certificate — for testing
/// the enrollment proof-of-possession handshake. Returns
/// `(spki_der, percent_encoded_pem, signing_key)`.
pub fn make_signing_certificate() -> (Vec<u8>, String, ring::signature::EcdsaKeyPair) {
	use ring::signature::ECDSA_P256_SHA256_ASN1_SIGNING;

	let rng = ring::rand::SystemRandom::new();
	let pkcs8 =
		ring::signature::EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng)
			.expect("keygen");
	let signing_key = ring::signature::EcdsaKeyPair::from_pkcs8(
		&ECDSA_P256_SHA256_ASN1_SIGNING,
		pkcs8.as_ref(),
		&rng,
	)
	.expect("ring key");

	let key = KeyPair::try_from(pkcs8.as_ref()).expect("rcgen key from pkcs8");

	let mut cert = CertificateParams::default();
	cert.is_ca = IsCa::NoCa;
	cert.not_before = OffsetDateTime::now_utc();
	cert.key_usages = vec![KeyUsagePurpose::DigitalSignature];
	cert.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
	cert.use_authority_key_identifier_extension = true;
	cert.distinguished_name = DistinguishedName::new();
	let cert = cert.self_signed(&key).expect("sign cert");

	let cert_pem = cert.pem();
	let cert_enc = utf8_percent_encode(&cert_pem, percent_encoding::NON_ALPHANUMERIC).to_string();

	let (_, pem_parsed) = parse_x509_pem(cert_pem.as_bytes()).expect("parse pem");
	let (_, x509_cert) = parse_x509_certificate(&pem_parsed.contents).expect("parse cert");
	let key_data = x509_cert.tbs_certificate.subject_pki.raw.to_vec();

	(key_data, cert_enc, signing_key)
}

/// Derive the DER `SubjectPublicKeyInfo` from a PKCS#8 PEM private key, exactly
/// as the mTLS auth path derives it from a presented certificate: self-sign a
/// throwaway cert and read `subject_pki.raw`. Used to prove a server-minted
/// credential's stored public key matches the delivered private key.
pub fn spki_from_key_pem(key_pem: &str) -> Vec<u8> {
	let key = KeyPair::from_pem(key_pem).expect("rcgen key from pem");
	let mut cert = CertificateParams::default();
	cert.is_ca = IsCa::NoCa;
	cert.key_usages = vec![KeyUsagePurpose::DigitalSignature];
	cert.extended_key_usages = vec![ExtendedKeyUsagePurpose::ClientAuth];
	cert.distinguished_name = DistinguishedName::new();
	let cert = cert.self_signed(&key).expect("sign cert");

	let cert_pem = cert.pem();
	let (_, pem_parsed) = parse_x509_pem(cert_pem.as_bytes()).expect("parse pem");
	let (_, x509_cert) = parse_x509_certificate(&pem_parsed.contents).expect("parse cert");
	x509_cert.tbs_certificate.subject_pki.raw.to_vec()
}

/// The client-certificate header the suite runs against.
///
/// Envoy/XFCC deliberately, not the live nginx path: nginx is exercised
/// continuously by production, while XFCC is the shape that goes live at the
/// cutover and has no other real coverage. Testing it here means the eventual
/// removal of the nginx path is a delete rather than a rewrite.
///
/// The handful of tests that still pin the nginx header use
/// [`run_with_device_auth_on`].
const DEFAULT_CERT_HEADER: ClientCertHeader = ClientCertHeader::Xfcc;

pub async fn run<F, T, Fut>(test: F) -> T
where
	F: FnOnce(AsyncPgConnection, TestServer, TestServer) -> Fut,
	Fut: Future<Output = T>,
{
	run_on(DEFAULT_CERT_HEADER, test).await
}

/// The artifact store the servers of this run were built on, for a test that
/// seeds or reads the bytes Canopy holds.
///
/// Keyed by the throwaway database's name, which is unique per run: a test
/// reaches its own store through the connection it already has rather than
/// every harness callback in the workspace growing an argument for it.
pub async fn artifacts(conn: &mut AsyncPgConnection) -> ArtifactStore {
	#[derive(QueryableByName)]
	struct Name {
		#[diesel(sql_type = sql_types::Text)]
		current_database: String,
	}

	let name: Name = sql_query("SELECT current_database()")
		.get_result(conn)
		.await
		.expect("read database name");
	store_for(&name.current_database)
}

/// The store for one throwaway database, created on first ask.
fn store_for(database: &str) -> ArtifactStore {
	static STORES: Mutex<BTreeMap<String, ArtifactStore>> = Mutex::new(BTreeMap::new());

	STORES
		.lock()
		.expect("artifact stores")
		.entry(database.to_owned())
		.or_insert_with(ArtifactStore::memory)
		.clone()
}

/// [`run`] against an explicitly chosen client-certificate header.
pub async fn run_on<F, T, Fut>(cert_header: ClientCertHeader, test: F) -> T
where
	F: FnOnce(AsyncPgConnection, TestServer, TestServer) -> Fut,
	Fut: Future<Output = T>,
{
	TestDb::run(async |conn, url| {
		// One pool per state, shared between the RW and RO handles — a second
		// pool would double connections against the throwaway test cluster,
		// and this mirrors production with RO_DATABASE_URL unset.
		// One store per run, shared by both servers: an artifact uploaded through
		// the private API is the one the public download endpoint serves, the way
		// a single bucket serves both pods.
		let artifacts = store_for(database_name(&url));

		let public_db = database::init_to(&url);
		let public_state = public_server::state::AppState {
			client_cert_header: cert_header,
			db: public_db.clone(),
			db_read: public_db,
			tera: public_server::state::AppState::init_tera().unwrap(),
			server_versions_secret: Some("test-secret".to_string()),
			tailnet_directory: None,
			rate_limiter: Default::default(),
			sts: None,
			kube: None,
			artifacts: Some(artifacts.clone()),
			// From the environment, so a test can configure zones before building
			// the server the way the real edge does.
			dns_zones: commons_types::dns::ManagedZone::list_from_env().unwrap_or_default(),
		};
		let public_router = router(
			axum::Router::from(public_server::routes().with_state(public_state.clone()))
				.merge(public_server::calendar::routes(public_state.clone()))
				.merge(public_server::mcp::routes(public_state)),
			ClientIpSource::RightmostForwarded,
		);
		let private_router = router(
			private_server::routes(private_server::state::AppState {
				artifacts: Some(artifacts.clone()),
				..private_server::state::AppState::from_db_url(&url)
					.await
					.unwrap()
			})
			.unwrap(),
			ClientIpSource::RightmostForwarded,
		);

		let mut public_server = TestServer::new(public_router);
		public_server.add_header("Forwarded", "for=192.0.1.60");

		let mut private_server = TestServer::new(private_router);
		private_server.add_header("Forwarded", "for=192.0.2.60");

		test(conn, public_server, private_server).await
	})
	.await
}

#[allow(dead_code)] // when imported into a test that only uses run()
pub async fn run_with_device_auth<F, T, Fut>(role: &'static str, test: F) -> T
where
	F: FnOnce(AsyncPgConnection, String, Uuid, TestServer, TestServer) -> Fut,
	Fut: Future<Output = T>,
{
	run(async |mut conn, mut public, private| {
		let (key_data, cert) = make_certificate();

		let device_row: Device = sql_query(
			r#"
				INSERT INTO devices (role)
				VALUES ($1)
				RETURNING id
			"#,
		)
		.bind::<sql_types::Text, _>(role)
		.get_result(&mut conn)
		.await
		.expect("insert device");
		let device_id = device_row.id;

		sql_query(
			r#"
				INSERT INTO device_keys (device_id, key_data, name, is_active)
				VALUES ($1, $2, 'Test Key', true)
			"#,
		)
		.bind::<sql_types::Uuid, _>(device_id)
		.bind::<sql_types::Binary, _>(key_data)
		.execute(&mut conn)
		.await
		.expect("insert device key");

		public.add_header("X-Version", "3.4.5");
		test(conn, cert, device_id, public, private).await
	})
	.await
}

/// [`run_with_device_auth`] against an explicitly chosen client-certificate
/// header, for the tests that still cover the nginx path while the suite
/// defaults to XFCC.
pub async fn run_with_device_auth_on<F, T, Fut>(
	cert_header: ClientCertHeader,
	role: &'static str,
	test: F,
) -> T
where
	F: FnOnce(AsyncPgConnection, String, Uuid, TestServer, TestServer) -> Fut,
	Fut: Future<Output = T>,
{
	run_on(cert_header, async |mut conn, mut public, private| {
		let (key_data, cert) = make_certificate();

		let device_row: Device = sql_query(
			r#"
				INSERT INTO devices (role)
				VALUES ($1)
				RETURNING id
			"#,
		)
		.bind::<sql_types::Text, _>(role)
		.get_result(&mut conn)
		.await
		.expect("insert device");
		let device_id = device_row.id;

		sql_query(
			r#"
				INSERT INTO device_keys (device_id, key_data, name, is_active)
				VALUES ($1, $2, 'Test Key', true)
			"#,
		)
		.bind::<sql_types::Uuid, _>(device_id)
		.bind::<sql_types::Binary, _>(key_data)
		.execute(&mut conn)
		.await
		.expect("insert device key");

		public.add_header("X-Version", "3.4.5");
		test(conn, cert, device_id, public, private).await
	})
	.await
}

/// Run a test against a private-server primed with a populated tailnet
/// directory. The yielded device row is pre-attached to a known
/// `tailscale_node_id`, and the directory resolves `100.64.0.42` to
/// that node id with the `tag:canopy-server` tag. Tests typically
/// drive requests through the private server with
/// `.add_header("Forwarded", "for=100.64.0.42")` to look like a tagged
/// device on the tailnet.
///
/// The public TestServer is also yielded, but its state has no tailnet
/// directory (mirrors the production wiring on the internet edge), so
/// the tailnet auth path cannot fire on it.
#[allow(dead_code)]
pub async fn run_with_tailnet_device_auth<F, T, Fut>(role: &'static str, test: F) -> T
where
	F: FnOnce(AsyncPgConnection, std::net::IpAddr, String, Uuid, TestServer, TestServer) -> Fut,
	Fut: Future<Output = T>,
{
	let cert_header = DEFAULT_CERT_HEADER;
	use commons_servers::tailnet_directory::{DirectoryEntry, TailnetDirectory};

	let tailnet_ip: std::net::IpAddr = "100.64.0.42".parse().expect("parse test ip");
	let node_id = "nodekey:canopytest42".to_string();

	TestDb::run(async |mut conn, url| {
		let device_row: Device = sql_query(
			r#"
				INSERT INTO devices (role, tailscale_node_id, tailscale_node_name, tailscale_tailnet)
				VALUES ($1, $2, 'canopy-test-server', 'test-tailnet')
				RETURNING id
			"#,
		)
		.bind::<sql_types::Text, _>(role)
		.bind::<sql_types::Text, _>(&node_id)
		.get_result(&mut conn)
		.await
		.expect("insert tailnet device");
		let device_id = device_row.id;

		let directory = TailnetDirectory::for_test([(
			tailnet_ip,
			DirectoryEntry {
				node_id: node_id.clone(),
				node_name: "canopy-test-server".to_string(),
				tailnet: "test-tailnet".to_string(),
				tags: vec!["tag:canopy-server".to_string()],
				addresses: vec![tailnet_ip],
				last_seen: None,
				key_expiry_disabled: true,
			},
		)]);

		let public_db = database::init_to(&url);
		let public_state = public_server::state::AppState {
			client_cert_header: cert_header,
			db: public_db.clone(),
			db_read: public_db,
			tera: public_server::state::AppState::init_tera().unwrap(),
			server_versions_secret: Some("test-secret".to_string()),
			tailnet_directory: None,
			rate_limiter: Default::default(),
			sts: None,
			kube: None,
			artifacts: Some(store_for(database_name(&url))),
			// From the environment, so a test can configure zones before building
			// the server the way the real edge does.
			dns_zones: commons_types::dns::ManagedZone::list_from_env().unwrap_or_default(),
		};
		let public_router = router(
			axum::Router::from(public_server::routes().with_state(public_state.clone()))
				.merge(public_server::calendar::routes(public_state.clone()))
				.merge(public_server::mcp::routes(public_state)),
			ClientIpSource::RightmostForwarded,
		);
		let private_db = database::init_to(&url);
		let private_router = router(
			private_server::routes(private_server::state::AppState {
				client_cert_header: cert_header,
				db: private_db.clone(),
				db_read: private_db,
				ro_pool: None,
				tailnet_directory: Some(directory),
				kube: Some(public_server::state::BackupSecrets::memory()),
				sts: None,
				prober: private_server::backup_probe::BucketProber::fake(
					private_server::backup_probe::ProbeState::Empty,
				),
				artifacts: Some(store_for(database_name(&url))),
				recovery_recipients: None,
				recovery_challenge: std::sync::Arc::new(std::sync::Mutex::new(None)),
				// This harness is for the tailnet-auth paths; no test on it
				// touches group domains or certificates.
				dns_zones: Vec::new(),
				acme: None,
				acme_directory: None,
			})
			.unwrap(),
			ClientIpSource::RightmostForwarded,
		);

		let mut public = TestServer::new(public_router);
		public.add_header("Forwarded", "for=192.0.1.60");
		public.add_header("X-Version", "3.4.5");
		let private = TestServer::new(private_router);
		// No default Forwarded — each test supplies its own to control
		// whether the caller looks like a tailnet device or not.

		test(conn, tailnet_ip, node_id, device_id, public, private).await
	})
	.await
}

/// The throwaway database's name, which is the last path segment of its URL.
fn database_name(url: &str) -> &str {
	url.rsplit('/').next().expect("a database in the url")
}
