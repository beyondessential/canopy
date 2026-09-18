//! Integration tests for the tailnet path of the device-auth extractor
//! on the private-server's `/public/...` mount.

use commons_tests::diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

async fn provision_server(
	conn: &mut commons_tests::diesel_async::AsyncPgConnection,
	device_id: Uuid,
) -> Uuid {
	let server_id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, device_id) VALUES ('{server_id}', '{device_id}'); \
		 INSERT INTO applications (id, host, type, machine_id) \
		 VALUES ('{server_id}', 'https://test.example.com', 'tamanu-central', '{server_id}');"
	))
	.await
	.expect("insert server");
	server_id
}

#[tokio::test(flavor = "multi_thread")]
async fn tailnet_server_device_can_push_status() {
	commons_tests::server::run_with_tailnet_device_auth(
		"server",
		async |mut conn, tailnet_ip, _node_id, device_id, _public, private| {
			let server_id = provision_server(&mut conn, device_id).await;

			let response = private
				.post(&format!("/public/status/{server_id}"))
				.add_header("Forwarded", &format!("for={tailnet_ip}"))
				.json(&serde_json::json!({
					"health": [ { "check": "disk", "result": "passed" } ],
				}))
				.await;
			response.assert_status_ok();

			// The push landed as a status row against the right server.
			conn.batch_execute(&format!(
				"DO $$ BEGIN IF NOT EXISTS (SELECT 1 FROM statuses \
				 WHERE server_id = '{server_id}' AND device_id = '{device_id}') \
				 THEN RAISE EXCEPTION 'status row not recorded'; END IF; END $$;"
			))
			.await
			.expect("status row recorded for the server");
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn unknown_tailnet_node_is_rejected_without_creating_a_row() {
	// The directory knows the caller's IP but no `Device` row has the
	// corresponding `tailscale_node_id` yet. The dual-auth extractor rejects
	// the request (unauthenticated) and does not auto-create any device row —
	// devices only exist once an operator provisions or attaches them.
	use axum_client_ip::ClientIpSource;
	use commons_servers::{
		router,
		tailnet_directory::{DirectoryEntry, TailnetDirectory},
	};
	use commons_tests::axum_test::TestServer;
	use commons_tests::db::TestDb;

	TestDb::run(async |conn, url| {
		let tailnet_ip: std::net::IpAddr = "100.64.0.99".parse().unwrap();
		let unknown_node_id = "nodekey:firstcontact99".to_string();

		let directory = TailnetDirectory::for_test([(
			tailnet_ip,
			DirectoryEntry {
				node_id: unknown_node_id.clone(),
				node_name: "fresh-node".into(),
				tailnet: "test-tailnet".into(),
				tags: vec!["tag:canopy-server".into()],
				addresses: vec![tailnet_ip],
				last_seen: None,
				key_expiry_disabled: true,
			},
		)]);

		let db = database::init_to(&url);
		let private_router = router(
			private_server::routes(private_server::state::AppState {
				client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::Xfcc,
				db: db.clone(),
				db_read: db,
				ro_pool: None,
				tailnet_directory: Some(directory),
				kube: None,
				sts: None,
				prober: private_server::backup_probe::BucketProber::fake(
					private_server::backup_probe::ProbeState::Empty,
				),
				artifacts: Some(commons_servers::artifact_store::ArtifactStore::memory()),
				recovery_recipients: None,
				recovery_challenge: std::sync::Arc::new(std::sync::Mutex::new(None)),
				dns_zones: Vec::new(),
				// Neither harness touches certificates.
				acme: None,
				acme_directory: None,
			})
			.unwrap(),
			ClientIpSource::RightmostForwarded,
		);
		let private = TestServer::new(private_router);

		let response = private
			.post(&format!("/public/status/{}", Uuid::new_v4()))
			.add_header("Forwarded", &format!("for={tailnet_ip}"))
			.json(&serde_json::json!({ "health": [] }))
			.await;
		assert_eq!(response.status_code().as_u16(), 401);

		// And no device row was created for the unknown node.
		use commons_tests::diesel_async::SimpleAsyncConnection;
		let mut conn = conn;
		conn.batch_execute(&format!(
			"DO $$ BEGIN IF EXISTS (SELECT 1 FROM devices \
			 WHERE tailscale_node_id = '{unknown_node_id}') \
			 THEN RAISE EXCEPTION 'unknown tailnet node should not create a device row'; END IF; END $$;"
		))
		.await
		.expect("no device row created for unknown node");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn non_tailnet_source_ip_rejected() {
	// Same setup (directory populated, device pre-attached), but the
	// request comes in with a public-IP Forwarded header. The tailnet
	// path's spoof-guard rejects on `!is_tailnet_ip`, and the tunnel
	// path doesn't fall back to mTLS (the Tailscale ingress proxy
	// terminates client TLS, so attempting mTLS here would never
	// succeed anyway). Expect 401 AuthMissingCertificate.
	commons_tests::server::run_with_tailnet_device_auth(
		"server",
		async |mut conn, _tailnet_ip, _node_id, device_id, _public, private| {
			let server_id = provision_server(&mut conn, device_id).await;

			let response = private
				.post(&format!("/public/status/{server_id}"))
				.add_header("Forwarded", "for=203.0.113.7")
				.json(&serde_json::json!({ "health": [] }))
				.await;
			assert_eq!(response.status_code().as_u16(), 401);
			let body: serde_json::Value = response.json();
			assert_eq!(
				body.get("type").and_then(|v| v.as_str()),
				Some("/errors/auth-tailnet-identity-missing"),
			);
		},
	)
	.await
}
