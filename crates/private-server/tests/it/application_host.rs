//! What the admin API makes of a URL an operator types: a scheme filled in
//! when it is missing, and blank read as no URL rather than as an empty one.
//!
//! An application is reported rather than entered, so the URL arrives through
//! an update. A device-only application, with no URL at all, is a normal case.

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use database::applications::Application;
use serde_json::json;
use uuid::Uuid;

async fn seed(conn: &mut AsyncPgConnection, host: &str) -> Uuid {
	let id = Uuid::new_v4();
	let host_sql = if host.is_empty() {
		"NULL".to_string()
	} else {
		format!("'{host}'")
	};
	conn.batch_execute(&format!(
		"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '{id}') RETURNING id) \
		 INSERT INTO applications (id, name, host, rank, type, machine_id) \
		 VALUES ('{id}', 'Host Test', {host_sql}, 'test', 'tamanu-central', '{id}')"
	))
	.await
	.expect("seed application");
	id
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blank_url_clears_it_rather_than_storing_an_empty_one() {
	commons_tests::server::run(async |mut conn, _, private| {
		let id = seed(&mut conn, "https://original.example.com").await;

		private
			.post("/api/fleet/applications/update")
			.json(&json!({ "server_id": id, "data": { "host": "  " } }))
			.await
			.assert_status_ok();

		let application = Application::get_by_id(&mut conn, id).await.unwrap();
		assert!(
			application.host.is_none(),
			"whitespace is no URL, not an empty one"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_schemeless_url_is_read_as_https() {
	commons_tests::server::run(async |mut conn, _, private| {
		let id = seed(&mut conn, "").await;

		private
			.post("/api/fleet/applications/update")
			.json(&json!({ "server_id": id, "data": { "host": "foo.example.com" } }))
			.await
			.assert_status_ok();

		let application = Application::get_by_id(&mut conn, id).await.unwrap();
		assert_eq!(
			application.host.unwrap().0.to_string(),
			"https://foo.example.com/"
		);
	})
	.await
}

/// An application with no URL is not a broken record: it is reached through
/// its device rather than at an address an operator typed.
#[tokio::test(flavor = "multi_thread")]
async fn an_application_may_carry_no_url_at_all() {
	commons_tests::server::run(async |mut conn, _, _private| {
		let id = seed(&mut conn, "").await;
		let application = Application::get_by_id(&mut conn, id).await.unwrap();
		assert!(application.host.is_none());
	})
	.await
}
