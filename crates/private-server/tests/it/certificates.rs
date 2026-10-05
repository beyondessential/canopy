//! Endpoint tests for the operator-facing `/api/certificates/*` fns that move a
//! name between the applications on a box: declaring which one serves it, and
//! releasing the hold so it can go elsewhere.

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use uuid::Uuid;

/// Two applications on one machine, which is the case declarations exist for.
async fn two_workloads_on_a_box(conn: &mut AsyncPgConnection) -> (Uuid, Uuid) {
	let machine = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (name, id) VALUES ('box', '{machine}')"
	))
	.await
	.expect("insert machine");
	let mut ids = Vec::new();
	for name in ["front", "worker"] {
		let id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO applications (id, name, host, type, machine_id) \
			 VALUES ('{id}', '{name}', 'https://{id}.example.invalid', 'tamanu-central', '{machine}')"
		))
		.await
		.expect("insert application");
		ids.push(id);
	}
	(ids[0], ids[1])
}

// spec: CRT#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn declare_release_roundtrip() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (front, worker) = two_workloads_on_a_box(&mut conn).await;

		let resp = private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({
				"application_id": front,
				"name": "Shared.Fiji.Tamanu.App.",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["name"], "shared.fiji.tamanu.app");

		// The other workload on the same box cannot take it while it is held.
		let refused = private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({
				"application_id": worker,
				"name": "shared.fiji.tamanu.app",
			}))
			.await;
		refused.assert_status(axum::http::StatusCode::CONFLICT);
		let problem: serde_json::Value = refused.json();
		assert!(
			problem["title"]
				.as_str()
				.expect("title")
				.contains(&front.to_string()),
			"an operator is told what to release first, but got: {problem}"
		);

		private
			.post("/api/certificates/release")
			.json(&serde_json::json!({
				"application_id": front,
				"name": "shared.fiji.tamanu.app",
			}))
			.await
			.assert_status_ok();

		private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({
				"application_id": worker,
				"name": "shared.fiji.tamanu.app",
			}))
			.await
			.assert_status_ok();
	})
	.await;
}

// spec: CRT#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn releasing_a_name_an_application_does_not_hold_is_a_404() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (front, _) = two_workloads_on_a_box(&mut conn).await;

		private
			.post("/api/certificates/release")
			.json(&serde_json::json!({
				"application_id": front,
				"name": "never.fiji.tamanu.app",
			}))
			.await
			.assert_status_not_found();
	})
	.await;
}
