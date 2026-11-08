//! Triaging vulnerabilities in the operator interface (specs `VUL`, `VTR`,
//! `SAFE`): deciding, splitting, withdrawing and editing are admin actions,
//! graded write, audited as the operator's; the list pages, searches and counts.

use commons_tests::axum_test::TestServer;
use database::diesel_async::AsyncPgConnection;
use database::{
	admins::Admin,
	audit::{self, Actor, Filter},
	osv::{Counted, Record},
	packages::{PackagePath, READER_VERSION, RepositoryPackage, RepositoryRead, Scope},
	vulnerabilities::{Advisory, refresh_items, replace_matches},
};
use serde_json::{Value, json};

const TRUST_HEADERS: &str = "CANOPY_TRUST_TAILSCALE_HEADERS";
const SESSION_HEADER: &str = "x-canopy-session";
const ADMIN: &str = "admin@example.com";
const NOT_ADMIN: &str = "someone@example.com";

fn trust_headers() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var(TRUST_HEADERS, "1") };
}

/// Tamanu carrying left-pad, reached by its `server` and `web` workspace
/// packages, with an advisory against it.
async fn an_item(conn: &mut AsyncPgConnection) {
	let path = |workspace: &str| PackagePath {
		workspace_package: workspace.into(),
		scope: Scope::Runtime,
		optional: false,
		direct: true,
		chain: vec!["left-pad".into()],
	};
	RepositoryRead::store(
		conn,
		"tamanu",
		"abc",
		READER_VERSION,
		&[RepositoryPackage {
			ecosystem: "npm".into(),
			name: "left-pad".into(),
			version: "1.0.0".into(),
			paths: vec![path("server"), path("web")],
		}],
	)
	.await
	.unwrap();
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
	refresh_items(conn).await.unwrap();
}

fn whole() -> Value {
	json!({
		"id": "GHSA-1",
		"source": "tamanu",
		"entries": [{ "outcome": "accepted_risk", "reason": "Only reachable by admins" }],
	})
}

fn split() -> Value {
	json!({
		"id": "GHSA-1",
		"source": "tamanu",
		"entries": [
			{ "workspace_packages": ["web"], "outcome": "will_fix", "issue_url": "https://example.org/issues/1" },
			{ "outcome": "not_affected", "reason": "Server never loads it" },
		],
	})
}

/// A session of the admin raised to write.
async fn writing(private: &TestServer) -> String {
	let session = private
		.post("/api/safety/session")
		.add_header("Tailscale-User-Login", ADMIN)
		.add_header("Tailscale-User-Name", "Admin")
		.json(&json!({}))
		.await;
	session.assert_status_ok();
	let session: Value = session.json();
	let id = session["id"].as_str().expect("session id").to_owned();
	private
		.post("/api/safety/raise")
		.add_header("Tailscale-User-Login", ADMIN)
		.add_header("Tailscale-User-Name", "Admin")
		.add_header(SESSION_HEADER, &id)
		.json(&json!({"mode": "write"}))
		.await
		.assert_status_ok();
	id
}

async fn admin_post(private: &TestServer, session: &str, path: &str, body: &Value) -> Value {
	let response = private
		.post(path)
		.add_header("Tailscale-User-Login", ADMIN)
		.add_header("Tailscale-User-Name", "Admin")
		.add_header(SESSION_HEADER, session)
		.json(body)
		.await;
	response.assert_status_ok();
	response.json()
}

async fn logged(conn: &mut AsyncPgConnection, action: &str) -> Vec<audit::Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some(action.to_owned()),
			limit: 10,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

