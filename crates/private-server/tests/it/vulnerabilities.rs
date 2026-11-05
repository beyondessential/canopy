//! Recording a triage decision in the operator interface (specs `VTR`, `SAFE`):
//! an admin action, graded write, audited as the operator's.

use database::diesel_async::AsyncPgConnection;
use database::{
	admins::Admin,
	audit::{self, Actor, Filter},
	osv::{Counted, Record},
	vulnerabilities::{Advisory, replace_matches},
};
use serde_json::json;

const TRUST_HEADERS: &str = "CANOPY_TRUST_TAILSCALE_HEADERS";
const SESSION_HEADER: &str = "x-canopy-session";
const ADMIN: &str = "admin@example.com";
const NOT_ADMIN: &str = "someone@example.com";

fn trust_headers() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var(TRUST_HEADERS, "1") };
}

async fn an_item(conn: &mut AsyncPgConnection) {
	Advisory::upsert(
		conn,
		&Record {
			id: "GHSA-1".into(),
			summary: "left-pad prototype pollution".into(),
			modified: "2026-10-01T00:00:00Z".into(),
			..Default::default()
		},
	)
	.await
	.unwrap();
	replace_matches(
		conn,
		&[(
			Counted {
				ecosystem: "npm".into(),
				name: "left-pad".into(),
				version: "1.0.0".into(),
			},
			vec!["GHSA-1".into()],
		)],
	)
	.await
	.unwrap();
}

fn decision() -> serde_json::Value {
	json!({
		"id": "GHSA-1",
		"source": "tamanu",
		"outcome": "accepted_risk",
		"reason": "Only reachable by admins",
		"review_by": "2099-01-01",
	})
}

#[tokio::test(flavor = "multi_thread")]
async fn an_admin_records_a_decision_and_it_is_audited_as_theirs() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;
		Admin::add(&mut conn, ADMIN).await.expect("add admin");

		let session = private
			.post("/api/safety/session")
			.add_header("Tailscale-User-Login", ADMIN)
			.add_header("Tailscale-User-Name", "Admin")
			.json(&json!({}))
			.await;
		session.assert_status_ok();
		let session: serde_json::Value = session.json();
		let id = session["id"].as_str().expect("session id").to_owned();
		private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", ADMIN)
			.add_header("Tailscale-User-Name", "Admin")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "write"}))
			.await
			.assert_status_ok();

		let decided = private
			.post("/api/vulnerabilities/decide")
			.add_header("Tailscale-User-Login", ADMIN)
			.add_header("Tailscale-User-Name", "Admin")
			.add_header(SESSION_HEADER, &id)
			.json(&decision())
			.await;
		decided.assert_status_ok();
		let decided: serde_json::Value = decided.json();
		assert_eq!(decided["outcome"], "accepted_risk");
		assert_eq!(decided["via"], "ui");

		let detail = private
			.post("/api/vulnerabilities/get")
			.add_header("Tailscale-User-Login", NOT_ADMIN)
			.add_header("Tailscale-User-Name", "Someone")
			.json(&json!({"id": "GHSA-1", "source": "tamanu"}))
			.await;
		detail.assert_status_ok();
		let detail: serde_json::Value = detail.json();
		assert_eq!(detail["item"]["status"], "accepted_risk");
		assert_eq!(detail["history"][0]["kind"], "decision");

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(log.len(), 1);
		assert!(matches!(&log[0].actor, Actor::Operator { login, .. } if login == ADMIN));
		assert_eq!(log[0].records[0].id, "GHSA-1");
		assert_eq!(log[0].detail["outcome"], "accepted_risk");
		assert_eq!(log[0].detail["via"], "ui");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_admin_cannot_decide_but_can_read() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;

		private
			.post("/api/vulnerabilities/decide")
			.add_header("Tailscale-User-Login", NOT_ADMIN)
			.add_header("Tailscale-User-Name", "Someone")
			.json(&decision())
			.await
			.assert_status_forbidden();

		let listing = private
			.post("/api/vulnerabilities/list")
			.add_header("Tailscale-User-Login", NOT_ADMIN)
			.add_header("Tailscale-User-Name", "Someone")
			.json(&json!({"include_uncounted": true}))
			.await;
		listing.assert_status_ok();
		let listing: serde_json::Value = listing.json();
		assert_eq!(listing["items"][0]["id"], "GHSA-1");
		assert_eq!(listing["items"][0]["status"], "open");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_is_refused_a_decision() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;
		Admin::add(&mut conn, ADMIN).await.expect("add admin");

		let session = private
			.post("/api/safety/session")
			.add_header("Tailscale-User-Login", ADMIN)
			.add_header("Tailscale-User-Name", "Admin")
			.json(&json!({}))
			.await;
		session.assert_status_ok();
		let session: serde_json::Value = session.json();
		let id = session["id"].as_str().expect("session id").to_owned();

		private
			.post("/api/vulnerabilities/decide")
			.add_header("Tailscale-User-Login", ADMIN)
			.add_header("Tailscale-User-Name", "Admin")
			.add_header(SESSION_HEADER, &id)
			.json(&decision())
			.await
			.assert_status_forbidden();

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert!(log.is_empty(), "a refused request records nothing");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_needs_what_its_outcome_requires() {
	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;

		private
			.post("/api/vulnerabilities/decide")
			.json(&json!({"id": "GHSA-1", "source": "tamanu", "outcome": "not_affected"}))
			.await
			.assert_status_bad_request();
		private
			.post("/api/vulnerabilities/decide")
			.json(&json!({
				"id": "GHSA-1", "source": "tamanu", "outcome": "accepted_risk",
				"reason": "fine", "review_by": "2000-01-01",
			}))
			.await
			.assert_status_bad_request();
		private
			.post("/api/vulnerabilities/decide")
			.json(&json!({"id": "GHSA-none", "source": "tamanu", "outcome": "will_fix"}))
			.await
			.assert_status_not_found();
	})
	.await;
}

