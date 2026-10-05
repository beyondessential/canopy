use diesel_async::SimpleAsyncConnection;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct PublicServer {
	pub name: String,
	pub host: String,
	pub rank: Option<String>,
}

// GET /applications tests. (The device-driven create/edit/remove endpoints were
// removed in favour of operator-first enrollment; see server_enrollment.rs.)

#[tokio::test(flavor = "multi_thread")]
async fn get_empty_list() {
	commons_tests::server::run(async |_conn, public, _| {
		let response = public.get("/servers").await;
		response.assert_status_ok();
		response.assert_json::<Vec<PublicServer>>(&Vec::new());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_with_central_server() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) INSERT INTO applications (name, host, type, rank, public_name, machine_id) SELECT 'Test Application', 'https://test.com', 'tamanu-central', 'production', 'Test Application', m.id FROM m",
		)
		.await
		.unwrap();

		let response = public.get("/servers").await;
		response.assert_status_ok();
		response.assert_json::<Vec<PublicServer>>(&vec![PublicServer {
			name: "Test Application".to_string(),
			host: "https://test.com".to_string(),
			rank: Some("production".to_string()),
		}]);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_without_public_name() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) INSERT INTO applications (name, host, type, rank, machine_id) SELECT 'Internal Application', 'https://test.com', 'tamanu-central', 'production', m.id FROM m",
		)
		.await
		.unwrap();

		let response = public.get("/servers").await;
		response.assert_status_ok();
		response.assert_json::<Vec<PublicServer>>(&Vec::new());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_filters_facility_servers() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id)
			INSERT INTO applications (name, host, type, rank, public_name, machine_id)
			SELECT 'Central Application', 'https://central.com', 'tamanu-central', 'production', 'Central Application', m.id FROM m;
			WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id)
			INSERT INTO applications (name, host, type, rank, public_name, machine_id)
			SELECT 'Facility Application', 'https://facility.com', 'tamanu-facility', 'production', NULL, m.id FROM m;",
		)
		.await
		.unwrap();

		let response = public.get("/servers").await;
		response.assert_status_ok();
		response.assert_json::<Vec<PublicServer>>(&vec![PublicServer {
			name: "Central Application".to_string(),
			host: "https://central.com".to_string(),
			rank: Some("production".to_string()),
		}]);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_multiple_central_servers() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id)
			INSERT INTO applications (name, host, type, rank, public_name, machine_id)
			SELECT 'Application A', 'https://a.com', 'tamanu-central', 'production', 'Application A', m.id FROM m;
			WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id)
			INSERT INTO applications (name, host, type, rank, public_name, machine_id)
			SELECT 'Application B', 'https://b.com', 'tamanu-central', 'staging', 'Application B', m.id FROM m;",
		)
		.await
		.unwrap();

		let response = public.get("/servers").await;
		response.assert_status_ok();
		let applications: Vec<PublicServer> = response.json();
		assert_eq!(applications.len(), 2);
		assert!(
			applications
				.iter()
				.any(|s| s.name == "Application A" && s.host == "https://a.com")
		);
		assert!(
			applications
				.iter()
				.any(|s| s.name == "Application B" && s.host == "https://b.com")
		);
	})
	.await
}
