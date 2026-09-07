//! Tests for the read-only MCP query interface mounted at `/api/mcp`.
//!
//! The endpoint runs in stateless mode (no server-side session), so each POST
//! is self-contained: a `tools/call` needs no prior `initialize`, just an
//! `MCP-Protocol-Version` header. Responses are plain `application/json`. The
//! debug build bypasses Tailscale auth, so no identity headers are needed.

use commons_tests::diesel_async::SimpleAsyncConnection;

/// Clients must accept both JSON and SSE on the POST leg.
const ACCEPT: &str = "application/json, text/event-stream";
/// Required on every non-initialize request in stateless mode.
const PROTO: &str = "2025-06-18";

/// Extract the JSON-RPC envelope from a response body (plain JSON in
/// json-response mode, or an SSE `data:` line otherwise).
fn parse_envelope(body: &str) -> serde_json::Value {
	let trimmed = body.trim_start();
	if trimmed.starts_with('{') {
		return serde_json::from_str(trimmed).expect("json body");
	}
	let data = body
		.lines()
		.filter_map(|l| l.strip_prefix("data:"))
		.last()
		.unwrap_or_else(|| panic!("no SSE data line in: {body}"));
	serde_json::from_str(data.trim()).expect("json in SSE data line")
}

/// Call a tool and return its `structuredContent`. Asserts the call succeeded
/// (no JSON-RPC error and `isError` is not true).
macro_rules! call_tool {
	($private:expr, $name:expr, $args:expr) => {{
		let resp = $private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 2, "method": "tools/call",
				"params": { "name": $name, "arguments": $args }
			}))
			.await;
		assert_eq!(resp.status_code().as_u16(), 200, "{} should 200", $name);
		let env = parse_envelope(&resp.text());
		assert!(env.get("error").is_none(), "{} rpc error: {env}", $name);
		let result = env.get("result").expect("result").clone();
		assert_ne!(
			result.get("isError").and_then(|v| v.as_bool()),
			Some(true),
			"{} returned a tool error: {result}",
			$name
		);
		result
			.get("structuredContent")
			.cloned()
			.unwrap_or(serde_json::Value::Null)
	}};
}

/// Seed a group, two applications (one grouped + monitored with a fresh healthy
/// status, one ungrouped), and a status carrying a version + platform.
const GROUP: &str = "11111111-1111-1111-1111-111111111111";
const SRV_GROUPED: &str = "22222222-2222-2222-2222-222222222222";
const SRV_UNGROUPED: &str = "33333333-3333-3333-3333-333333333333";

async fn seed(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'Prod Group'); \
		 WITH m AS (INSERT INTO machines (id, group_id) VALUES ('{SRV_GROUPED}', '{GROUP}') RETURNING id) INSERT INTO applications (id, host, name, type, rank, group_id, is_monitored, machine_id) VALUES \
			('{SRV_GROUPED}', 'https://prod-central', 'Prod Central', 'tamanu-central', 'production', '{GROUP}', true, '{SRV_GROUPED}'); \
		 WITH m AS (INSERT INTO machines (id) VALUES ('{SRV_UNGROUPED}') RETURNING id) INSERT INTO applications (id, host, name, type, machine_id) VALUES \
			('{SRV_UNGROUPED}', 'https://lonely', 'Lonely Facility', 'tamanu-facility', '{SRV_UNGROUPED}'); \
		 INSERT INTO statuses (server_id, version, healthy, health, extra, created_at) VALUES \
			('{SRV_GROUPED}', '2.34.1', true, '[]'::jsonb, \
			 '{{\"pgVersion\": \"PostgreSQL 14.2 on x86_64-pc-linux-gnu\"}}'::jsonb, NOW() - interval '1 minute'); \
		 INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES \
			('{SRV_GROUPED}', 'alertd', \
			 '{{\"pgVersion\": \"PostgreSQL 14.2 on x86_64-pc-linux-gnu\", \"bestoolVersion\": \"2.10.5\"}}'::jsonb, '2.34.1');"
	))
	.await
	.expect("seed");
}

