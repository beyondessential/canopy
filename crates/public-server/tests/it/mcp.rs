//! The internet-facing MCP mount at `/mcp` is gated by bearer tokens: no
//! token, a malformed header, an unknown token, and a revoked token all get
//! the same opaque 401 with a `WWW-Authenticate: Bearer` challenge; a minted
//! token calls tools; hammering failures trips the per-IP rate limit.

use database::mcp_tokens::McpToken;

/// Clients must accept both JSON and SSE on the POST leg.
const ACCEPT: &str = "application/json, text/event-stream";
/// Required on every non-initialize request in stateless mode.
const PROTO: &str = "2025-06-18";

fn fleet_summary_call() -> serde_json::Value {
	serde_json::json!({
		"jsonrpc": "2.0", "id": 1, "method": "tools/call",
		"params": { "name": "fleet_summary", "arguments": {} }
	})
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_without_a_usable_token() {
	commons_tests::server::run(async |mut conn, public, _private| {
		// No Authorization header at all.
		let bare = public
			.post("/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&fleet_summary_call())
			.await;
		assert_eq!(bare.status_code().as_u16(), 401);
		assert_eq!(
			bare.headers()
				.get("www-authenticate")
				.and_then(|v| v.to_str().ok()),
			Some("Bearer"),
		);

		// Wrong scheme.
		let basic = public
			.post("/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.add_header("authorization", "Basic dXNlcjpwYXNz")
			.json(&fleet_summary_call())
			.await;
		assert_eq!(basic.status_code().as_u16(), 401);

		// Well-formed but unknown token.
		let unknown = public
			.post("/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.add_header("authorization", "Bearer canopy_mcp_definitely-not-minted")
			.json(&fleet_summary_call())
			.await;
		assert_eq!(unknown.status_code().as_u16(), 401);

		// Revoked token.
		let (token, plaintext) = McpToken::mint(&mut conn, "revoked", "test@example.com", &[])
			.await
			.expect("mint");
		McpToken::revoke(&mut conn, token.id).await.expect("revoke");
		let revoked = public
			.post("/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.add_header("authorization", format!("Bearer {plaintext}"))
			.json(&fleet_summary_call())
			.await;
		assert_eq!(revoked.status_code().as_u16(), 401);

		// The refusals are uniform: same status, same problem type.
		assert_eq!(unknown.text(), revoked.text());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn minted_token_calls_tools_and_records_use() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let (token, plaintext) = McpToken::mint(&mut conn, "claude", "test@example.com", &[])
			.await
			.expect("mint");
		assert!(token.last_used_at.is_none());

		let resp = public
			.post("/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.add_header("authorization", format!("Bearer {plaintext}"))
			.json(&fleet_summary_call())
			.await;
		assert_eq!(resp.status_code().as_u16(), 200, "body: {}", resp.text());
		let env: serde_json::Value = serde_json::from_str(&resp.text()).expect("json body");
		assert!(env.get("error").is_none(), "rpc error: {env}");
		let result = &env["result"];
		assert_ne!(result.get("isError").and_then(|v| v.as_bool()), Some(true));
		assert!(result["structuredContent"]["total_servers"].is_u64());

		let listed = McpToken::list(&mut conn).await.expect("list");
		assert!(
			listed
				.iter()
				.any(|t| t.id == token.id && t.last_used_at.is_some()),
			"successful call must stamp last_used_at"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn failed_attempts_rate_limit_per_ip() {
	commons_tests::server::run(async |_conn, public, _private| {
		// The failed-auth budget is 30/minute per IP; the 401s must flip to
		// 429 before 40 attempts.
		let mut saw_429 = false;
		for _ in 0..40 {
			let resp = public
				.post("/mcp")
				.add_header("accept", ACCEPT)
				.add_header("mcp-protocol-version", PROTO)
				.add_header("authorization", "Bearer canopy_mcp_guess")
				.json(&fleet_summary_call())
				.await;
			match resp.status_code().as_u16() {
				401 => continue,
				429 => {
					saw_429 = true;
					break;
				}
				other => panic!("unexpected status {other}"),
			}
		}
		assert!(saw_429, "failed attempts never rate limited");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn oauth_discovery_is_404() {
	// MCP clients probe for OAuth authorization-server metadata; the public
	// mount must 404 those so clients fall back to plain bearer auth.
	commons_tests::server::run(async |_conn, public, _private| {
		for path in [
			"/.well-known/oauth-authorization-server",
			"/.well-known/oauth-protected-resource",
		] {
			let resp = public.get(path).await;
			assert_eq!(resp.status_code().as_u16(), 404, "{path}");
		}
	})
	.await
}

/// The budget is documented as bounding "the DB lookups a token-guesser can
/// burn", but it was only consulted inside `refuse` — after
/// `McpToken::find_active` had already taken a pool connection and run its
/// query. So an over-budget IP still cost a lookup per guess.
///
/// Proved by making the lookup impossible: with the table gone, any code
/// path that still reaches the database errors into a 500. A 429 means the
/// request was turned away before it got there.
#[tokio::test(flavor = "multi_thread")]
async fn an_over_budget_ip_is_refused_without_a_database_lookup() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let guess = async || {
			public
				.post("/mcp")
				.add_header("accept", ACCEPT)
				.add_header("mcp-protocol-version", PROTO)
				.add_header("authorization", "Bearer canopy_mcp_guess")
				.json(&fleet_summary_call())
				.await
				.status_code()
				.as_u16()
		};

		// Spend the budget.
		let mut spent = false;
		for _ in 0..40 {
			if guess().await == 429 {
				spent = true;
				break;
			}
		}
		assert!(spent, "failed attempts never rate limited");

		diesel_async::SimpleAsyncConnection::batch_execute(&mut conn, "DROP TABLE mcp_tokens")
			.await
			.expect("drop the token table");

		assert_eq!(
			guess().await,
			429,
			"an over-budget IP must be refused before the lookup, not after it",
		);
	})
	.await
}

// --- vulnerability triage over the token path ------------------------------

mod triage {
	use super::*;
	use database::{
		audit::{self, Actor, Filter},
		osv::{Counted, Record},
		vulnerabilities::{Advisory, replace_matches},
	};
	use diesel_async::AsyncPgConnection;

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
			&Counted {
				ecosystem: "npm".into(),
				name: "left-pad".into(),
				version: "1.0.0".into(),
			},
			&["GHSA-1".into()],
		)
		.await
		.unwrap();
	}

	fn decide_call() -> serde_json::Value {
		serde_json::json!({
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

	async fn decisions(conn: &mut AsyncPgConnection) -> Vec<audit::Entry> {
		audit::find(
			conn,
			&Filter {
				action: Some("vulnerability_decided".to_owned()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap()
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_token_without_the_triage_grant_is_refused_with_no_effect() {
		commons_tests::server::run(async |mut conn, public, _private| {
			an_item(&mut conn).await;
			let (_, plaintext) = McpToken::mint(&mut conn, "reader", "test@example.com", &[])
				.await
				.unwrap();

			let resp = public
				.post("/mcp")
				.add_header("accept", ACCEPT)
				.add_header("mcp-protocol-version", PROTO)
				.add_header("authorization", format!("Bearer {plaintext}"))
				.json(&decide_call())
				.await;
			let env: serde_json::Value = serde_json::from_str(&resp.text()).expect("json body");
			assert!(env.get("error").is_some(), "refused: {env}");
			assert!(decisions(&mut conn).await.is_empty());

			let read = public
				.post("/mcp")
				.add_header("accept", ACCEPT)
				.add_header("mcp-protocol-version", PROTO)
				.add_header("authorization", format!("Bearer {plaintext}"))
				.json(&serde_json::json!({
					"jsonrpc": "2.0", "id": 3, "method": "tools/call",
					"params": {
						"name": "get_vulnerability",
						"arguments": {"id": "GHSA-1", "source": "tamanu"},
					},
				}))
				.await;
			let env: serde_json::Value = serde_json::from_str(&read.text()).expect("json body");
			assert_eq!(
				env["result"]["structuredContent"]["item"]["status"], "open",
				"a read-only token still reads, and the item is still open: {env}"
			);
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_token_with_the_grant_records_a_decision_attributed_to_it() {
		commons_tests::server::run(async |mut conn, public, _private| {
			an_item(&mut conn).await;
			let (token, plaintext) = McpToken::mint(
				&mut conn,
				"triager",
				"test@example.com",
				&["triage".to_owned()],
			)
			.await
			.unwrap();

			let resp = public
				.post("/mcp")
				.add_header("accept", ACCEPT)
				.add_header("mcp-protocol-version", PROTO)
				.add_header("authorization", format!("Bearer {plaintext}"))
				.json(&decide_call())
				.await;
			let env: serde_json::Value = serde_json::from_str(&resp.text()).expect("json body");
			assert!(env.get("error").is_none(), "{env}");
			let decision = &env["result"]["structuredContent"];
			assert_eq!(decision["outcome"], "not_affected");
			assert_eq!(decision["via"], "mcp");
			assert_eq!(decision["actor"]["kind"], "token");
			assert_eq!(decision["actor"]["name"], "triager");

			let log = decisions(&mut conn).await;
			assert_eq!(log.len(), 1);
			assert!(
				matches!(&log[0].actor, Actor::Token { id, name } if *id == token.id && name == "triager")
			);
			assert_eq!(log[0].detail["via"], "mcp");
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_token_may_replace_an_operators_decision_and_the_history_keeps_both() {
		use database::vulnerabilities::{Decision, NewDecision, Outcome, Source, Via, today};
		commons_tests::server::run(async |mut conn, public, _private| {
			an_item(&mut conn).await;
			Decision::record(
				&mut conn,
				NewDecision {
					vulnerability_id: "GHSA-1".into(),
					source: Source::Tamanu,
					outcome: Outcome::WillFix,
					reason: None,
					review_by: None,
					issue_url: None,
					ground: None,
					actor: Actor::operator("admin@example.com", None),
					via: Via::Ui,
				},
				today(),
			)
			.await
			.unwrap();
			let (_, plaintext) = McpToken::mint(
				&mut conn,
				"triager",
				"test@example.com",
				&["triage".to_owned()],
			)
			.await
			.unwrap();
			public
				.post("/mcp")
				.add_header("accept", ACCEPT)
				.add_header("mcp-protocol-version", PROTO)
				.add_header("authorization", format!("Bearer {plaintext}"))
				.json(&decide_call())
				.await
				.assert_status_ok();

			let history = Decision::history(&mut conn, "GHSA-1", Source::Tamanu)
				.await
				.unwrap();
			assert_eq!(history.len(), 2);
			assert_eq!(history[0].via, Via::Mcp);
			assert_eq!(history[1].via, Via::Ui);
		})
		.await
	}
}
