use commons_tests::server;
use diesel_async::SimpleAsyncConnection;
use http::StatusCode;

#[tokio::test(flavor = "multi_thread")]
async fn server_versions_wrong_secret() {
	server::run(|_conn, public, _private| async move {
		// Request with wrong secret should fail
		let response = public.get("/server-versions?s=wrong-secret").await;
		assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_versions_missing_query_parameter() {
	server::run(|_conn, public, _private| async move {
		// Request without query parameter should fail with bad request
		let response = public.get("/server-versions").await;
		assert_eq!(response.status_code(), StatusCode::BAD_REQUEST);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_versions_correct_secret() {
	server::run(|mut conn, public, _private| async move {
		// Set up test data with multiple applications
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111'), ('box', '22222222-2222-2222-2222-222222222222'), ('box', '33333333-3333-3333-3333-333333333333'), ('box', '44444444-4444-4444-4444-444444444444') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Production Application 1', 'https://prod1.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111'),
			('22222222-2222-2222-2222-222222222222', 'Production Application 2', 'https://prod2.example.com', 'production', 'tamanu-central', '22222222-2222-2222-2222-222222222222'),
			('33333333-3333-3333-3333-333333333333', 'Clone Application', 'https://clone.example.com', 'clone', 'tamanu-central', '33333333-3333-3333-3333-333333333333'),
			('44444444-4444-4444-4444-444444444444', 'Facility Application', 'https://facility.example.com', 'production', 'tamanu-facility', '44444444-4444-4444-4444-444444444444')",
		)
		.await
		.unwrap();

		// Add versions and statuses
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 2, 10, 0, 'published', 'Version 2.10.0'),
			('bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 2, 11, 0, 'published', 'Version 2.11.0'),
			('cccccccc-cccc-cccc-cccc-cccccccccccc', 2, 12, 0, 'published', 'Version 2.12.0')",
		)
		.await
		.unwrap();

		// Add statuses for applications
		conn.batch_execute(
			"INSERT INTO statuses (id, server_id, version, extra) VALUES
			('11111111-1111-1111-1111-111111111111', '11111111-1111-1111-1111-111111111111', '2.12.0', '{}'),
			('22222222-2222-2222-2222-222222222222', '22222222-2222-2222-2222-222222222222', '2.10.0', '{}')",
		)
		.await
		.unwrap();

		// Request with correct secret should succeed
		let response = public.get("/server-versions?s=test-secret").await;
		assert_eq!(response.status_code(), StatusCode::OK);
		let body = response.text();
		assert!(body.contains("Production Server Versions"));

		// Should display production central applications only
		assert!(body.contains("Production Application 1"));
		assert!(body.contains("Production Application 2"));
		assert!(body.contains("2.12.0"));
		assert!(body.contains("2.10.0"));

		// Should NOT display clone or facility applications
		assert!(!body.contains("Clone Application"));
		assert!(!body.contains("Facility Application"));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_versions_constant_time_comparison() {
	server::run(|_conn, public, _private| async move {
		// Different length should fail
		let response = public.get("/server-versions?s=short").await;
		assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

		// Same length but wrong secret should fail
		let response = public.get("/server-versions?s=wrong-secret").await;
		assert_eq!(response.status_code(), StatusCode::UNAUTHORIZED);

		// Correct secret should succeed
		let response = public.get("/server-versions?s=test-secret").await;
		assert_eq!(response.status_code(), StatusCode::OK);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn server_versions_rc_section() {
	server::run(|mut conn, public, _private| async move {
		// Create a published version to establish the current minor
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 2, 15, 0, 'published', 'Version 2.15.0')",
		)
		.await
		.unwrap();

		// Request with correct secret
		let response = public.get("/server-versions?s=test-secret").await;
		assert_eq!(response.status_code(), StatusCode::OK);
		let body = response.text();

		// RC section should not be shown since the environments are not actually available
		// (the probe will fail in the test environment)
		assert!(!body.contains("<h1>RC</h1>"));
	})
	.await;
}

/// A server can report a version while `version_distance` is null — most
/// plainly when nothing is published yet, so there's no latest to measure
/// against. The distance ladder in the template compares that against
/// numbers, and a null there used to blow the whole page up.
#[tokio::test(flavor = "multi_thread")]
async fn server_versions_renders_a_reported_version_with_no_distance() {
	server::run(|mut conn, public, _private| async move {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Production Application 1', 'https://prod1.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111');
			INSERT INTO statuses (id, server_id, version, extra) VALUES
			('11111111-1111-1111-1111-111111111111', '11111111-1111-1111-1111-111111111111', '2.12.0', '{}')",
		)
		.await
		.unwrap();

		let response = public.get("/server-versions?s=test-secret").await;
		assert_eq!(response.status_code(), StatusCode::OK);
		let body = response.text();
		assert!(body.contains("Production Application 1"));
		assert!(body.contains("2.12.0"));
	})
	.await;
}