#[tokio::test(flavor = "multi_thread")]
async fn initialize_and_list_tools() {
	commons_tests::server::run(async |_conn, _public, private| {
		// initialize works (and returns tool capability), but is not required
		// before other calls in stateless mode.
		let init = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 1, "method": "initialize",
				"params": {
					"protocolVersion": PROTO,
					"capabilities": {},
					"clientInfo": { "name": "canopy-tests", "version": "0" }
				}
			}))
			.await;
		assert_eq!(init.status_code().as_u16(), 200, "initialize should 200");

		let list = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 2, "method": "tools/list", "params": {}
			}))
			.await;
		assert_eq!(list.status_code().as_u16(), 200);
		let body = list.text();
		for tool in [
			"find_servers",
			"get_server",
			"find_machines",
			"get_machine",
			"find_groups",
			"get_group",
			"list_versions",
			"get_version",
			"fleet_summary",
			"find_backup_problems",
			"find_incidents",
			"get_incident",
			"find_issues",
			"get_issue",
			"list_backup_runs",
			"list_maintenance_runs",
			"find_restore_replicas",
			"get_restore_replica",
			"get_backup_defaults",
			"list_upgrade_plans",
			"get_upgrade_plan_history",
		] {
			assert!(body.contains(tool), "tools/list missing {tool}: {body}");
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn tools_call_needs_no_session() {
	// Regression: the endpoint is stateless, so a `tools/call` must succeed on
	// its own — no `initialize` handshake and no session id. (The default
	// stateful mode 404s any request routed to a replica that didn't handle
	// `initialize`, which is what broke real clients behind the load balancer.)
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;
		let summary = call_tool!(private, "fleet_summary", serde_json::json!({}));
		assert!(summary["total_servers"].as_u64().unwrap() >= 2);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn oauth_discovery_is_404_not_spa() {
	// MCP clients probe `/.well-known/oauth-*` for OAuth metadata. The SPA
	// fallback used to answer 200 text/html, which clients fail to parse as JSON
	// and report as "needs authentication". A 404 tells them there's no OAuth, so
	// they connect with the ambient (Tailscale) identity instead.
	commons_tests::server::run(async |_conn, _public, private| {
		for path in [
			"/.well-known/oauth-protected-resource",
			"/.well-known/oauth-protected-resource/api/mcp",
			"/.well-known/oauth-authorization-server",
		] {
			let resp = private.get(path).await;
			assert_eq!(
				resp.status_code().as_u16(),
				404,
				"{path} should be 404, not the SPA"
			);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn accepts_non_loopback_host() {
	// Regression: rmcp's DNS-rebinding guard defaults to a loopback-only Host
	// allowlist, which 403s the real tailnet instance's host. The endpoint is
	// gated by the ingress + tagged-device guard + tailnet-user check instead,
	// so a non-loopback Host must be accepted.
	commons_tests::server::run(async |_conn, _public, private| {
		let init = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("host", "canopy.example.ts.net")
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 1, "method": "initialize",
				"params": {
					"protocolVersion": PROTO,
					"capabilities": {},
					"clientInfo": { "name": "canopy-tests", "version": "0" }
				}
			}))
			.await;
		assert_eq!(
			init.status_code().as_u16(),
			200,
			"non-loopback Host should be accepted, got: {}",
			init.text()
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn find_servers_filters_and_decorates() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;

		// Unfiltered: both seeded applications.
		let all = call_tool!(private, "find_servers", serde_json::json!({}));
		assert!(all["total_matched"].as_u64().unwrap() >= 2);

		// Filter by type=tamanu-facility → only the ungrouped one.
		let facility = call_tool!(
			private,
			"find_servers",
			serde_json::json!({ "type": "tamanu-facility" })
		);
		let applications = facility["applications"].as_array().unwrap();
		assert_eq!(applications.len(), 1);
		assert_eq!(applications[0]["id"], SRV_UNGROUPED);
		assert_eq!(applications[0]["health"], "healthy");

		// Query matches by name.
		let q = call_tool!(
			private,
			"find_servers",
			serde_json::json!({ "query": "central" })
		);
		let names: Vec<&str> = q["applications"]
			.as_array()
			.unwrap()
			.iter()
			.map(|s| s["name"].as_str().unwrap())
			.collect();
		assert_eq!(names, vec!["Prod Central"]);

		// A type Canopy has never seen is a type, not an error: the set is
		// open, so filtering by one simply matches nothing rather than being
		// refused. What is refused is a value that is not a type at all.
		// spec: APP#where-a-type-comes-from
		let unknown = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 9, "method": "tools/call",
				"params": { "name": "find_servers", "arguments": { "type": "open-mrs" } }
			}))
			.await;
		let env = parse_envelope(&unknown.text());
		assert!(
			env.get("error").is_none() && env["result"]["isError"] != serde_json::json!(true),
			"an unknown type filters rather than erroring: {env}"
		);

		let bad = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 9, "method": "tools/call",
				"params": { "name": "find_servers", "arguments": { "type": "not a slug" } }
			}))
			.await;
		let env = parse_envelope(&bad.text());
		assert!(
			env.get("error").is_some() || env["result"]["isError"] == serde_json::json!(true),
			"a value that is not a type at all should error: {env}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_server_detail_and_not_found() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;

		let detail = call_tool!(
			private,
			"get_server",
			serde_json::json!({ "server_id": SRV_GROUPED })
		);
		assert_eq!(detail["name"], "Prod Central");
		assert_eq!(detail["group_name"], "Prod Group");
		assert_eq!(detail["latest_status"]["version"], "2.34.1");
		// The figures describe the server, not the push that happened to
		// land last, so they sit beside `latest_status` rather than inside it.
		// spec: FIG#sourcing
		assert_eq!(detail["figures"]["platform"], "Linux");
		assert_eq!(detail["figures"]["postgres_version"], "14.2");
		assert_eq!(detail["figures"]["bestool_version"], "2.10.5");

		// Unknown id → tool error (isError), not a protocol error.
		let missing = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 3, "method": "tools/call",
				"params": {
					"name": "get_server",
					"arguments": { "server_id": "44444444-4444-4444-4444-444444444444" }
				}
			}))
			.await;
		let env = parse_envelope(&missing.text());
		assert_eq!(env["result"]["isError"], serde_json::json!(true));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn group_listing_and_detail() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;

		let groups = call_tool!(private, "find_groups", serde_json::json!({}));
		let g = groups["groups"]
			.as_array()
			.unwrap()
			.iter()
			.find(|g| g["id"] == GROUP)
			.expect("seeded group present");
		assert_eq!(g["name"], "Prod Group");
		assert_eq!(g["member_count"], 1);
		assert_eq!(g["highest_rank"], "production");

		let detail = call_tool!(
			private,
			"get_group",
			serde_json::json!({ "group_id": GROUP })
		);
		assert_eq!(detail["name"], "Prod Group");
		let members = detail["members"].as_array().unwrap();
		assert_eq!(members.len(), 1);
		assert_eq!(members[0]["id"], SRV_GROUPED);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn fleet_summary_rolls_up() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;

		let s = call_tool!(private, "fleet_summary", serde_json::json!({}));
		assert!(s["total_servers"].as_u64().unwrap() >= 2);
		assert_eq!(s["counts"]["by_type"]["tamanu-facility"], 1);
		assert_eq!(s["version_distribution"]["2.34.1"], 1);
		assert_eq!(s["health"]["healthy"], 1);
	})
	.await
}

// --- incidents & issues ------------------------------------------------------

const IGROUP: &str = "aaaaaaaa-0000-0000-0000-000000000001";
const ISRV: &str = "aaaaaaaa-0000-0000-0000-000000000002";
const ISSUE1: &str = "aaaaaaaa-0000-0000-0000-000000000011";
const ISSUE2: &str = "aaaaaaaa-0000-0000-0000-000000000012";
const INC_OPEN: &str = "aaaaaaaa-0000-0000-0000-0000000000a1";
const INC_CLOSED: &str = "aaaaaaaa-0000-0000-0000-0000000000a2";

/// Seed a group + server, two issues (one active error, one inactive warning),
/// an open incident (2d ago) linked to the active issue, and a closed incident
/// (opened 5d ago, closed 3d ago).
async fn seed_incidents(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{IGROUP}', 'Inc Group'); \
		 WITH m AS (INSERT INTO machines (id, group_id) VALUES ('{ISRV}', '{IGROUP}') RETURNING id) INSERT INTO applications (id, host, name, type, group_id, is_monitored, machine_id) VALUES \
			('{ISRV}', 'https://inc', 'Inc Application', 'tamanu-central', '{IGROUP}', true, '{ISRV}'); \
		 INSERT INTO issues (id, created_at, updated_at, application_id, source, ref, check_name, observed_result, effective_result, description, message, active, first_seen, last_seen, last_degraded_at) VALUES \
			('{ISSUE1}', NOW(), NOW(), '{ISRV}', 'test', 'r1', 'r1', 'failed', 'failed', 'Disk full', 'disk usage 98%', true, NOW() - interval '2 days', NOW() - interval '1 hour', NOW() - interval '1 hour'), \
			('{ISSUE2}', NOW(), NOW(), '{ISRV}', 'test', 'r2', 'r2', 'warning', 'warning', NULL, 'slow query', false, NOW() - interval '10 days', NOW() - interval '9 days', NOW() - interval '9 days'); \
		 INSERT INTO incidents (id, created_at, updated_at, server_group_id, opened_at, closed_at) VALUES \
			('{INC_OPEN}', NOW(), NOW(), '{IGROUP}', NOW() - interval '2 days', NULL), \
			('{INC_CLOSED}', NOW(), NOW(), '{IGROUP}', NOW() - interval '5 days', NOW() - interval '3 days'); \
		 INSERT INTO incident_issues (incident_id, issue_id, joined_at, left_at) VALUES \
			('{INC_OPEN}', '{ISSUE1}', NOW() - interval '2 days', NULL); \
		 INSERT INTO slack_outbox (kind, incident_id, payload, deliver_after, delivered_at, attempts) VALUES \
			('incident_open', '{INC_OPEN}', '{{}}'::jsonb, NOW() - interval '2 days', NOW() - interval '2 days', 1);"
	))
	.await
	.expect("seed incidents");
}