// --- the same decision through the fleet query interface ---------------------

const ACCEPT: &str = "application/json, text/event-stream";
const PROTO: &str = "2025-06-18";

fn mcp_decision() -> serde_json::Value {
	json!({
		"jsonrpc": "2.0", "id": 2, "method": "tools/call",
		"params": {
			"name": "record_vulnerability_decision",
			"arguments": {
				"id": "GHSA-1", "source": "tamanu",
				"outcome": "not_affected", "reason": "Never loaded on the server",
			},
		},
	})
}

#[tokio::test(flavor = "multi_thread")]
async fn an_administrator_records_a_decision_through_the_query_interface() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;
		Admin::add(&mut conn, ADMIN).await.expect("add admin");

		let resp = private
			.post("/api/mcp")
			.add_header("Tailscale-User-Login", ADMIN)
			.add_header("Tailscale-User-Name", "Admin")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&mcp_decision())
			.await;
		let env: serde_json::Value = serde_json::from_str(&resp.text()).expect("json body");
		assert!(env.get("error").is_none(), "{env}");
		let decision = &env["result"]["structuredContent"];
		assert_eq!(decision["via"], "mcp");
		assert_eq!(decision["actor"]["kind"], "operator");
		assert_eq!(decision["actor"]["login"], ADMIN);

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].detail["via"], "mcp");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_tailnet_user_who_is_not_an_administrator_is_refused_with_no_effect() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;

		let resp = private
			.post("/api/mcp")
			.add_header("Tailscale-User-Login", NOT_ADMIN)
			.add_header("Tailscale-User-Name", "Someone")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&mcp_decision())
			.await;
		let env: serde_json::Value = serde_json::from_str(&resp.text()).expect("json body");
		assert!(env.get("error").is_some(), "{env}");

		let log = audit::find(
			&mut conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert!(log.is_empty());
	})
	.await;
}
