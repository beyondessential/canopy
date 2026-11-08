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

// --- vulnerabilities over the token path -----------------------------------

mod vulnerabilities {
	use std::collections::BTreeSet;

	use axum_test::TestServer;
	use database::{
		audit::{self, Actor, Filter},
		osv::{Counted, Record},
		packages::{PackagePath, READER_VERSION, RepositoryPackage, RepositoryRead, Scope},
		vulnerabilities::{Advisory, refresh_items, replace_matches},
	};
	use diesel_async::AsyncPgConnection;
	use serde_json::{Value, json};

	use super::*;

	const DETAILS: &str = "A long account of how the prototype is polluted";

	fn path(workspace: &str) -> PackagePath {
		PackagePath {
			workspace_package: workspace.into(),
			scope: Scope::Runtime,
			optional: false,
			direct: true,
			chain: vec!["pkg".into()],
		}
	}

	fn package(name: &str, workspaces: &[&str]) -> RepositoryPackage {
		RepositoryPackage {
			ecosystem: "npm".into(),
			name: name.into(),
			version: "1.0.0".into(),
			paths: workspaces.iter().map(|w| path(w)).collect(),
		}
	}

	/// Three open items of `tamanu`: GHSA-1 and GHSA-2 against left-pad,
	/// reached by the `server` and `web` workspace packages, and GHSA-3 against
	/// lodash, reached by `server` alone.
	async fn items(conn: &mut AsyncPgConnection) {
		RepositoryRead::store(
			conn,
			"tamanu",
			"abc",
			READER_VERSION,
			&[
				package("left-pad", &["server", "web"]),
				package("lodash", &["server"]),
			],
		)
		.await
		.unwrap();
		for (id, name) in [
			("GHSA-1", "left-pad"),
			("GHSA-2", "left-pad"),
			("GHSA-3", "lodash"),
		] {
			Advisory::upsert(
				conn,
				&Record {
					id: id.into(),
					summary: format!("{id} in {name}"),
					details: DETAILS.into(),
					modified: "2026-10-01T00:00:00Z".into(),
					..Default::default()
				},
			)
			.await
			.unwrap();
		}
		let matched = |name: &str, ids: &[&str]| {
			(
				Counted {
					ecosystem: "npm".into(),
					name: name.into(),
					version: "1.0.0".into(),
				},
				ids.iter().map(|id| (*id).to_owned()).collect::<Vec<_>>(),
			)
		};
		replace_matches(
			conn,
			&[
				matched("left-pad", &["GHSA-1", "GHSA-2"]),
				matched("lodash", &["GHSA-3"]),
			],
		)
		.await
		.unwrap();
		refresh_items(conn).await.unwrap();
	}