fn ids_of(list: &serde_json::Value, key: &str) -> Vec<String> {
	list[key]
		.as_array()
		.unwrap()
		.iter()
		.map(|x| x["id"].as_str().unwrap().to_string())
		.collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn incidents_window_status_and_detail() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_incidents(&mut conn).await;

		// Default 7-day window: both the still-open and the (closed 3d ago) incident.
		let week = call_tool!(private, "find_incidents", serde_json::json!({}));
		let week_ids = ids_of(&week, "incidents");
		assert!(week_ids.contains(&INC_OPEN.to_string()));
		assert!(
			week_ids.contains(&INC_CLOSED.to_string()),
			"7d window should include the incident closed 3d ago"
		);
		let open = week["incidents"]
			.as_array()
			.unwrap()
			.iter()
			.find(|i| i["id"] == INC_OPEN)
			.unwrap();
		assert_eq!(open["status"], "open");
		assert_eq!(open["group_name"], "Inc Group");
		assert_eq!(open["issue_count"], 1);
		// INC_OPEN has a delivered Slack open → published; INC_CLOSED has none.
		assert_eq!(open["published"], true);
		assert!(open["open_duration_secs"].as_i64().unwrap() > 0);
		assert_eq!(week["published_count"], 1);
		let closed = week["incidents"]
			.as_array()
			.unwrap()
			.iter()
			.find(|i| i["id"] == INC_CLOSED)
			.unwrap();
		assert_eq!(closed["published"], false);

		// 1-day window excludes the incident that closed 3 days ago.
		let day = call_tool!(
			private,
			"find_incidents",
			serde_json::json!({ "since_days": 1 })
		);
		let day_ids = ids_of(&day, "incidents");
		assert!(day_ids.contains(&INC_OPEN.to_string()));
		assert!(!day_ids.contains(&INC_CLOSED.to_string()));

		// status=open excludes the closed one.
		let only_open = call_tool!(
			private,
			"find_incidents",
			serde_json::json!({ "status": "open" })
		);
		let open_ids = ids_of(&only_open, "incidents");
		assert!(open_ids.contains(&INC_OPEN.to_string()));
		assert!(!open_ids.contains(&INC_CLOSED.to_string()));

		// Detail: the open incident carries its attached issue.
		let detail = call_tool!(
			private,
			"get_incident",
			serde_json::json!({ "incident_id": INC_OPEN })
		);
		assert_eq!(detail["published"], true);
		let issues = detail["issues"].as_array().unwrap();
		assert_eq!(issues.len(), 1);
		assert_eq!(issues[0]["issue_id"], ISSUE1);
		assert_eq!(issues[0]["effective_result"], "failed");
		assert_eq!(issues[0]["ref"], "r1");
		// The scope says whose failure it is, and names it.
		// spec: MCP#incidents-and-issues
		assert_eq!(issues[0]["scope"]["grain"], "application");
		assert_eq!(issues[0]["scope"]["name"], "Inc Application");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn issues_filter_and_detail() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_incidents(&mut conn).await;

		// active_only (default true) → only the active error issue.
		let active = call_tool!(private, "find_issues", serde_json::json!({}));
		let active_ids = ids_of(&active, "issues");
		assert!(active_ids.contains(&ISSUE1.to_string()));
		assert!(!active_ids.contains(&ISSUE2.to_string()));

		// active_only=false → the inactive one shows up too.
		let all = call_tool!(
			private,
			"find_issues",
			serde_json::json!({ "active_only": false })
		);
		let all_ids = ids_of(&all, "issues");
		assert!(all_ids.contains(&ISSUE2.to_string()));

		// result filter.
		let warnings = call_tool!(
			private,
			"find_issues",
			serde_json::json!({ "active_only": false, "results": ["warning"] })
		);
		let w = warnings["issues"].as_array().unwrap();
		assert!(!w.is_empty() && w.iter().all(|i| i["effective_result"] == "warning"));

		// Detail: the issue's fields + the incident it belongs to.
		let detail = call_tool!(
			private,
			"get_issue",
			serde_json::json!({ "issue_id": ISSUE1 })
		);
		assert_eq!(detail["effective_result"], "failed");
		let inc_ids: Vec<String> = detail["incidents"]
			.as_array()
			.unwrap()
			.iter()
			.map(|i| i["incident_id"].as_str().unwrap().to_string())
			.collect();
		assert!(inc_ids.contains(&INC_OPEN.to_string()));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn backup_problems_scan_runs() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;
		// No ready backup config seeded, so the scan returns an empty, well-formed set.
		let p = call_tool!(private, "find_backup_problems", serde_json::json!({}));
		assert!(p["count"].is_number());
		assert!(p["problems"].is_array());
	})
	.await
}

/// The advertised window is "failed runs in the last 24h". Inspecting only the
/// N newest runs and time-filtering afterwards shrinks that window for exactly
/// the groups that back up most often — a real failure vanishes behind the
/// successes reported since.
#[tokio::test(flavor = "multi_thread")]
async fn backup_problems_finds_a_failure_behind_many_later_successes() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = "cccccccc-0000-0000-0000-000000000001";
		let server = "cccccccc-0000-0000-0000-000000000002";
		let device = "cccccccc-0000-0000-0000-000000000003";

		let mut sql = format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'Chatty'); \
			 WITH m AS (INSERT INTO machines (id, group_id) VALUES ('{server}', '{group}') RETURNING id) INSERT INTO applications (id, host, name, type, group_id, machine_id) VALUES \
				('{server}', 'https://chatty', 'Chatty', 'tamanu-central', '{group}', '{server}'); \
			 INSERT INTO devices (id, role) VALUES ('{device}', 'machine'); \
			 INSERT INTO server_group_backup_config \
				(group_id, bucket, prefix, target_role_arn, maintenance_role_arn, \
				 repo_password_ref, status, mode, placement) \
				VALUES ('{group}', 'b', '', 'arn', 'maint', 'pw', 'ready', 'from_birth', 'external'); \
			 INSERT INTO backup_runs \
				(id, device_id, group_id, machine_id, type, purpose, outcome, error, reported_at) \
			  VALUES ('{}', '{device}', '{group}', '{server}', 'tamanu-postgres', 'backup', \
				'failure', 'disk full', NOW() - interval '8 hours');",
			uuid::Uuid::new_v4()
		);
		// 30 later successes — more than any per-group row cap — all inside the
		// same 24h window, each newer than the failure.
		for i in 0..30 {
			sql.push_str(&format!(
				"INSERT INTO backup_runs \
					(id, device_id, group_id, machine_id, type, purpose, outcome, reported_at) \
				  VALUES ('{}', '{device}', '{group}', '{server}', 'tamanu-postgres', 'backup', \
					'success', NOW() - interval '{i} minutes');",
				uuid::Uuid::new_v4()
			));
		}
		conn.batch_execute(&sql).await.expect("seed chatty group");

		let p = call_tool!(
			private,
			"find_backup_problems",
			serde_json::json!({ "group_id": group })
		);
		let failures: Vec<&serde_json::Value> = p["problems"]
			.as_array()
			.unwrap()
			.iter()
			.filter(|x| x["kind"] == "failed_run")
			.collect();
		assert_eq!(
			failures.len(),
			1,
			"the 8h-old failure is inside the 24h window: {}",
			p["problems"]
		);
		assert_eq!(failures[0]["detail"], "disk full");
	})
	.await
}

// --- backup runs, maintenance runs, restore replicas, backup defaults -------

