//! Endpoint tests for the operator-facing `/api/dns_names/*` fns: the address
//! side of declaring, denying and releasing DNS names, and what it shares with
//! the certificate side (`certificates.rs`), which is that a DNS name has one
//! holder, and that undeclared requests and denials are kept per kind.

use axum::http::StatusCode;

use crate::certificates::{record_undeclared, shared_box_in_group, two_workloads_on_a_box};

// spec: DNS#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn declare_release_roundtrip() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (front, worker) = two_workloads_on_a_box(&mut conn).await;

		let resp = private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({
				"application_id": front,
				"name": "Shared.Fiji.Tamanu.App.",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["name"], "shared.fiji.tamanu.app");
		assert_eq!(body["addresses"], serde_json::json!([]));

		let refused = private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({
				"application_id": worker,
				"name": "shared.fiji.tamanu.app",
			}))
			.await;
		refused.assert_status(StatusCode::CONFLICT);
		let problem: serde_json::Value = refused.json();
		assert!(
			problem["title"]
				.as_str()
				.expect("title")
				.contains(&front.to_string()),
			"an operator is told what to release first, but got: {problem}"
		);

		private
			.post("/api/dns_names/release")
			.json(&serde_json::json!({
				"application_id": front,
				"name": "shared.fiji.tamanu.app",
			}))
			.await
			.assert_status_ok();
		private
			.post("/api/dns_names/release")
			.json(&serde_json::json!({
				"application_id": front,
				"name": "shared.fiji.tamanu.app",
			}))
			.await
			.assert_status_not_found();

		private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({
				"application_id": worker,
				"name": "shared.fiji.tamanu.app",
			}))
			.await
			.assert_status_ok();
	})
	.await;
}

/// A DNS name is held by one application whichever kinds it is declared for, so
/// the other kind's endpoint refuses it too, and the same application may hold
/// it for both.
// spec: DNS#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn a_dns_name_has_one_holder_across_kinds() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (front, worker) = two_workloads_on_a_box(&mut conn).await;
		let name = "both.fiji.tamanu.app";

		private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({"application_id": front, "name": name}))
			.await
			.assert_status_ok();

		let refused = private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": worker, "name": name}))
			.await;
		refused.assert_status(StatusCode::CONFLICT);
		let problem: serde_json::Value = refused.json();
		assert!(
			problem["title"]
				.as_str()
				.expect("title")
				.contains(&front.to_string()),
			"the refusal names the address holder: {problem}"
		);

		private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": front, "name": name}))
			.await
			.assert_status_ok();

		let refused = private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({"application_id": worker, "name": name}))
			.await;
		refused.assert_status(StatusCode::CONFLICT);

		// Released for one kind it is still held for the other.
		private
			.post("/api/dns_names/release")
			.json(&serde_json::json!({"application_id": front, "name": name}))
			.await
			.assert_status_ok();
		private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": worker, "name": name}))
			.await
			.assert_status(StatusCode::CONFLICT);
	})
	.await;
}

/// Each machine section carries only its own kind: what is declared, what was
/// asked about, and what is denied.
// spec: DNS#on-a-machine
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_sections_carry_only_their_own_kind() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, tamanu, lab) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;

		private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({"application_id": tamanu, "name": "records.fiji.tamanu.app"}))
			.await
			.assert_status_ok();
		private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": lab, "name": "tls.fiji.tamanu.app"}))
			.await
			.assert_status_ok();
		record_undeclared(
			&mut conn,
			machine,
			"ask-a.fiji.tamanu.app",
			"addresses",
			"1 minute",
		)
		.await;
		record_undeclared(
			&mut conn,
			machine,
			"ask-c.fiji.tamanu.app",
			"certificate",
			"1 minute",
		)
		.await;
		private
			.post("/api/dns_names/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": "no-a.fiji.tamanu.app"}))
			.await
			.assert_status_ok();
		private
			.post("/api/certificates/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": "no-c.fiji.tamanu.app"}))
			.await
			.assert_status_ok();

		let names = |view: &serde_json::Value, key: &str, field: &str| -> Vec<String> {
			view[key]
				.as_array()
				.unwrap()
				.iter()
				.map(|row| row[field].as_str().unwrap().to_string())
				.collect()
		};

		let dns: serde_json::Value = private
			.post("/api/dns_names/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert_eq!(
			names(&dns, "declared", "name"),
			vec!["records.fiji.tamanu.app"]
		);
		assert_eq!(
			names(&dns, "undeclared", "name"),
			vec!["ask-a.fiji.tamanu.app"]
		);
		assert_eq!(names(&dns, "denied", "name"), vec!["no-a.fiji.tamanu.app"]);
		assert!(
			dns["declared"][0]["published"].is_null(),
			"declared with no addresses registered has nothing published: {dns}"
		);
		assert!(dns["declared"][0]["certificate"].is_null());

		let tls: serde_json::Value = private
			.post("/api/certificates/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert_eq!(names(&tls, "declared", "name"), vec!["tls.fiji.tamanu.app"]);
		assert_eq!(
			names(&tls, "undeclared", "name"),
			vec!["ask-c.fiji.tamanu.app"]
		);
		assert_eq!(names(&tls, "denied", "name"), vec!["no-c.fiji.tamanu.app"]);
		assert!(tls["declared"][0]["published"].is_null());
		assert!(
			tls["declared"][0]["certificate"].is_null(),
			"declared, none held yet"
		);
	})
	.await
}

