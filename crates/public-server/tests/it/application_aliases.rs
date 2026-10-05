//! `/applications` and `/servers` answer identically on the device API.
//!
//! A device API client cannot follow a redirect: a fielded bestool asks for the
//! path it was built against and takes the answer. So the application
//! endpoints gained the name they are about without the old one going
//! anywhere, and both have to keep saying the same thing — an alias that
//! drifted would be worse than not having one.

use diesel::{sql_query, sql_types};
use diesel_async::{RunQueryDsl, SimpleAsyncConnection};
use http::StatusCode;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
struct PublicServer {
	name: String,
	host: String,
	rank: Option<String>,
}

#[derive(Deserialize)]
struct SelfResponse {
	server_id: Uuid,
	device_id: Uuid,
}

#[tokio::test(flavor = "multi_thread")]
async fn the_listing_reads_the_same_under_both_names() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) \
			 INSERT INTO applications (name, host, type, rank, public_name, machine_id) \
			 SELECT 'Test Application', 'https://test.com', 'tamanu-central', 'production', 'Test Application', m.id FROM m",
		)
		.await
		.unwrap();

		let expected = vec![PublicServer {
			name: "Test Application".to_string(),
			host: "https://test.com".to_string(),
			rank: Some("production".to_string()),
		}];

		let aliased = public.get("/applications").await;
		aliased.assert_status_ok();
		aliased.assert_json::<Vec<PublicServer>>(&expected);

		let original = public.get("/servers").await;
		original.assert_status_ok();
		original.assert_json::<Vec<PublicServer>>(&expected);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn self_reads_the_same_under_both_names() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = Uuid::new_v4();
			sql_query(
				"WITH m AS (INSERT INTO machines (name, id, device_id) VALUES ('box', $1, $2) RETURNING id) \
				 INSERT INTO applications (id, host, type, machine_id) \
				 VALUES ($1, 'https://self.example.com', 'tamanu-central', $1)",
			)
			.bind::<sql_types::Uuid, _>(machine_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.execute(&mut conn)
			.await
			.unwrap();

			for path in ["/applications/self", "/servers/self"] {
				let response = public
					.get(path)
					.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
					.await;
				response.assert_status_ok();
				let body: SelfResponse = response.json();
				assert_eq!(body.server_id, machine_id, "{path}");
				assert_eq!(body.device_id, device_id, "{path}");
			}
		},
	)
	.await
}

/// The alias is authenticated, not merely present: an alias that answered
/// without a certificate would be a way around the device gate.
#[tokio::test(flavor = "multi_thread")]
async fn the_aliased_self_still_requires_a_certificate() {
	commons_tests::server::run(async |_conn, public, _| {
		let response = public.get("/applications/self").await;
		response.assert_status(StatusCode::UNAUTHORIZED);
	})
	.await
}