const RGROUP: &str = "bbbbbbbb-0000-0000-0000-000000000001";
const RSERVER: &str = "bbbbbbbb-0000-0000-0000-000000000002";
const RDEVICE: &str = "bbbbbbbb-0000-0000-0000-000000000003";
const RUN_OK: &str = "bbbbbbbb-0000-0000-0000-0000000000a1";
const RUN_FAIL: &str = "bbbbbbbb-0000-0000-0000-0000000000a2";
const CONSUMER: &str = "bbbbbbbb-0000-0000-0000-000000000004";
const REPLICA_GROUP_WIDE: &str = "bbbbbbbb-0000-0000-0000-0000000000b1";
const REPLICA_SERVER_SCOPED: &str = "bbbbbbbb-0000-0000-0000-0000000000b2";
const REPLICA_GAP: &str = "bbbbbbbb-0000-0000-0000-0000000000b3";

/// A group + server + device, two backup runs (one success, one failure), and
/// two maintenance runs (one finished, one still running).
async fn seed_backup_runs(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{RGROUP}', 'Backup Group'); \
		 WITH m AS (INSERT INTO machines (id, group_id, name) VALUES ('{RSERVER}', '{RGROUP}', 'Backup Target') RETURNING id) INSERT INTO applications (id, host, name, type, group_id, machine_id) VALUES \
			('{RSERVER}', 'https://backup-target', 'Backup Target', 'tamanu-central', '{RGROUP}', '{RSERVER}'); \
		 INSERT INTO devices (id, role) VALUES ('{RDEVICE}', 'machine'); \
		 INSERT INTO backup_runs \
			(id, device_id, group_id, machine_id, type, purpose, outcome, snapshot_id, bytes_uploaded, s3_sent_raw_bytes, snapshot_logical_bytes, reported_at) \
			VALUES \
			('{RUN_OK}', '{RDEVICE}', '{RGROUP}', '{RSERVER}', 'tamanu-postgres', 'backup', 'success', 'snap-1', 1000, 1200, 900, NOW() - interval '1 hour'), \
			('{RUN_FAIL}', '{RDEVICE}', '{RGROUP}', '{RSERVER}', 'tamanu-postgres', 'backup', 'failure', NULL, NULL, NULL, NULL, NOW() - interval '10 minutes'); \
		 INSERT INTO backup_maintenance_runs (group_id, kind, started_at, finished_at, outcome, bytes_reclaimed) VALUES \
			('{RGROUP}', 'quick', NOW() - interval '2 hours', NOW() - interval '110 minutes', 'success', 2048), \
			('{RGROUP}', 'full', NOW() - interval '30 minutes', NULL, NULL, NULL);"
	))
	.await
	.expect("seed backup runs");
}

/// A restore consumer advertising only `verify`, and three declarations: a
/// group-wide `verify` (supported, no single server to report health for), a
/// server-scoped `verify` (supported, with a healthy check on record), and a
/// server-scoped `analytics` (unsupported — a gap, since the consumer only
/// advertises `verify`).
async fn seed_restore_replicas(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{CONSUMER}', 'backup-restore'); \
		 INSERT INTO restore_consumer_capabilities (consumer_device_id, intent, semantics) VALUES \
			('{CONSUMER}', 'verify', '[\"check\"]'::jsonb); \
		 INSERT INTO restore_replicas (id, consumer_device_id, group_id, machine_id, type, intent, name) VALUES \
			('{REPLICA_GROUP_WIDE}', '{CONSUMER}', '{RGROUP}', NULL, 'tamanu-postgres', 'verify', 'group-wide'), \
			('{REPLICA_SERVER_SCOPED}', '{CONSUMER}', '{RGROUP}', '{RSERVER}', 'tamanu-postgres', 'verify', 'server-scoped'), \
			('{REPLICA_GAP}', '{CONSUMER}', '{RGROUP}', '{RSERVER}', 'tamanu-postgres', 'analytics', 'server-scoped-gap'); \
		 INSERT INTO backup_restore_checks \
			(replica_id, replica_name, consumer_device_id, group_id, machine_id, type, intent, snapshot_id, outcome, replica_healthy, observed_at) \
			VALUES \
			('{REPLICA_SERVER_SCOPED}', 'server-scoped', '{CONSUMER}', '{RGROUP}', '{RSERVER}', 'tamanu-postgres', 'verify', 'snap-1', 'success', true, NOW() - interval '30 minutes');"
	))
	.await
	.expect("seed restore replicas");
}