/// Denying one kind leaves the other allowed, and lifting is of the one kind.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn denying_one_kind_leaves_the_other_allowed() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, _, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		let name = "old.fiji.tamanu.app";

		let denied: serde_json::Value = private
			.post("/api/dns_names/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": name, "note": "retired"}))
			.await
			.json();
		assert_eq!(denied["note"], "retired");

		let tls: serde_json::Value = private
			.post("/api/certificates/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert!(tls["denied"].as_array().unwrap().is_empty());

		private
			.post("/api/certificates/lift_denial")
			.json(&serde_json::json!({"machine_id": machine, "name": name}))
			.await
			.assert_status(StatusCode::NOT_FOUND);
		private
			.post("/api/dns_names/lift_denial")
			.json(&serde_json::json!({"machine_id": machine, "name": name}))
			.await
			.assert_status_ok();
	})
	.await
}

/// Declaring ends the undeclared record and the denial of the kind declared, and
/// only that kind's.
// spec: DNS#denied-dns-names
// spec: DNS#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn declaring_ends_only_the_declared_kinds_record_and_denial() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, tamanu, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		let name = "central.fiji.tamanu.app";
		record_undeclared(&mut conn, machine, name, "addresses", "1 minute").await;
		record_undeclared(&mut conn, machine, name, "certificate", "1 minute").await;
		private
			.post("/api/dns_names/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": "x.fiji.tamanu.app"}))
			.await
			.assert_status_ok();
		private
			.post("/api/certificates/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": "x.fiji.tamanu.app"}))
			.await
			.assert_status_ok();

		private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({"application_id": tamanu, "name": name}))
			.await
			.assert_status_ok();
		private
			.post("/api/certificates/declare")
			.json(&serde_json::json!({"application_id": tamanu, "name": "x.fiji.tamanu.app"}))
			.await
			.assert_status_ok();

		let dns: serde_json::Value = private
			.post("/api/dns_names/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert!(dns["undeclared"].as_array().unwrap().is_empty(), "{dns}");
		assert_eq!(
			dns["denied"][0]["name"], "x.fiji.tamanu.app",
			"the address denial of the name declared for certificates stands: {dns}"
		);

		let tls: serde_json::Value = private
			.post("/api/certificates/for_machine")
			.json(&serde_json::json!({"machine_id": machine}))
			.await
			.json();
		assert_eq!(
			tls["undeclared"][0]["name"], name,
			"the certificate record of the name declared for addresses stands: {tls}"
		);
		assert!(tls["denied"].as_array().unwrap().is_empty(), "{tls}");
	})
	.await
}

/// Denying contradicts a declaration of the same kind on the same box, which has
/// to go first, but not one of the other kind.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn denying_a_name_declared_for_that_kind_on_the_machine_is_refused() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (machine, tamanu, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		let name = "central.fiji.tamanu.app";
		private
			.post("/api/dns_names/declare")
			.json(&serde_json::json!({"application_id": tamanu, "name": name}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/dns_names/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": name}))
			.await;
		resp.assert_status(StatusCode::CONFLICT);
		let body: serde_json::Value = resp.json();
		assert!(
			body["title"]
				.as_str()
				.unwrap_or_default()
				.contains("central"),
			"the refusal names the declaring application: {body}"
		);

		private
			.post("/api/certificates/deny")
			.json(&serde_json::json!({"machine_id": machine, "name": name}))
			.await
			.assert_status_ok();
	})
	.await
}

