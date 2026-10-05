//! Operator presence (from the `external_users` healthcheck) on the
//! group_details, get_detail, and snapshot endpoints.

use commons_tests::diesel_async::SimpleAsyncConnection;
use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
struct OperatorResponse {
	login: String,
	name: Option<String>,
	profile_pic: Option<String>,
	connected_since: Option<String>,
}

#[derive(Debug, Deserialize)]
struct GroupCardResponse {
	members: Vec<GroupMemberResponse>,
}

#[derive(Debug, Deserialize)]
struct GroupMemberResponse {
	name: String,
	operators: Vec<OperatorResponse>,
}

#[derive(Debug, Deserialize)]
struct DetailResponse {
	last_status: Option<LastStatusResponse>,
}

#[derive(Debug, Deserialize)]
struct LastStatusResponse {
	operators: Vec<OperatorResponse>,
}

#[derive(Debug, Deserialize)]
struct SnapshotResponse {
	operators: Vec<OperatorResponse>,
}

/// `health[]` payload with three sessions for alice+bob (alice twice, the
/// earlier `connected_since` second so dedupe has to pick it out of order)
/// and one local session without a Tailscale identity.
const HEALTH: &str = r#"[{"check":"external_users","result":"passed","count":4,"users":[
	{"name":"ubuntu","line":"pts/0","source":"100.64.0.1","tailscale":"alice@example.com","connected_since":"2026-06-01T03:56:40Z"},
	{"name":"ubuntu","line":"pts/1","source":"100.64.0.1","tailscale":"alice@example.com","connected_since":"2026-06-01T02:00:00Z"},
	{"name":"ubuntu","line":"pts/2","source":"100.64.0.2","tailscale":"bob@example.com","connected_since":"2026-06-01T04:00:00Z"},
	{"name":"root","line":"tty1"}
]}]"#;

#[tokio::test(flavor = "multi_thread")]
async fn group_details_dedupes_enriches_and_gates_operators() {
	commons_tests::server::run(async |mut conn, _, private| {
		// Two applications in one group: Fresh reports now (operators must show,
		// alice enriched from the cache), Stale reported 45 minutes ago
		// (its sessions can't claim active presence).
		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, status, changelog, created_at) VALUES
			('00000000-0000-0000-0000-000000000001', 1, 0, 0, 'published', 'Test version', NOW());
			INSERT INTO server_groups (id, name) VALUES
			('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'Cluster');
			INSERT INTO machines (name, id, group_id) VALUES
			('box', '11111111-1111-1111-1111-111111111111', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa'),
			('box', '22222222-2222-2222-2222-222222222222', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa');
			INSERT INTO applications (id, name, host, rank, type, group_id, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Fresh', 'https://fresh.example.com', 'production', 'tamanu-central', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', '11111111-1111-1111-1111-111111111111'),
			('22222222-2222-2222-2222-222222222222', 'Stale', 'https://stale.example.com', 'production', 'tamanu-facility', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', '22222222-2222-2222-2222-222222222222');
			INSERT INTO tailscale_users (login, name, profile_pic) VALUES
			('alice@example.com', 'Alice Example', 'https://pics.example.com/alice.png');
			INSERT INTO statuses (server_id, created_at, health) VALUES
			('11111111-1111-1111-1111-111111111111', NOW(), '{HEALTH}'::jsonb),
			('22222222-2222-2222-2222-222222222222', NOW() - INTERVAL '45 minutes', '{HEALTH}'::jsonb)",
		))
		.await
		.unwrap();

		let response = private
			.post("/api/statuses/group_details")
			.json(&serde_json::json!({"server_group_id": "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa"}))
			.await;
		response.assert_status_ok();
		let card: GroupCardResponse = response.json();

		let fresh = card.members.iter().find(|m| m.name == "Fresh").unwrap();
		assert_eq!(
			fresh.operators,
			vec![
				OperatorResponse {
					login: "alice@example.com".into(),
					name: Some("Alice Example".into()),
					profile_pic: Some("https://pics.example.com/alice.png".into()),
					// Earliest across alice's two sessions.
					connected_since: Some("2026-06-01T02:00:00Z".into()),
				},
				OperatorResponse {
					login: "bob@example.com".into(),
					name: None,
					profile_pic: None,
					connected_since: Some("2026-06-01T04:00:00Z".into()),
				},
			],
		);

		let stale = card.members.iter().find(|m| m.name == "Stale").unwrap();
		assert!(
			stale.operators.is_empty(),
			"a stale push can't claim active presence",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn get_detail_last_status_carries_operators() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, status, changelog, created_at) VALUES
			('00000000-0000-0000-0000-000000000001', 1, 0, 0, 'published', 'Test version', NOW());
			INSERT INTO machines (name, id) VALUES
			('box', '11111111-1111-1111-1111-111111111111');
			INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Application', 'https://s.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111');
			INSERT INTO statuses (server_id, created_at, health) VALUES
			('11111111-1111-1111-1111-111111111111', NOW(), '{HEALTH}'::jsonb)",
		))
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/applications/get_detail")
			.json(&serde_json::json!({"server_id": "11111111-1111-1111-1111-111111111111"}))
			.await;
		response.assert_status_ok();
		let detail: DetailResponse = response.json();
		let status = detail.last_status.unwrap();
		let logins: Vec<&str> = status.operators.iter().map(|o| o.login.as_str()).collect();
		assert_eq!(logins, ["alice@example.com", "bob@example.com"]);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_operators_are_not_freshness_gated() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id) VALUES
			('box', '11111111-1111-1111-1111-111111111111');
			INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Application', 'https://s.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111');
			INSERT INTO statuses (server_id, created_at, health) VALUES
			('11111111-1111-1111-1111-111111111111', NOW() - INTERVAL '2 hours', '{HEALTH}'::jsonb)",
		))
		.await
		.unwrap();

		let response = private
			.post("/api/statuses/snapshot")
			.json(&serde_json::json!({"server_id": "11111111-1111-1111-1111-111111111111"}))
			.await;
		response.assert_status_ok();
		let snap: Option<SnapshotResponse> = response.json();
		let snap = snap.expect("snapshot exists");
		assert_eq!(snap.operators.len(), 2, "as-of data, no freshness gate");
	})
	.await
}