#[tokio::test(flavor = "multi_thread")]
async fn list_backup_runs_filters() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_backup_runs(&mut conn).await;

		let all = call_tool!(private, "list_backup_runs", serde_json::json!({}));
		assert_eq!(all["count"], 2);
		let run = all["runs"]
			.as_array()
			.unwrap()
			.iter()
			.find(|r| r["id"] == RUN_OK)
			.expect("seeded run present");
		assert_eq!(run["group_name"], "Backup Group");
		assert_eq!(run["machine_name"], "Backup Target");
		assert_eq!(run["outcome"], "success");
		assert_eq!(run["s3_sent_raw_bytes"], 1200);
		assert_eq!(run["snapshot_logical_bytes"], 900);

		let failures = call_tool!(
			private,
			"list_backup_runs",
			serde_json::json!({ "outcome": "failure" })
		);
		let ids: Vec<&str> = failures["runs"]
			.as_array()
			.unwrap()
			.iter()
			.map(|r| r["id"].as_str().unwrap())
			.collect();
		assert_eq!(ids, vec![RUN_FAIL]);

		let by_type = call_tool!(
			private,
			"list_backup_runs",
			serde_json::json!({ "type": "tamanu-postgres" })
		);
		assert_eq!(by_type["count"], 2);

		let by_other_type = call_tool!(
			private,
			"list_backup_runs",
			serde_json::json!({ "type": "files" })
		);
		assert_eq!(by_other_type["count"], 0);

		let by_server = call_tool!(
			private,
			"list_backup_runs",
			serde_json::json!({ "server_id": RSERVER })
		);
		assert_eq!(by_server["count"], 2);

		let limited = call_tool!(
			private,
			"list_backup_runs",
			serde_json::json!({ "limit": 1 })
		);
		assert_eq!(limited["count"], 1);
		assert_eq!(limited["truncated"], true);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn list_maintenance_runs_filters() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_backup_runs(&mut conn).await;

		let all = call_tool!(private, "list_maintenance_runs", serde_json::json!({}));
		assert_eq!(all["count"], 2);
		let finished = all["runs"]
			.as_array()
			.unwrap()
			.iter()
			.find(|r| r["kind"] == "quick")
			.expect("finished run present");
		assert_eq!(finished["group_name"], "Backup Group");
		assert_eq!(finished["outcome"], "success");
		assert_eq!(finished["bytes_reclaimed"], 2048);

		let running = call_tool!(
			private,
			"list_maintenance_runs",
			serde_json::json!({ "outcome": "running" })
		);
		let kinds: Vec<&str> = running["runs"]
			.as_array()
			.unwrap()
			.iter()
			.map(|r| r["kind"].as_str().unwrap())
			.collect();
		assert_eq!(kinds, vec!["full"]);
		assert!(running["runs"][0]["finished_at"].is_null());

		let by_kind = call_tool!(
			private,
			"list_maintenance_runs",
			serde_json::json!({ "kind": "full" })
		);
		assert_eq!(by_kind["count"], 1);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn find_restore_replicas_reports_gap_and_health() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_backup_runs(&mut conn).await;
		seed_restore_replicas(&mut conn).await;

		let all = call_tool!(
			private,
			"find_restore_replicas",
			serde_json::json!({ "group_id": RGROUP })
		);
		assert_eq!(all["count"], 3);
		let replicas = all["replicas"].as_array().unwrap();

		let group_wide = replicas
			.iter()
			.find(|r| r["id"] == REPLICA_GROUP_WIDE)
			.unwrap();
		assert_eq!(group_wide["gap"], false);
		assert!(
			group_wide["last_healthy_at"].is_null(),
			"group-wide declarations have no single server to report health for"
		);

		let server_scoped = replicas
			.iter()
			.find(|r| r["id"] == REPLICA_SERVER_SCOPED)
			.unwrap();
		assert_eq!(server_scoped["gap"], false);
		assert_eq!(server_scoped["machine_name"], "Backup Target");
		assert!(!server_scoped["last_healthy_at"].is_null());

		let gap = replicas.iter().find(|r| r["id"] == REPLICA_GAP).unwrap();
		assert_eq!(
			gap["gap"], true,
			"consumer only advertises verify, not analytics"
		);

		// Empty for an unrelated group.
		let none = call_tool!(
			private,
			"find_restore_replicas",
			serde_json::json!({ "group_id": "cccccccc-0000-0000-0000-000000000000" })
		);
		assert_eq!(none["count"], 0);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_restore_replica_detail_and_gap() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_backup_runs(&mut conn).await;
		seed_restore_replicas(&mut conn).await;

		let detail = call_tool!(
			private,
			"get_restore_replica",
			serde_json::json!({ "replica_id": REPLICA_SERVER_SCOPED })
		);
		assert_eq!(detail["gap"], false);
		assert_eq!(detail["intent_descriptor"]["intent"], "verify");
		assert_eq!(detail["intent_descriptor"]["semantics"][0], "check");
		let checks = detail["recent_checks"].as_array().unwrap();
		assert_eq!(checks.len(), 1);
		assert_eq!(checks[0]["outcome"], "success");
		assert_eq!(checks[0]["replica_healthy"], true);
		assert_eq!(checks[0]["snapshot_id"], "snap-1");

		let gap_detail = call_tool!(
			private,
			"get_restore_replica",
			serde_json::json!({ "replica_id": REPLICA_GAP })
		);
		assert_eq!(gap_detail["gap"], true);
		assert!(gap_detail["intent_descriptor"].is_null());
		assert!(gap_detail["recent_checks"].as_array().unwrap().is_empty());

		// Unknown id → tool error, not a protocol error.
		let missing = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 4, "method": "tools/call",
				"params": {
					"name": "get_restore_replica",
					"arguments": { "replica_id": "44444444-4444-4444-4444-444444444444" }
				}
			}))
			.await;
		let env = parse_envelope(&missing.text());
		assert_eq!(env["result"]["isError"], serde_json::json!(true));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn backup_defaults_lists_seeded_org_default() {
	commons_tests::server::run(async |_conn, _public, private| {
		// The `tamanu-postgres` default is seeded by a migration, so it's present
		// even with no other setup.
		let d = call_tool!(private, "get_backup_defaults", serde_json::json!({}));
		let defaults = d["defaults"].as_array().unwrap();
		let tpg = defaults
			.iter()
			.find(|d| d["type"] == "tamanu-postgres")
			.expect("seeded default present");
		assert_eq!(tpg["default_interval_seconds"], 6 * 60 * 60);
		assert_eq!(tpg["default_retention"]["keep_daily"], 7);
		assert_eq!(tpg["auto_enable"], false);
	})
	.await
}

/// File one observation of (source=test, check=wobbly) on a server, with
/// the given observed result, through the real filing path so the
/// stability record updates like production.
async fn observe_wobbly(
	conn: &mut database::diesel_async::AsyncPgConnection,
	server_id: uuid::Uuid,
	observed: commons_types::status::CheckResult,
) {
	use commons_types::status::CheckResult;
	let active = matches!(
		observed,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let stamp = database::issues::CheckStateStamp {
		check: "wobbly".into(),
		observed,
		effective: observed,
		escalates: false,
		detail: None,
	};
	database::issues::NewEvent {
		source: "test".into(),
		r#ref: "wobbly".into(),
		description: None,
		message: "obs".into(),
		active: Some(active),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), false)
	.await
	.expect("file observation");
}

#[tokio::test(flavor = "multi_thread")]
async fn check_stability_returns_full_records_for_pairs() {
	use commons_types::status::CheckResult;
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;
		let grouped: uuid::Uuid = SRV_GROUPED.parse().unwrap();

		// A flap: red, green, red — three observations, three ring entries.
		observe_wobbly(&mut conn, grouped, CheckResult::Failed).await;
		observe_wobbly(&mut conn, grouped, CheckResult::Passed).await;
		observe_wobbly(&mut conn, grouped, CheckResult::Failed).await;

		let out = call_tool!(
			private,
			"get_check_stability",
			serde_json::json!({
				"checks": [{ "source": "test", "check_name": "wobbly" }],
			})
		);
		let rows = out["rows"].as_array().expect("rows array");
		assert_eq!(rows.len(), 1, "one matching state: {rows:?}");
		let row = &rows[0];
		assert_eq!(row["application_id"], serde_json::json!(SRV_GROUPED));
		assert_eq!(row["server_name"], serde_json::json!("Prod Central"));
		assert_eq!(row["source"], serde_json::json!("test"));
		assert_eq!(row["check_name"], serde_json::json!("wobbly"));
		assert_eq!(row["observed_result"], serde_json::json!("failed"));
		let stability = &row["stability"];
		assert_eq!(stability["observations"], serde_json::json!(3));
		assert_eq!(stability["degraded_observations"], serde_json::json!(2));
		assert_eq!(
			stability["transitions"].as_array().map(|t| t.len()),
			Some(3),
			"red, green, red: {stability}"
		);
		assert_eq!(
			stability["duty_cycle"].as_array().map(|d| d.len()),
			Some(168)
		);
		assert_eq!(stability["stats"]["flips_24h"], serde_json::json!(3));

		// Narrowing to a server with no such state returns nothing.
		let out = call_tool!(
			private,
			"get_check_stability",
			serde_json::json!({
				"checks": [{ "source": "test", "check_name": "wobbly" }],
				"application_id": SRV_UNGROUPED,
			})
		);
		assert_eq!(out["rows"].as_array().map(|r| r.len()), Some(0));

		// Narrowing to the group finds it again.
		let out = call_tool!(
			private,
			"get_check_stability",
			serde_json::json!({
				"checks": [{ "source": "test", "check_name": "wobbly" }],
				"group_id": GROUP,
			})
		);
		assert_eq!(out["rows"].as_array().map(|r| r.len()), Some(1));

		// An empty checks list is an invalid-params error, not a result.
		let resp = private
			.post("/api/mcp")
			.add_header("accept", ACCEPT)
			.add_header("mcp-protocol-version", PROTO)
			.json(&serde_json::json!({
				"jsonrpc": "2.0", "id": 9, "method": "tools/call",
				"params": { "name": "get_check_stability", "arguments": { "checks": [] } }
			}))
			.await;
		let env = parse_envelope(&resp.text());
		assert!(
			env.get("error").is_some(),
			"empty checks should be an rpc error: {env}"
		);
	})
	.await
}

