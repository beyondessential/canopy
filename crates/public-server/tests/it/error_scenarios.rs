use axum::{body::Bytes, http::StatusCode};
use commons_tests::diesel_async::SimpleAsyncConnection;
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn status_invalid_server_id() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to POST status with invalid UUID
		let response = public.post("/status/invalid-uuid").await;
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn status_nonexistent_server_id() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to POST status with valid UUID but nonexistent server
		let response = public
			.post("/status/11111111-1111-1111-1111-111111111111")
			.await;
		// This will likely fail due to missing authentication
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn status_missing_auth_headers() {
	commons_tests::server::run(async |mut conn, public, _| {
		// Create a test server
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, host, type, machine_id) VALUES ('11111111-1111-1111-1111-111111111111', 'https://test.com', 'tamanu-facility', '11111111-1111-1111-1111-111111111111')",
		)
		.await
		.unwrap();

		// Try to POST status without authentication
		let response = public
			.post("/status/11111111-1111-1111-1111-111111111111")
			.json(&json!({"test": "data"}))
			.await;

		// Should fail due to missing authentication
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_invalid_version_format() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to access version with invalid format
		let response = public.get("/versions/invalid.version.format").await;
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_negative_version() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to access version with negative numbers
		let response = public.get("/versions/-1.0.0").await;
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_create_without_auth() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to create version without authentication
		let response = public
			.post("/versions/2.0.0")
			.text("# New Version\n\nChangelog content")
			.await;

		// Should fail due to missing authentication
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_delete_without_auth() {
	commons_tests::server::run(async |mut conn, public, _| {
		// Create a version first
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		// Try to delete version without authentication
		let response = public.delete("/versions/1.0.0").await;

		// Should fail due to missing authentication
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifacts_create_without_auth() {
	commons_tests::server::run(async |mut conn, public, _| {
		// Create a version first
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		// Try to create artifact without authentication
		let response = public
			.post("/artifacts/1.0.0/mobile/android")
			.text("https://example.com/download.apk")
			.await;

		// Should fail due to missing authentication
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifacts_invalid_version() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to create artifact for invalid version format
		let response = public
			.post("/artifacts/invalid.version/mobile/android")
			.text("https://example.com/download.apk")
			.await;

		// Should fail due to invalid version format
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_update_for_invalid_version() {
	commons_tests::server::run(async |_conn, public, _| {
		// Try to get updates for invalid version format
		let response = public.get("/versions/update-for/invalid.version").await;
		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn timesync_invalid_request() {
	commons_tests::server::run(async |_conn, public, _| {
		// Send invalid timesync request data
		let response = public
			.post("/timesync")
			.bytes(Bytes::from(vec![0xff, 0xff, 0xff, 0xff])) // Invalid request
			.await;

		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn timesync_oversized_request() {
	commons_tests::server::run(async |_conn, public, _| {
		// Send very large request
		let large_request = vec![0; 1024];
		let response = public
			.post("/timesync")
			.bytes(Bytes::from(large_request))
			.await;

		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn error_empty_slug() {
	commons_tests::server::run(async |_conn, public, _| {
		// Test error redirect with empty slug
		let response = public.get("/errors/").await;
		response.assert_status(StatusCode::NOT_FOUND); // Should not match the route
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_sequential_access() {
	commons_tests::server::run(async |mut conn, public, _| {
		// Create a version
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		// Make multiple sequential requests to the same version
		for _ in 0..5 {
			let response = public.get("/versions/1.0.0/artifacts").await;
			response.assert_status_ok();
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn database_connection_sequential() {
	commons_tests::server::run(async |_conn, public, _| {
		// Make many sequential requests to test connection handling
		for _ in 0..20 {
			let response = public.get("/versions").await;
			response.assert_status_ok();
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn malformed_json_body() {
	commons_tests::server::run(async |mut conn, public, _| {
		// Create a server for status endpoint
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, host, type, machine_id) VALUES ('11111111-1111-1111-1111-111111111111', 'https://test.com', 'tamanu-central', '11111111-1111-1111-1111-111111111111')",
		)
		.await
		.unwrap();

		// Send malformed JSON to status endpoint
		let response = public
			.post("/status/11111111-1111-1111-1111-111111111111")
			.add_header("content-type", "application/json")
			.text("{invalid json}")
			.await;

		response.assert_status_not_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn missing_content_type() {
	commons_tests::server::run(async |_conn, public, _| {
		// Send request without content-type header where it might be expected
		let response = public.post("/timesync").text("some data").await;
		response.assert_status_not_ok();
	})
	.await
}
