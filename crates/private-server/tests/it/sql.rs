//! The SQL playground's read-only query endpoint. Needs a real read-only
//! pool, which the shared harness doesn't build (it mirrors production with
//! `RO_DATABASE_URL` unset), so these stand up their own state.

use axum_client_ip::ClientIpSource;
use commons_servers::router;
use commons_tests::axum_test::TestServer;
use commons_tests::db::TestDb;

async fn private_with_ro_pool(url: &str) -> TestServer {
	let db = database::init_to(url);
	let ro_pool = bestool_postgres::pool::create_pool(url, "canopy-playground-test")
		.await
		.expect("read-only pool");
	let router = router(
		private_server::routes(private_server::state::AppState {
			client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::Xfcc,
			db: db.clone(),
			db_read: db,
			ro_pool: Some(ro_pool),
			tailnet_directory: None,
			kube: None,
			sts: None,
			prober: private_server::backup_probe::BucketProber::fake(
				private_server::backup_probe::ProbeState::Empty,
			),
			artifacts: Some(commons_servers::artifact_store::ArtifactStore::memory()),
			recovery_recipients: None,
			recovery_challenge: std::sync::Arc::new(std::sync::Mutex::new(None)),
			dns_zones: Vec::new(),
			acme: None,
			acme_directory: None,
		})
		.unwrap(),
		ClientIpSource::RightmostForwarded,
	);
	let mut server = TestServer::new(router);
	server.add_header("Forwarded", "for=192.0.2.10");
	server
}

/// `execution_time_ms` is documented as the query's own time. It used to
/// start before both pool checkouts, the history INSERT, and the
/// transaction setup, and to stop before the values were decoded — so it
/// measured endpoint overhead at one end and missed real work at the other.
///
/// The window now brackets the execution itself. A query that sleeps for a
/// known time is the one thing that pins that from outside: the reported
/// figure has to account for the sleep, and a trivial query next to it has
/// to come back far below it.
#[tokio::test(flavor = "multi_thread")]
async fn execution_time_measures_the_query_not_the_endpoint() {
	TestDb::run(async |_conn, url| {
		let private = private_with_ro_pool(&url).await;

		let slow: serde_json::Value = private
			.post("/api/sql/execute_query")
			.json(&serde_json::json!({ "query": { "query": "SELECT pg_sleep(0.5)" } }))
			.await
			.json();
		let slow_ms = slow["execution_time_ms"].as_u64().expect("timing");
		assert!(
			slow_ms >= 450,
			"a half-second query must be reported as such, got {slow_ms}ms: {slow}",
		);

		let fast: serde_json::Value = private
			.post("/api/sql/execute_query")
			.json(&serde_json::json!({ "query": { "query": "SELECT 1 AS n" } }))
			.await
			.json();
		let fast_ms = fast["execution_time_ms"].as_u64().expect("timing");
		assert!(
			fast_ms < 250,
			"a trivial query must not be reported as slow, got {fast_ms}ms: {fast}",
		);
	})
	.await
}
