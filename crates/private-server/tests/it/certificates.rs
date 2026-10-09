//! Endpoint tests for the operator-facing `/api/certificates/*` fns that move a
//! DNS name between the applications on a box for certificates: declaring which
//! one serves it, and releasing the hold so it can go elsewhere. The address
//! side is `dns_names.rs`.

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use uuid::Uuid;

/// Two applications on one machine, which is the case declarations exist for.
pub(crate) async fn two_workloads_on_a_box(conn: &mut AsyncPgConnection) -> (Uuid, Uuid) {
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

// spec: DNS#declared-dns-names
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

// spec: DNS#declared-dns-names
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

// ── Undeclared requests and denials ─────────────────────────────────────────

/// A group claiming `domain`, and a machine in it carrying two applications of
/// different types, both in the group.
pub(crate) async fn shared_box_in_group(
	conn: &mut AsyncPgConnection,
	domain: &str,
) -> (Uuid, Uuid, Uuid) {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let (tamanu, lab) = (Uuid::new_v4(), Uuid::new_v4());
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group}', 'grp-{group}'); \
		 INSERT INTO server_group_domains (group_id, domain) VALUES ('{group}', '{domain}'); \
		 INSERT INTO machines (id, name, group_id) VALUES ('{machine}', 'box', '{group}'); \
		 INSERT INTO applications (id, name, host, type, group_id, machine_id) VALUES \
		   ('{tamanu}', 'central', 'https://{tamanu}.example.invalid', 'tamanu-central', '{group}', '{machine}'), \
		   ('{lab}', 'lab', 'https://{lab}.example.invalid', 'senaite', '{group}', '{machine}')"
	))
	.await
	.expect("seed shared box");
	(machine, tamanu, lab)
}

pub(crate) async fn record_undeclared(
	conn: &mut AsyncPgConnection,
	machine: Uuid,
	name: &str,
	kind: &str,
	age: &str,
) {
	conn.batch_execute(&format!(
		"INSERT INTO undeclared_dns_names (machine_id, dns_name, kind, first_asked_at, last_asked_at) \
		 VALUES ('{machine}', '{name}', '{kind}', now() - interval '{age}', now() - interval '{age}')"
	))
	.await
	.expect("record undeclared");
}

// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn deny_lift_roundtrip() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, _, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		record_undeclared(
			&mut conn,
			machine,
			"old.fiji.tamanu.app",
			"certificate",
			"1 minute",
		)
		.await;

		let resp = private
			.post("/api/certificates/deny")
			.json(&serde_json::json!({
				"machine_id": machine,
				"name": "Old.Fiji.Tamanu.App.",
				"note": "site retired",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["name"], "old.fiji.tamanu.app");
		assert_eq!(body["note"], "site retired");
		assert_eq!(body["denied_by"], "admin@localhost");

		let view: serde_json::Value = private
			.post("/api/certificates/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert!(
			view["undeclared"].as_array().unwrap().is_empty(),
			"denying ends the undeclared record: {view}"
		);
		assert_eq!(view["denied"][0]["name"], "old.fiji.tamanu.app");

		private
			.post("/api/certificates/lift_denial")
			.json(&serde_json::json!({"machine_id": machine, "name": "old.fiji.tamanu.app"}))
			.await
			.assert_status_ok();
		private
			.post("/api/certificates/lift_denial")
			.json(&serde_json::json!({"machine_id": machine, "name": "old.fiji.tamanu.app"}))
			.await
			.assert_status(axum::http::StatusCode::NOT_FOUND);
	})
	.await
}

/// A declaration is the answer an undeclared request waited for and the
/// opposite of a denial, so it ends both for the declaring application's box.
// spec: DNS#denied-dns-names
// spec: DNS#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn declaring_ends_the_undeclared_record_and_the_denial() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, tamanu, lab) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		record_undeclared(
			&mut conn,
			machine,
			"central.fiji.tamanu.app",
			"certificate",
			"1 minute",
		)
		.await;
		private
			.post("/api/certificates/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": "lab.fiji.tamanu.app"}))
			.await
			.assert_status_ok();

		for (application, name) in [
			(tamanu, "central.fiji.tamanu.app"),
			(lab, "lab.fiji.tamanu.app"),
		] {
			private
				.post("/api/certificates/declare")
				.json(&serde_json::json!({"application_id": application, "name": name}))
				.await
				.assert_status_ok();
		}

		let view: serde_json::Value = private
			.post("/api/certificates/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert!(view["undeclared"].as_array().unwrap().is_empty(), "{view}");
		assert!(view["denied"].as_array().unwrap().is_empty(), "{view}");
		let declared: Vec<(&str, &str)> = view["declared"]
			.as_array()
			.unwrap()
			.iter()
			.map(|d| {
				(
					d["name"].as_str().unwrap(),
					d["application_name"].as_str().unwrap(),
				)
			})
			.collect();
		assert_eq!(
			declared,
			vec![
				("central.fiji.tamanu.app", "central"),
				("lab.fiji.tamanu.app", "lab")
			]
		);
	})
	.await
}

/// Denying contradicts a declaration on the same box, which has to go first.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn denying_a_name_declared_on_the_machine_is_refused() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, tamanu, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": tamanu, "name": "central.fiji.tamanu.app"}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/certificates/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": "central.fiji.tamanu.app"}))
			.await;
		resp.assert_status(axum::http::StatusCode::CONFLICT);
		let body: serde_json::Value = resp.json();
		assert!(
			body["title"]
				.as_str()
				.unwrap_or_default()
				.contains("central"),
			"the refusal names the declaring application: {body}"
		);
	})
	.await
}

/// Declaring a name outside the group's domains is allowed and flagged, since
/// nothing can be published or certified for it until the group claims one.
// spec: DNS#declared-dns-names
// spec: DNS#on-an-application
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_outside_the_group_domains_is_flagged() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (_, tamanu, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;

		let outside: serde_json::Value = private
			.post("/api/certificates/declare")
			.json(
				&serde_json::json!({"application_id": tamanu, "name": "central.samoa.tamanu.app"}),
			)
			.await
			.json();
		assert_eq!(outside["within_domains"], false);

		let inside: serde_json::Value = private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": tamanu, "name": "central.fiji.tamanu.app"}))
			.await
			.json();
		assert_eq!(inside["within_domains"], true);
	})
	.await
}