async fn read(private: &TestServer, path: &str, body: &Value) -> Value {
	let response = private
		.post(path)
		.add_header("Tailscale-User-Login", NOT_ADMIN)
		.add_header("Tailscale-User-Name", "Someone")
		.json(body)
		.await;
	response.assert_status_ok();
	response.json()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_admin_records_a_split_decision_and_it_is_audited_as_theirs() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;
		Admin::add(&mut conn, ADMIN).await.expect("add admin");
		let session = writing(&private).await;

		let decided = admin_post(&private, &session, "/api/vulnerabilities/decide", &split()).await;
		assert_eq!(decided["via"], "ui");
		assert_eq!(decided["status"], "mixed");
		assert_eq!(decided["entries"].as_array().unwrap().len(), 2);

		let detail = read(
			&private,
			"/api/vulnerabilities/get",
			&json!({"id": "GHSA-1", "source": "tamanu"}),
		)
		.await;
		assert_eq!(detail["item"]["status"], "mixed");
		assert_eq!(detail["history"][0]["kind"], "decision");
		assert_eq!(detail["commit"], "abc");

		let log = logged(&mut conn, "vulnerability_decided").await;
		assert_eq!(log.len(), 1);
		assert!(matches!(&log[0].actor, Actor::Operator { login, .. } if login == ADMIN));
		assert_eq!(log[0].records[0].id, "GHSA-1");
		assert_eq!(log[0].detail["entries"][0]["workspace_packages"][0], "web");
		assert_eq!(log[0].detail["via"], "ui");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_and_editing_are_audited_and_show_in_the_history() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;
		Admin::add(&mut conn, ADMIN).await.expect("add admin");
		let session = writing(&private).await;
		admin_post(&private, &session, "/api/vulnerabilities/decide", &whole()).await;

		let edit = admin_post(
			&private,
			&session,
			"/api/vulnerabilities/edit",
			&json!({"id": "GHSA-1", "source": "tamanu", "reason": "Only reachable by admins on the VPN"}),
		)
		.await;
		assert_eq!(edit["earlier_reason"], "Only reachable by admins");

		let item = read(
			&private,
			"/api/vulnerabilities/list",
			&json!({"status": "accepted_risk"}),
		)
		.await;
		assert_eq!(item["items"][0]["edited"], true);

		admin_post(
			&private,
			&session,
			"/api/vulnerabilities/withdraw",
			&json!({"id": "GHSA-1", "source": "tamanu", "reason": "Needs another look"}),
		)
		.await;
		let after = read(
			&private,
			"/api/vulnerabilities/list",
			&json!({"status": "open"}),
		)
		.await;
		assert_eq!(after["items"][0]["withdrawn"], true);
		assert_eq!(after["total"], 1);

		let detail = read(
			&private,
			"/api/vulnerabilities/get",
			&json!({"id": "GHSA-1", "source": "tamanu"}),
		)
		.await;
		let kinds: Vec<&str> = detail["history"]
			.as_array()
			.unwrap()
			.iter()
			.map(|entry| entry["kind"].as_str().unwrap())
			.collect();
		assert_eq!(kinds, ["withdrawal", "edit", "decision"]);

		for (action, detail_key) in [
			("vulnerability_decision_edited", "earlier_reason"),
			("vulnerability_decision_withdrawn", "reason"),
		] {
			let log = logged(&mut conn, action).await;
			assert_eq!(log.len(), 1, "{action}");
			assert!(matches!(&log[0].actor, Actor::Operator { login, .. } if login == ADMIN));
			assert!(!log[0].detail[detail_key].is_null());
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_non_admin_cannot_change_a_decision_but_can_read() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;

		for (path, body) in [
			("/api/vulnerabilities/decide", whole()),
			(
				"/api/vulnerabilities/withdraw",
				json!({"id": "GHSA-1", "source": "tamanu", "reason": "x"}),
			),
			(
				"/api/vulnerabilities/edit",
				json!({"id": "GHSA-1", "source": "tamanu", "reason": "x"}),
			),
		] {
			private
				.post(path)
				.add_header("Tailscale-User-Login", NOT_ADMIN)
				.add_header("Tailscale-User-Name", "Someone")
				.json(&body)
				.await
				.assert_status_forbidden();
		}

		let listing = read(&private, "/api/vulnerabilities/list", &json!({})).await;
		assert_eq!(listing["items"][0]["id"], "GHSA-1");
		assert_eq!(listing["items"][0]["status"], "open");
		let count = read(&private, "/api/vulnerabilities/count", &json!({})).await;
		assert_eq!(count["needs_triage"], 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_is_refused_all_three_changes() {
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
		let session: Value = session.json();
		let id = session["id"].as_str().expect("session id").to_owned();

		for (path, body) in [
			("/api/vulnerabilities/decide", whole()),
			(
				"/api/vulnerabilities/withdraw",
				json!({"id": "GHSA-1", "source": "tamanu", "reason": "x"}),
			),
			(
				"/api/vulnerabilities/edit",
				json!({"id": "GHSA-1", "source": "tamanu", "reason": "x"}),
			),
		] {
			private
				.post(path)
				.add_header("Tailscale-User-Login", ADMIN)
				.add_header("Tailscale-User-Name", "Admin")
				.add_header(SESSION_HEADER, &id)
				.json(&body)
				.await
				.assert_status_forbidden();
		}

		for action in [
			"vulnerability_decided",
			"vulnerability_decision_withdrawn",
			"vulnerability_decision_edited",
		] {
			assert!(
				logged(&mut conn, action).await.is_empty(),
				"a refused request records nothing"
			);
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_decision_needs_what_its_outcome_requires_and_covers_every_workspace_package() {
	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;

		let refused = |entries: Value| {
			let private = &private;
			async move {
				private
					.post("/api/vulnerabilities/decide")
					.json(&json!({"id": "GHSA-1", "source": "tamanu", "entries": entries}))
					.await
					.assert_status_bad_request();
			}
		};
		refused(json!([{ "outcome": "not_affected" }])).await;
		refused(json!([{ "workspace_packages": ["web"], "outcome": "will_fix" }])).await;
		refused(json!([
			{ "workspace_packages": ["desktop"], "outcome": "will_fix" },
			{ "outcome": "will_fix" },
		]))
		.await;
		refused(json!([])).await;
		private
			.post("/api/vulnerabilities/decide")
			.json(
				&json!({"id": "GHSA-none", "source": "tamanu", "entries": [{ "outcome": "will_fix" }]}),
			)
			.await
			.assert_status_not_found();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_list_pages_searches_and_counts_its_facets() {
	commons_tests::server::run(async |mut conn, _public, private| {
		an_item(&mut conn).await;

		let found = read(
			&private,
			"/api/vulnerabilities/list",
			&json!({
				"search": "left-pad",
				"sort": { "column": "severity", "descending": true },
				"page_size": 1,
				"status": "open",
				"source": "tamanu",
			}),
		)
		.await;
		assert_eq!(found["total"], 1);
		assert!(found["next_cursor"].is_null());
		assert_eq!(found["items"][0]["packages"][0]["name"], "left-pad");
		let facets = &found["facets"];
		let count = |facet: &str, value: &str| {
			facets[facet]
				.as_array()
				.unwrap()
				.iter()
				.find(|choice| choice["value"] == value)
				.unwrap()["count"]
				.clone()
		};
		assert_eq!(count("source", "tamanu"), 1);
		assert_eq!(count("scope", "runtime"), 1);
		assert_eq!(facets["totals"]["scope"], 1);

		let later = read(
			&private,
			"/api/vulnerabilities/list",
			&json!({"cursor": "o0", "status": "open"}),
		)
		.await;
		assert!(
			later["facets"].is_null(),
			"a later page leaves the facets to the first"
		);

		let none = read(
			&private,
			"/api/vulnerabilities/list",
			&json!({"search": "nothing-like-it"}),
		)
		.await;
		assert_eq!(none["total"], 0);

		private
			.post("/api/vulnerabilities/list")
			.add_header("Tailscale-User-Login", NOT_ADMIN)
			.add_header("Tailscale-User-Name", "Someone")
			.json(&json!({"cursor": "junk"}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

// --- the same decisions through the fleet query interface --------------------

const ACCEPT: &str = "application/json, text/event-stream";
const PROTO: &str = "2025-06-18";

fn mcp_decision() -> Value {
	json!({
		"jsonrpc": "2.0", "id": 2, "method": "tools/call",
		"params": {
			"name": "record_vulnerability_decisions",
			"arguments": { "decisions": [{
				"id": "GHSA-1", "source": "tamanu",
				"entries": [{ "outcome": "not_affected", "reason": "Never loaded on the server" }],
			}] },
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
		let env: Value = serde_json::from_str(&resp.text()).expect("json body");
		assert!(env.get("error").is_none(), "{env}");
		let results = env["result"]["structuredContent"]["results"]
			.as_array()
			.unwrap_or_else(|| panic!("{env}"));
		assert_eq!(results[0]["recorded"], true, "{env}");

		let log = logged(&mut conn, "vulnerability_decided").await;
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].detail["via"], "mcp");
		assert!(matches!(&log[0].actor, Actor::Operator { login, .. } if login == ADMIN));
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
		let env: Value = serde_json::from_str(&resp.text()).expect("json body");
		assert!(env.get("error").is_some(), "{env}");
		assert!(logged(&mut conn, "vulnerability_decided").await.is_empty());
	})
	.await;
}