/// Declaring a name outside the group's domains is allowed and flagged, since
/// nothing can be published for it until the group claims one.
// spec: DNS#declared-dns-names
// spec: DNS#on-an-application
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_outside_the_group_domains_is_flagged() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (_, tamanu, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;

		for path in ["/api/dns_names/declare", "/api/certificates/declare"] {
			let outside: serde_json::Value = private
				.post(path)
				.json(
					&serde_json::json!({"application_id": tamanu, "name": "central.samoa.tamanu.app"}),
				)
				.await
				.json();
			assert_eq!(outside["within_domains"], false, "{path}");

			let inside: serde_json::Value = private
				.post(path)
				.json(
					&serde_json::json!({"application_id": tamanu, "name": "central.fiji.tamanu.app"}),
				)
				.await
				.json();
			assert_eq!(inside["within_domains"], true, "{path}");
		}
	})
	.await
}

/// Notices count each machine's requests that still count, of each kind,
/// fleet-wide or within one group; a request not repeated for a day no longer
/// counts.
// spec: DNS#notices
// spec: DNS#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn notices_count_live_undeclared_requests_of_each_kind() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (first, _, _) = shared_box_in_group(&mut conn, "fiji.tamanu.app").await;
		let (second, _, _) = shared_box_in_group(&mut conn, "samoa.tamanu.app").await;
		record_undeclared(
			&mut conn,
			first,
			"a.fiji.tamanu.app",
			"certificate",
			"1 minute",
		)
		.await;
		record_undeclared(
			&mut conn,
			first,
			"a.fiji.tamanu.app",
			"addresses",
			"1 minute",
		)
		.await;
		record_undeclared(
			&mut conn,
			first,
			"b.fiji.tamanu.app",
			"certificate",
			"1 hour",
		)
		.await;
		record_undeclared(
			&mut conn,
			first,
			"stale.fiji.tamanu.app",
			"certificate",
			"25 hours",
		)
		.await;
		record_undeclared(
			&mut conn,
			second,
			"a.samoa.tamanu.app",
			"addresses",
			"1 minute",
		)
		.await;

		let all: serde_json::Value = private
			.post("/api/dns_names/undeclared_notices")
			.json(&serde_json::json!({}))
			.await
			.json();
		let mut counts: Vec<(String, i64, i64)> = all
			.as_array()
			.unwrap()
			.iter()
			.map(|n| {
				(
					n["machine_id"].as_str().unwrap().to_string(),
					n["addresses"].as_i64().unwrap(),
					n["certificates"].as_i64().unwrap(),
				)
			})
			.collect();
		counts.sort();
		let mut expected = vec![(first.to_string(), 1, 2), (second.to_string(), 1, 0)];
		expected.sort();
		assert_eq!(counts, expected);

		let second_group = all
			.as_array()
			.unwrap()
			.iter()
			.find(|n| n["machine_id"] == second.to_string())
			.map(|n| n["group_id"].clone())
			.expect("the second machine's group");
		let one: serde_json::Value = private
			.post("/api/dns_names/undeclared_notices")
			.json(&serde_json::json!({"server_group_id": second_group}))
			.await
			.json();
		assert_eq!(one.as_array().unwrap().len(), 1);
		assert_eq!(one[0]["machine_id"], second.to_string());
		assert_eq!(one[0]["machine_name"], "box");
	})
	.await
}

/// A pause is of the application, not of either kind, and shows in both
/// sections' reads.
// spec: DNS#pausing-an-application
#[tokio::test(flavor = "multi_thread")]
async fn a_pause_shows_in_both_sections() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let (front, _) = two_workloads_on_a_box(&mut conn).await;

		private
			.post("/api/dns_names/pause")
			.json(&serde_json::json!({"server_id": front, "reason": "looking into it"}))
			.await
			.assert_status_ok();

		for path in ["/api/dns_names/for_server", "/api/certificates/for_server"] {
			let view: serde_json::Value = private
				.post(path)
				.json(&serde_json::json!({"server_id": front}))
				.await
				.json();
			assert_eq!(view["pause"]["reason"], "looking into it", "{path}");
			assert_eq!(view["pause"]["paused_by"], "admin@localhost", "{path}");
		}

		private
			.post("/api/dns_names/resume")
			.json(&serde_json::json!({"server_id": front}))
			.await
			.assert_status_ok();
		let view: serde_json::Value = private
			.post("/api/certificates/for_server")
			.json(&serde_json::json!({"server_id": front}))
			.await
			.json();
		assert!(view["pause"].is_null());
	})
	.await
}