	async fn rpc(public: &TestServer, token: &str, method: &str, params: Value) -> Value {
		let resp = public
			.post("/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.add_header("authorization", format!("Bearer {token}"))
			.json(&json!({ "jsonrpc": "2.0", "id": 2, "method": method, "params": params }))
			.await;
		serde_json::from_str(&resp.text()).expect("json body")
	}

	async fn call(public: &TestServer, token: &str, tool: &str, arguments: Value) -> Value {
		rpc(
			public,
			token,
			"tools/call",
			json!({ "name": tool, "arguments": arguments }),
		)
		.await
	}

	async fn granted(conn: &mut AsyncPgConnection) -> (McpToken, String) {
		McpToken::mint(conn, "triager", "test@example.com", &["triage".to_owned()])
			.await
			.unwrap()
	}

	fn whole(id: &str, outcome: &str, reason: Option<&str>) -> Value {
		json!({
			"id": id, "source": "tamanu",
			"entries": [{ "outcome": outcome, "reason": reason }],
		})
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

	async fn status_of(public: &TestServer, token: &str, id: &str) -> Value {
		let env = call(
			public,
			token,
			"get_vulnerability",
			json!({ "id": id, "source": "tamanu" }),
		)
		.await;
		env["result"]["structuredContent"]["item"]["status"].clone()
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_batch_records_the_valid_items_and_refuses_the_rest_in_order() {
		commons_tests::server::run(async |mut conn, public, _private| {
			items(&mut conn).await;
			let (token, plaintext) = granted(&mut conn).await;

			let env = call(
				&public,
				&plaintext,
				"record_vulnerability_decisions",
				json!({ "decisions": [
					whole("GHSA-1", "not_affected", Some("Never loaded on the server")),
					{
						"id": "GHSA-2", "source": "tamanu",
						"entries": [{
							"workspace_packages": ["server"],
							"outcome": "will_fix",
						}],
					},
					whole("GHSA-9", "not_affected", Some("No such item")),
					whole("GHSA-3", "will_fix", None),
					whole("GHSA-3", "mystery", None),
				] }),
			)
			.await;
			assert!(env.get("error").is_none(), "{env}");
			let batch = &env["result"]["structuredContent"];
			let results = batch["results"].as_array().unwrap();
			assert_eq!(
				results
					.iter()
					.map(|r| r["id"].as_str().unwrap())
					.collect::<Vec<_>>(),
				["GHSA-1", "GHSA-2", "GHSA-9", "GHSA-3", "GHSA-3"],
				"a result per item, in the order given"
			);
			assert_eq!(
				results
					.iter()
					.map(|r| r["recorded"].as_bool().unwrap())
					.collect::<Vec<_>>(),
				[true, false, false, true, false]
			);
			assert_eq!(batch["refused"], 3);
			assert!(
				results[1]["error"].as_str().unwrap().contains("web"),
				"the split that leaves web uncovered says so: {}",
				results[1]
			);
			assert!(results[2]["error"].as_str().unwrap().contains("GHSA-9"));
			assert!(results[4]["error"].as_str().unwrap().contains("mystery"));

			let decision = &results[0]["decision"];
			assert_eq!(decision["via"], "mcp");
			assert_eq!(decision["actor"]["kind"], "token");
			assert_eq!(decision["actor"]["name"], "triager");

			assert_eq!(
				status_of(&public, &plaintext, "GHSA-1").await,
				"not_affected"
			);
			assert_eq!(status_of(&public, &plaintext, "GHSA-2").await, "open");
			assert_eq!(status_of(&public, &plaintext, "GHSA-3").await, "will_fix");

			let log = logged(&mut conn, "vulnerability_decided").await;
			assert_eq!(log.len(), 2, "only the recorded items are audited");
			assert!(log.iter().all(|entry| {
				matches!(&entry.actor, Actor::Token { id, name } if *id == token.id && name == "triager")
					&& entry.detail["via"] == "mcp"
			}));
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_batch_over_the_limit_is_refused_whole() {
		commons_tests::server::run(async |mut conn, public, _private| {
			items(&mut conn).await;
			let (_, plaintext) = granted(&mut conn).await;

			let decisions: Vec<Value> = (0..101)
				.map(|_| whole("GHSA-1", "not_affected", Some("Never loaded")))
				.collect();
			let env = call(
				&public,
				&plaintext,
				"record_vulnerability_decisions",
				json!({ "decisions": decisions }),
			)
			.await;
			assert!(env.get("error").is_some(), "{env}");
			assert!(logged(&mut conn, "vulnerability_decided").await.is_empty());
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_token_without_the_triage_grant_is_refused_every_write_with_no_effect() {
		commons_tests::server::run(async |mut conn, public, _private| {
			items(&mut conn).await;
			let (_, plaintext) = McpToken::mint(&mut conn, "reader", "test@example.com", &[])
				.await
				.unwrap();

			for (tool, arguments) in [
				(
					"record_vulnerability_decisions",
					json!({ "decisions": [whole("GHSA-1", "not_affected", Some("Never loaded"))] }),
				),
				(
					"withdraw_vulnerability_decisions",
					json!({
						"items": [{ "id": "GHSA-1", "source": "tamanu" }],
						"reason": "Wrong",
					}),
				),
				(
					"edit_vulnerability_decision",
					json!({ "id": "GHSA-1", "source": "tamanu", "reason": "Better" }),
				),
			] {
				let env = call(&public, &plaintext, tool, arguments).await;
				assert!(env.get("error").is_some(), "{tool} refused: {env}");
			}
			for action in [
				"vulnerability_decided",
				"vulnerability_decision_withdrawn",
				"vulnerability_decision_edited",
			] {
				assert!(logged(&mut conn, action).await.is_empty(), "{action}");
			}
			assert_eq!(
				status_of(&public, &plaintext, "GHSA-1").await,
				"open",
				"a read-only token still reads, and the item is still open"
			);
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn a_granted_token_edits_and_withdraws_a_decision() {
		commons_tests::server::run(async |mut conn, public, _private| {
			items(&mut conn).await;
			let (token, plaintext) = granted(&mut conn).await;
			call(
				&public,
				&plaintext,
				"record_vulnerability_decisions",
				json!({ "decisions": [whole("GHSA-1", "accepted_risk", Some("Behind a firewall"))] }),
			)
			.await;

			let edit = call(
				&public,
				&plaintext,
				"edit_vulnerability_decision",
				json!({ "id": "GHSA-1", "source": "tamanu", "reason": "Behind the VPN" }),
			)
			.await;
			assert!(edit.get("error").is_none(), "{edit}");
			let edit = &edit["result"]["structuredContent"];
			assert_eq!(edit["reason"], "Behind the VPN");
			assert_eq!(edit["earlier_reason"], "Behind a firewall");
			assert_eq!(edit["via"], "mcp");
			let log = logged(&mut conn, "vulnerability_decision_edited").await;
			assert_eq!(log.len(), 1);
			assert!(matches!(&log[0].actor, Actor::Token { id, .. } if *id == token.id));

			let unchanged = call(
				&public,
				&plaintext,
				"edit_vulnerability_decision",
				json!({ "id": "GHSA-1", "source": "tamanu", "reason": "Behind the VPN" }),
			)
			.await;
			assert!(unchanged.get("error").is_some(), "{unchanged}");
			assert_eq!(
				logged(&mut conn, "vulnerability_decision_edited")
					.await
					.len(),
				1
			);

			let withdrawn = call(
				&public,
				&plaintext,
				"withdraw_vulnerability_decisions",
				json!({
					"items": [
						{ "id": "GHSA-1", "source": "tamanu" },
						{ "id": "GHSA-3", "source": "tamanu" },
					],
					"reason": "The VPN is going away",
				}),
			)
			.await;
			assert!(withdrawn.get("error").is_none(), "{withdrawn}");
			let batch = &withdrawn["result"]["structuredContent"];
			assert_eq!(batch["results"][0]["withdrawn"], true);
			assert_eq!(
				batch["results"][0]["withdrawal"]["reason"],
				"The VPN is going away"
			);
			assert_eq!(
				batch["results"][1]["withdrawn"], false,
				"GHSA-3 has no decision"
			);
			assert_eq!(batch["refused"], 1);
			assert_eq!(status_of(&public, &plaintext, "GHSA-1").await, "open");
			assert_eq!(
				logged(&mut conn, "vulnerability_decision_withdrawn")
					.await
					.len(),
				1,
				"only the withdrawn item is audited"
			);
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn fields_limit_the_output_and_details_are_opt_in() {
		commons_tests::server::run(async |mut conn, public, _private| {
			items(&mut conn).await;
			let (_, plaintext) = granted(&mut conn).await;
			let find = async |arguments: Value| {
				call(&public, &plaintext, "find_vulnerabilities", arguments).await
			};
			let keys = |env: &Value| -> BTreeSet<String> {
				env["result"]["structuredContent"]["items"][0]
					.as_object()
					.expect("an item")
					.keys()
					.cloned()
					.collect()
			};
			let set = |keys: &[&str]| {
				keys.iter()
					.map(|k| (*k).to_owned())
					.collect::<BTreeSet<_>>()
			};

			let default = find(json!({})).await;
			assert_eq!(default["result"]["structuredContent"]["total"], 3);
			let default_keys = keys(&default);
			assert!(default_keys.contains("summary") && default_keys.contains("suggestion"));
			assert!(
				!default_keys.contains("details"),
				"details are left out by default"
			);

			let some = find(json!({ "fields": ["summary", "status"] })).await;
			assert_eq!(keys(&some), set(&["id", "source", "summary", "status"]));

			let with_details = find(json!({ "fields": ["details"] })).await;
			assert_eq!(keys(&with_details), set(&["id", "source", "details"]));
			assert_eq!(
				with_details["result"]["structuredContent"]["items"][0]["details"],
				DETAILS
			);

			let included = find(json!({ "include_details": true })).await;
			let mut expected = default_keys;
			expected.insert("details".into());
			assert_eq!(keys(&included), expected);

			let unknown = find(json!({ "fields": ["summary", "colour"] })).await;
			let message = unknown["error"]["message"].as_str().expect("an error");
			assert!(
				message.contains("colour") && message.contains("workspace_packages"),
				"{message}"
			);

			let page = find(json!({ "page_size": 2, "fields": ["summary"] })).await;
			let page = &page["result"]["structuredContent"];
			assert_eq!(page["items"].as_array().unwrap().len(), 2);
			assert_eq!(page["total"], 3);
			let cursor = page["next_cursor"].as_str().expect("a next page");
			let last = find(json!({ "page_size": 2, "cursor": cursor })).await;
			let last = &last["result"]["structuredContent"];
			assert_eq!(last["items"].as_array().unwrap().len(), 1);
			assert!(last["next_cursor"].is_null());

			let filtered = find(json!({ "package": "lodash" })).await;
			assert_eq!(filtered["result"]["structuredContent"]["total"], 1);
			let searched = find(json!({ "search": "GHSA-3" })).await;
			assert_eq!(
				searched["result"]["structuredContent"]["items"][0]["id"],
				"GHSA-3"
			);
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn group_by_returns_counts_and_no_items() {
		commons_tests::server::run(async |mut conn, public, _private| {
			items(&mut conn).await;
			let (_, plaintext) = granted(&mut conn).await;
			let group = async |by: &str| {
				let env = call(
					&public,
					&plaintext,
					"find_vulnerabilities",
					json!({ "group_by": by }),
				)
				.await;
				assert!(env.get("error").is_none(), "{env}");
				env["result"]["structuredContent"].clone()
			};

			let by_workspace = group("workspace_package").await;
			assert!(by_workspace.get("items").is_none(), "{by_workspace}");
			assert_eq!(
				by_workspace["groups"],
				json!([{ "key": "server", "count": 3 }, { "key": "web", "count": 2 }]),
				"an item in several groups counts in each"
			);
			assert_eq!(by_workspace["total"], 3);

			assert_eq!(
				group("package").await["groups"],
				json!([{ "key": "left-pad", "count": 2 }, { "key": "lodash", "count": 1 }])
			);
			assert_eq!(
				group("status").await["groups"],
				json!([{ "key": "open", "count": 3 }])
			);

			let bad = call(
				&public,
				&plaintext,
				"find_vulnerabilities",
				json!({ "group_by": "colour" }),
			)
			.await;
			assert!(bad.get("error").is_some(), "{bad}");
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn the_triage_prompt_is_listed_and_takes_a_repository() {
		commons_tests::server::run(async |mut conn, public, _private| {
			let (_, plaintext) = granted(&mut conn).await;

			let list = rpc(&public, &plaintext, "prompts/list", json!({})).await;
			let prompts = list["result"]["prompts"].as_array().expect("prompts");
			let prompt = prompts
				.iter()
				.find(|prompt| prompt["name"] == "triage_vulnerabilities")
				.unwrap_or_else(|| panic!("the prompt is listed: {list}"));
			let argument = prompt["arguments"]
				.as_array()
				.and_then(|arguments| arguments.iter().find(|a| a["name"] == "repository"))
				.unwrap_or_else(|| panic!("it takes a repository: {prompt}"));
			assert_eq!(argument["required"], true);

			let got = rpc(
				&public,
				&plaintext,
				"prompts/get",
				json!({ "name": "triage_vulnerabilities", "arguments": { "repository": "bestool" } }),
			)
			.await;
			let text = got["result"]["messages"][0]["content"]["text"]
				.as_str()
				.unwrap_or_else(|| panic!("a message: {got}"));
			assert!(text.contains("source=`bestool`"));
			assert!(text.contains("record_vulnerability_decisions"));

			for repository in ["linux", "nonesuch"] {
				let refused = rpc(
					&public,
					&plaintext,
					"prompts/get",
					json!({
						"name": "triage_vulnerabilities",
						"arguments": { "repository": repository },
					}),
				)
				.await;
				assert!(refused.get("error").is_some(), "{repository}: {refused}");
			}
		})
		.await
	}
}