const SRV_OFFLINE: &str = "44444444-4444-4444-4444-444444444444";

/// `ServerSummary.version` is documented "retained even when long offline",
/// and `get_server` implements that via `ReportedDetail::last_version`.
/// `find_servers` took it from the status row instead, which is read through
/// a seven-day window — so a server quiet for longer reported no version at
/// all, and a fleet version survey run through this tool undercounted
/// exactly the applications most worth noticing.
#[tokio::test(flavor = "multi_thread")]
async fn find_servers_retains_the_version_of_a_long_offline_server() {
	commons_tests::server::run(async |mut conn, _public, private| {
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (id) VALUES ('{SRV_OFFLINE}') RETURNING id) INSERT INTO applications (id, host, name, type, rank, machine_id) VALUES \
				('{SRV_OFFLINE}', 'https://long-gone', 'Long Gone', 'tamanu-central', 'production', '{SRV_OFFLINE}'); \
			 INSERT INTO statuses (server_id, version, healthy, health, extra, created_at) VALUES \
				('{SRV_OFFLINE}', '2.30.0', true, '[]'::jsonb, '{{}}'::jsonb, \
				 NOW() - interval '30 days'); \
			 INSERT INTO application_reported_detail (application_id, source, extra, version, reported_at) VALUES \
				('{SRV_OFFLINE}', 'alertd', '{{}}'::jsonb, '2.30.0', NOW() - interval '30 days');"
		))
		.await
		.expect("seed long-offline server");

		let found = call_tool!(
			private,
			"find_servers",
			serde_json::json!({ "query": "Long Gone" })
		);
		let applications = found["applications"].as_array().unwrap();
		assert_eq!(applications.len(), 1, "seeded server should match");
		let s = &applications[0];

		assert_eq!(
			s["version"], "2.30.0",
			"the last version it reported is retained, got {s}",
		);
		// Both come from the same projection row, which carries when the
		// source last reported as well as what it reported. Answering either
		// from `statuses` means scanning every weekly partition, which is what
		// the lookback cap exists to refuse; `application_reported_detail` is
		// one row per (application, source) reached by primary key, so it
		// answers both unbounded.
		assert!(
			!s["last_seen"].is_null(),
			"a server past the status window still has a last-seen: {s}",
		);
		assert_eq!(
			s["reachability"], "down",
			"a server that reported a month ago is unreachable, not never heard from",
		);
	})
	.await
}

/// The state above is the one it is not: a server that has genuinely never
/// reported has no projection row, so it stays `gone` with no last-seen.
#[tokio::test(flavor = "multi_thread")]
async fn find_servers_reports_a_never_reported_server_as_gone() {
	commons_tests::server::run(async |mut conn, _public, private| {
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (id) VALUES ('{SRV_OFFLINE}') RETURNING id) INSERT INTO applications (id, host, name, type, rank, machine_id) VALUES \
				('{SRV_OFFLINE}', 'https://never-spoke', 'Never Spoke', 'tamanu-central', 'production', '{SRV_OFFLINE}');"
		))
		.await
		.expect("seed never-reported server");

		let found = call_tool!(
			private,
			"find_servers",
			serde_json::json!({ "query": "Never Spoke" })
		);
		let applications = found["applications"].as_array().unwrap();
		assert_eq!(applications.len(), 1, "seeded server should match");
		let s = &applications[0];

		assert!(s["last_seen"].is_null(), "nothing was ever heard: {s}");
		assert_eq!(s["reachability"], "gone");
	})
	.await
}

/// `get_restore_replica` took the group's 50 newest checks and *then* kept
/// the ones belonging to this replica — filter-after-limit. A replica
/// checked rarely, alongside a chatty one in the same group, had every one
/// of its reports pushed out of the window and read as never checked.
#[tokio::test(flavor = "multi_thread")]
async fn get_restore_replica_checks_are_the_replicas_own_newest() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed_backup_runs(&mut conn).await;
		seed_restore_replicas(&mut conn).await;

		// The group-wide replica reports rarely; give it one old check, then
		// bury it under a run of newer checks from its noisy neighbour.
		let mut rows = format!(
			"INSERT INTO backup_restore_checks \
			 (replica_id, consumer_device_id, group_id, machine_id, type, intent, snapshot_id, outcome, replica_healthy, observed_at) \
			 VALUES ('{REPLICA_GROUP_WIDE}', '{CONSUMER}', '{RGROUP}', NULL, 'tamanu-postgres', 'verify', 'quiet-snap', 'success', true, NOW() - interval '10 days');"
		);
		for i in 0..60 {
			rows.push_str(&format!(
				"INSERT INTO backup_restore_checks \
				 (replica_id, consumer_device_id, group_id, machine_id, type, intent, snapshot_id, outcome, replica_healthy, observed_at) \
				 VALUES ('{REPLICA_SERVER_SCOPED}', '{CONSUMER}', '{RGROUP}', '{RSERVER}', 'tamanu-postgres', 'verify', 'noisy-{i}', 'success', true, NOW() - interval '{i} minutes');"
			));
		}
		conn.batch_execute(&rows).await.expect("seed noisy checks");

		let detail = call_tool!(
			private,
			"get_restore_replica",
			serde_json::json!({ "replica_id": REPLICA_GROUP_WIDE })
		);
		let checks = detail["recent_checks"].as_array().expect("recent_checks");
		assert_eq!(
			checks.len(),
			1,
			"the quiet replica's own check must survive a noisy neighbour: {detail}",
		);
		assert_eq!(checks[0]["snapshot_id"], "quiet-snap");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn upgrade_plans_list_the_open_ones_and_keep_the_withdrawn_in_history() {
	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES \
				('44444444-4444-4444-4444-444444444444', 'Drifting'); \
			 WITH m AS (INSERT INTO machines (id, group_id) VALUES ('44444444-4444-4444-4444-4444444444a1', '44444444-4444-4444-4444-444444444444') RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES \
				('44444444-4444-4444-4444-4444444444a1', 'https://drifting', 'tamanu-central', 'production', '44444444-4444-4444-4444-444444444444', '44444444-4444-4444-4444-4444444444a1'); \
			 INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES \
				('44444444-4444-4444-4444-4444444444a1', 'test', '{{}}'::jsonb, '2.34.1'); \
			 INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES \
				('55555555-5555-5555-5555-555555555555', 2, 36, 0, 'x', 'published'), \
				('66666666-6666-6666-6666-666666666666', 2, 40, 0, 'x', 'published'); \
			 INSERT INTO upgrade_plans (group_id, rank, target_version_id, created_by, withdrawn_at, withdrawn_by) VALUES \
				('{GROUP}', 'production', '66666666-6666-6666-6666-666666666666', 'someone@example.com', NOW(), 'someone@example.com'); \
			 INSERT INTO upgrade_plans (group_id, rank, target_version_id, planned_for, note, created_by) VALUES \
				('{GROUP}', 'production', '55555555-5555-5555-5555-555555555555', DATE '2020-01-01', 'site can absorb 2.36 only', 'someone@example.com');"
		))
		.await
		.expect("seed plans");

		let list = call_tool!(private, "list_upgrade_plans", serde_json::json!({}));
		let plans = list["plans"].as_array().expect("plans");
		assert_eq!(plans.len(), 1, "one group has an open plan: {list}");
		assert_eq!(plans[0]["group_name"], "Prod Group");
		assert_eq!(plans[0]["rank"], "production");
		assert_eq!(plans[0]["current_version"], "2.34.1");
		assert_eq!(plans[0]["target_version"], "2.36.0");
		assert_eq!(plans[0]["planned_for"], "2020-01-01");
		assert_eq!(plans[0]["late"], true, "the planned day has passed unmet");
		assert_eq!(plans[0]["note"], "site can absorb 2.36 only");

		// A production with nothing recorded gets no pre-upgrade testing, so it
		// is returned rather than omitted.
		let unplanned = list["environments_without_a_plan"]
			.as_array()
			.expect("unplanned environments");
		assert!(
			unplanned.iter().any(|g| g["group_name"] == "Drifting"),
			"missing the unplanned group: {list}"
		);

		let history = call_tool!(
			private,
			"get_upgrade_plan_history",
			serde_json::json!({ "group_id": GROUP })
		);
		let plans = history["plans"].as_array().expect("history");
		assert_eq!(plans.len(), 2);
		let withdrawn = plans
			.iter()
			.find(|p| p["outcome"] == "withdrawn")
			.unwrap_or_else(|| panic!("no withdrawn plan in {history}"));
		assert_eq!(withdrawn["target_version"], "2.40.0");
		assert_eq!(withdrawn["rank"], "production");
		assert_eq!(withdrawn["withdrawn_by"], "someone@example.com");
		assert!(!withdrawn["ended_at"].is_null());
		assert!(plans.iter().any(|p| p["outcome"] == "open"));
	})
	.await
}

