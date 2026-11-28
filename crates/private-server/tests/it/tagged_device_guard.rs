//! Integration tests for `commons_servers::tailnet_guard`: tagged-device
//! callers (no `Tailscale-User-Login` header, source IP in the
//! Tailscale CGNAT v4 or ULA v6 ranges) must be rejected from every
//! private-server surface outside `/public/...`.

#[tokio::test(flavor = "multi_thread")]
async fn tagged_device_rejected_on_spa_fallback() {
	commons_tests::server::run(async |_conn, _public, private| {
		let response = private
			.get("/")
			.add_header("Forwarded", "for=100.64.0.42")
			.await;
		assert_eq!(response.status_code().as_u16(), 403);
		let body: serde_json::Value = response.json();
		assert_eq!(
			body.get("type").and_then(|v| v.as_str()),
			Some("/errors/tagged-device-not-allowed"),
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn tagged_device_rejected_on_api_route() {
	commons_tests::server::run(async |_conn, _public, private| {
		let response = private
			.post("/api/commons/current_user_level")
			.add_header("Forwarded", "for=100.64.0.42")
			.json(&serde_json::json!({}))
			.await;
		assert_eq!(response.status_code().as_u16(), 403);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn tagged_device_rejected_on_ipv6_ula() {
	commons_tests::server::run(async |_conn, _public, private| {
		let response = private
			.get("/")
			.add_header("Forwarded", "for=\"[fd7a:115c:a1e0::3701:2c8a]\"")
			.await;
		assert_eq!(response.status_code().as_u16(), 403);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn human_user_with_user_login_header_passes() {
	// Tailnet source IP but a populated User-Login header → logged-in
	// human; the guard should not 403 this. (The handler itself may
	// still reject for other reasons, but not with our error type.)
	commons_tests::server::run(async |_conn, _public, private| {
		let response = private
			.get("/")
			.add_header("Forwarded", "for=100.64.0.42")
			.add_header("Tailscale-User-Login", "alice@example.com")
			.await;
		assert_ne!(response.status_code().as_u16(), 403);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn non_tailnet_source_ip_passes_guard() {
	commons_tests::server::run(async |_conn, _public, private| {
		let response = private
			.get("/")
			.add_header("Forwarded", "for=203.0.113.7")
			.await;
		assert_ne!(response.status_code().as_u16(), 403);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn public_mount_is_not_subject_to_tagged_device_guard() {
	// `/public/...` lives outside the guard's coverage. A tagged-device
	// caller here gets the underlying auth handling (401 AuthMissing*
	// because no mTLS cert and no tailnet directory configured in this
	// test fixture), NOT a 403 from the guard.
	commons_tests::server::run(async |_conn, _public, private| {
		let response = private
			.post(&format!("/public/status/{}", uuid::Uuid::new_v4()))
			.add_header("Forwarded", "for=100.64.0.42")
			.json(&serde_json::json!({ "health": [] }))
			.await;
		assert_eq!(response.status_code().as_u16(), 401);
		let body: serde_json::Value = response.json();
		assert_ne!(
			body.get("type").and_then(|v| v.as_str()),
			Some("/errors/tagged-device-not-allowed"),
		);
	})
	.await
}