const MGROUP: &str = "dddddddd-0000-0000-0000-000000000001";
const MACHINE: &str = "dddddddd-0000-0000-0000-000000000002";
const MAPP_A: &str = "dddddddd-0000-0000-0000-00000000000a";
const MAPP_B: &str = "dddddddd-0000-0000-0000-00000000000b";

/// One box carrying two workloads, with figures split by grain: the machine
/// reports platform and hardware, the applications report versions.
async fn seed_two_workload_box(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{MGROUP}', 'Split Group'); \
		 INSERT INTO machines (id, group_id, name, cloud) VALUES \
			('{MACHINE}', '{MGROUP}', 'box-one', false); \
		 INSERT INTO applications (id, host, name, type, group_id, machine_id) VALUES \
			('{MAPP_A}', 'https://front', 'Front', 'tamanu-central', '{MGROUP}', '{MACHINE}'), \
			('{MAPP_B}', 'https://worker', 'Worker', 'tamanu-central', '{MGROUP}', '{MACHINE}'); \
		 INSERT INTO machine_reported_detail (machine_id, source, extra, reported_at) VALUES \
			('{MACHINE}', 'alertd', \
			 '{{\"osName\":\"Debian\",\"osVersion\":\"12\",\"hostname\":\"box-one.internal\",\
			   \"cpuCores\":8,\"totalMemoryBytes\":16000000000,\"uptimeSecs\":86400}}'::jsonb, NOW()); \
		 INSERT INTO application_reported_detail (application_id, source, extra, reported_at) VALUES \
			('{MAPP_A}', 'alertd', '{{\"tamanuVersion\":\"2.62.0\",\"pgVersion\":\"PostgreSQL 16.2\"}}'::jsonb, NOW());"
	))
	.await
	.expect("seed two-workload box");
}

/// The grains carry different figures, and each names the other side.
// spec: MCP#detail
#[tokio::test(flavor = "multi_thread")]
async fn get_machine_carries_hardware_and_get_server_carries_version() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed_two_workload_box(&mut conn).await;

		let machine = call_tool!(
			private,
			"get_machine",
			serde_json::json!({ "machine_id": MACHINE })
		);
		assert_eq!(machine["name"], "box-one");
		let figures = &machine["figures"];
		assert_eq!(figures["platform"], "Debian 12");
		assert_eq!(figures["hostname"], "box-one.internal");
		assert_eq!(figures["cpu_cores"], 8);
		assert_eq!(figures["total_memory_bytes"], 16000000000u64);
		assert_eq!(figures["uptime_seconds"], 86400);
		assert!(
			figures.get("postgres_version").is_none(),
			"a database engine is the software's, not the box's: {figures}"
		);

		// The box names the workloads on it.
		let on_box = machine["applications"].as_array().expect("applications");
		assert_eq!(on_box.len(), 2, "both workloads: {on_box:?}");

		let application = call_tool!(
			private,
			"get_server",
			serde_json::json!({ "server_id": MAPP_A })
		);
		assert_eq!(
			application["machine_id"], MACHINE,
			"and the workload names its box, so a client can cross over"
		);
		assert_eq!(application["figures"]["postgres_version"], "16.2");
	})
	.await
}

/// A box hosting two workloads is one machine with a count of two, which is
/// the case the grain exists for.
// spec: MCP#discovery
#[tokio::test(flavor = "multi_thread")]
async fn find_machines_reports_how_many_applications_each_carries() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed_two_workload_box(&mut conn).await;

		let all = call_tool!(private, "find_machines", serde_json::json!({}));
		let machines = all["machines"].as_array().expect("machines");
		let found = machines
			.iter()
			.find(|m| m["id"] == MACHINE)
			.expect("the seeded box");
		assert_eq!(found["application_count"], 2);
		assert_eq!(found["platform"], "Debian 12");
		assert_eq!(found["group_name"], "Split Group");

		// The reported hostname is searchable, not just the operator's name.
		let by_hostname = call_tool!(
			private,
			"find_machines",
			serde_json::json!({ "query": "box-one.internal" })
		);
		assert_eq!(by_hostname["machines"].as_array().unwrap().len(), 1);

		let by_platform = call_tool!(
			private,
			"find_machines",
			serde_json::json!({ "platform": "debian" })
		);
		assert!(
			by_platform["machines"]
				.as_array()
				.unwrap()
				.iter()
				.any(|m| m["id"] == MACHINE)
		);
	})
	.await
}

/// A box's disk filling is the application's problem too, so asking about an
/// application returns its machine's issues among its own. Asking about the
/// machine returns only the machine's.
// spec: MCP#incidents-and-issues
#[tokio::test(flavor = "multi_thread")]
async fn find_issues_by_application_includes_its_machines_issues() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed_two_workload_box(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO check_policies (source, subject, application_type, check_name) VALUES \
				('alertd', 'machine', NULL, 'disk_free'), \
				('alertd', 'application', 'tamanu-central', 'tamanu_version'); \
			 INSERT INTO issues \
				(machine_id, source, ref, check_name, observed_result, effective_result, \
				 message, active, first_seen, last_seen, degraded_since, last_degraded_at) \
				VALUES ('{MACHINE}', 'alertd', 'disk', 'disk_free', 'failed', 'failed', \
				 'disk full', true, NOW(), NOW(), NOW(), NOW()); \
			 INSERT INTO issues \
				(application_id, source, ref, check_name, observed_result, effective_result, \
				 message, active, first_seen, last_seen, degraded_since, last_degraded_at) \
				VALUES ('{MAPP_A}', 'alertd', 'ver', 'tamanu_version', 'warning', 'warning', \
				 'behind', true, NOW(), NOW(), NOW(), NOW());"
		))
		.await
		.expect("seed issues");

		let for_app = call_tool!(
			private,
			"find_issues",
			serde_json::json!({ "application_id": MAPP_A })
		);
		let refs: Vec<&str> = for_app["issues"]
			.as_array()
			.expect("issues")
			.iter()
			.map(|i| i["ref"].as_str().unwrap())
			.collect();
		assert!(
			refs.contains(&"disk") && refs.contains(&"ver"),
			"the box's disk and the software's version both: {refs:?}"
		);

		// And each says whose failure it is.
		let disk = for_app["issues"]
			.as_array()
			.unwrap()
			.iter()
			.find(|i| i["ref"] == "disk")
			.expect("the disk issue");
		assert_eq!(disk["scope"]["grain"], "machine");
		assert_eq!(disk["scope"]["name"], "box-one");

		// The machine's own view is the box's checks only.
		let for_machine = call_tool!(
			private,
			"find_issues",
			serde_json::json!({ "machine_id": MACHINE })
		);
		let machine_refs: Vec<&str> = for_machine["issues"]
			.as_array()
			.expect("issues")
			.iter()
			.map(|i| i["ref"].as_str().unwrap())
			.collect();
		assert_eq!(
			machine_refs,
			vec!["disk"],
			"asking what is wrong with the box does not answer about its software"
		);

		// The other workload on the same box sees the disk too: it is that
		// box's failure, and both workloads run on it.
		let for_sibling = call_tool!(
			private,
			"find_issues",
			serde_json::json!({ "application_id": MAPP_B })
		);
		assert!(
			for_sibling["issues"]
				.as_array()
				.unwrap()
				.iter()
				.any(|i| i["ref"] == "disk")
		);
	})
	.await
}

/// An agent reading the fleet through MCP and an operator reading it in the UI
/// must not be told different things about the same box. Both go through the
/// same two rollups, one per grain, and this pins that they stay the same two.
/// The grains are given different answers to tell apart: the box only warns,
/// while the workload on it has failed a check of its own.
/// spec: MCP, FLT
#[tokio::test(flavor = "multi_thread")]
async fn mcp_health_matches_what_the_ui_presents() {
	use commons_types::status::CheckResult;
	use database::issues::{CheckFiling, Scope, file_check};

	commons_tests::server::run(async |mut conn, _public, private| {
		seed(&mut conn).await;
		let machine_id: uuid::Uuid = SRV_GROUPED.parse().unwrap();
		let application_id: uuid::Uuid = SRV_GROUPED.parse().unwrap();

		let filing = |scope, check, observed| CheckFiling {
			source: "alertd",
			scope,
			device_id: None,
			check,
			observed,
			title: Some("parity"),
			message: "parity filing",
			detail: None,
			default_ceiling: CheckResult::Failed,
			default_escalates: false,
			documentation: None,
		};
		file_check(
			&mut conn,
			filing(
				Scope::Machine(machine_id),
				"disk_free",
				CheckResult::Warning,
			),
		)
		.await
		.expect("machine filing");
		file_check(
			&mut conn,
			filing(
				Scope::Application(application_id),
				"db",
				CheckResult::Failed,
			),
		)
		.await
		.expect("application filing");

		let card = private
			.post("/api/statuses/group_details")
			.json(&serde_json::json!({ "server_group_id": GROUP }))
			.await;
		assert_eq!(card.status_code().as_u16(), 200);
		let card: serde_json::Value = card.json();
		let member = card["members"]
			.as_array()
			.expect("members")
			.iter()
			.find(|m| m["id"] == SRV_GROUPED)
			.expect("the grouped member")
			.clone();

		let machine = call_tool!(
			private,
			"get_machine",
			serde_json::json!({ "machine_id": SRV_GROUPED })
		);
		let application = call_tool!(
			private,
			"get_server",
			serde_json::json!({ "server_id": SRV_GROUPED })
		);

		assert_eq!(member["machine_health"], "warning");
		assert_eq!(member["health"], "unhealthy");
		assert_eq!(machine["health"], member["machine_health"]);
		assert_eq!(application["health"], member["health"]);

		// And the box's own listing of what runs on it agrees with both.
		let on_box = machine["applications"]
			.as_array()
			.expect("applications")
			.iter()
			.find(|a| a["id"] == SRV_GROUPED)
			.expect("the workload");
		assert_eq!(on_box["health"], member["health"]);
	})
	.await
}

const ENV_GROUP: &str = "aaaaaaaa-0000-0000-0000-0000000000e0";
const ENV_TEST_APP: &str = "aaaaaaaa-0000-0000-0000-0000000000e1";
const ENV_TEST_INC: &str = "aaaaaaaa-0000-0000-0000-0000000000e2";
const ENV_GROUP_INC: &str = "aaaaaaaa-0000-0000-0000-0000000000e3";

/// An incident targets one of a group's environments, and both tools report
/// which. An agent summarising the fleet reads these, and a site's test box
/// counted as the site's production trouble is a wrong answer with no way for
/// the reader to tell.
///
/// spec: INC#targets
#[tokio::test(flavor = "multi_thread")]
async fn incidents_name_the_environment_they_target() {
	commons_tests::server::run(async |mut conn, _public, private| {
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{ENV_GROUP}', 'kamaka'); \
			 WITH m AS (INSERT INTO machines (id, group_id) VALUES ('{ENV_TEST_APP}', '{ENV_GROUP}') RETURNING id) \
			 INSERT INTO applications (id, host, name, type, rank, group_id, is_monitored, machine_id) VALUES \
				('{ENV_TEST_APP}', 'https://test.kamaka', 'kamaka test', 'tamanu-central', 'test', '{ENV_GROUP}', true, '{ENV_TEST_APP}'); \
			 INSERT INTO incidents (id, created_at, updated_at, server_group_id, rank, opened_at) VALUES \
				('{ENV_TEST_INC}', NOW(), NOW(), '{ENV_GROUP}', 'test', NOW() - interval '1 hour'), \
				('{ENV_GROUP_INC}', NOW(), NOW(), '{ENV_GROUP}', NULL, NOW() - interval '1 hour');"
		))
		.await
		.expect("seed an environment incident beside a group one");

		let found = call_tool!(private, "find_incidents", serde_json::json!({}));
		let by_id = |id: &str| {
			found["incidents"]
				.as_array()
				.unwrap()
				.iter()
				.find(|i| i["id"] == id)
				.unwrap_or_else(|| panic!("{id} missing from find_incidents"))
				.clone()
		};
		assert_eq!(
			by_id(ENV_TEST_INC)["rank"], "test",
			"the environment's incident says which environment"
		);
		assert!(
			by_id(ENV_GROUP_INC)["rank"].is_null(),
			"and the group's own carries no rank, which is how a reader tells them apart"
		);
		assert_eq!(by_id(ENV_TEST_INC)["group_name"], "kamaka");

		let detail = call_tool!(
			private,
			"get_incident",
			serde_json::json!({ "incident_id": ENV_TEST_INC })
		);
		assert_eq!(
			detail["rank"], "test",
			"the detail carries it too, since an agent may fetch one without listing"
		);
		let group_detail = call_tool!(
			private,
			"get_incident",
			serde_json::json!({ "incident_id": ENV_GROUP_INC })
		);
		assert!(group_detail["rank"].is_null());
	})
	.await
}
