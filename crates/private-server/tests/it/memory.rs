//! The operator surface of incident memory and of notes history: any tailnet
//! user reads, administrators write, and each write is audited without its
//! text (specs `MEM`, `FLT`).

use commons_tests::diesel_async::SimpleAsyncConnection;
use database::{
	admins::Admin,
	audit::{self, Entry, Filter},
};
use serde_json::{Value, json};
use uuid::Uuid;

const GROUP: &str = "dddddddd-0000-0000-0000-000000000001";
const BOX: &str = "dddddddd-0000-0000-0000-0000000000b1";
const APP: &str = "dddddddd-0000-0000-0000-0000000000a1";

async fn seed(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka');
		 INSERT INTO machines (name, id, group_id) VALUES ('box', '{BOX}', '{GROUP}');
		 INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
			('{APP}', 'https://kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{BOX}');"
	))
	.await
	.unwrap();
}

async fn log(conn: &mut database::diesel_async::AsyncPgConnection, action: &str) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some(action.to_owned()),
			limit: 100,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entry_is_recorded_amended_retired_and_restored_with_each_audited() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		let recorded: Value = private
			.post("/api/memory/record")
			.json(&json!({
				"title": "kamaka needs the VPN",
				"body": "Join the site network first.",
				"link": { "kind": "group", "id": GROUP },
			}))
			.await
			.json();
		let id = recorded["id"].as_str().unwrap().to_owned();
		assert_eq!(recorded["revision"], 1);
		assert_eq!(recorded["link"]["name"], "kamaka");
		assert_eq!(recorded["created_by"], "admin@localhost");

		let amend = |revision: i64| {
			private.post("/api/memory/amend").json(&json!({
				"id": id, "revision": revision, "body": "Join the site network, then the VPN.",
			}))
		};
		let amended = amend(1).await;
		amended.assert_status_ok();
		assert_eq!(amended.json::<Value>()["revision"], 2);
		amend(1)
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);

		let retired = private
			.post("/api/memory/retire")
			.json(&json!({ "id": id, "reason": "site decommissioned" }))
			.await;
		retired.assert_status_ok();
		assert!(!retired.json::<Value>()["retired_at"].is_null());

		let hidden: Value = private
			.post("/api/memory/list")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(hidden["total"], 0);
		let shown: Value = private
			.post("/api/memory/list")
			.json(&json!({ "include_retired": true, "text": "vpn" }))
			.await
			.json();
		assert_eq!(shown["total"], 1);

		let restored = private
			.post("/api/memory/restore")
			.json(&json!({ "id": id, "revision": 3, "to_revision": 1 }))
			.await;
		restored.assert_status_ok();
		let restored: Value = restored.json();
		assert_eq!(restored["revision"], 4);
		assert_eq!(restored["body"], "Join the site network first.");
		assert!(restored["retired_at"].is_null());

		let detail: Value = private
			.post("/api/memory/get")
			.json(&json!({ "id": id }))
			.await
			.json();
		let kinds: Vec<&str> = detail["history"]
			.as_array()
			.unwrap()
			.iter()
			.map(|h| h["kind"].as_str().unwrap())
			.collect();
		assert_eq!(kinds, ["restored", "retired", "amended", "recorded"]);

		for action in [
			"memory_entry_recorded",
			"memory_entry_amended",
			"memory_entry_retired",
			"memory_entry_restored",
		] {
			let entries = log(&mut conn, action).await;
			assert_eq!(entries.len(), 1, "{action}");
			let rendered = serde_json::to_string(&entries[0].detail).unwrap();
			assert!(
				!rendered.contains("VPN") && !rendered.contains("network"),
				"{rendered}"
			);
		}

		private
			.post("/api/memory/get")
			.json(&json!({ "id": Uuid::new_v4() }))
			.await
			.assert_status_not_found();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn writes_are_for_administrators_and_reads_for_any_tailnet_user() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var("CANOPY_TRUST_TAILSCALE_HEADERS", "1") };

	commons_tests::server::run(async |mut conn, _, private| {
		Admin::add(&mut conn, "boss@example.com").await.unwrap();
		let session = private
			.post("/api/safety/session")
			.add_header("Tailscale-User-Login", "boss@example.com")
			.add_header("Tailscale-User-Name", "Boss")
			.json(&json!({}))
			.await;
		session.assert_status_ok();
		let session = session.json::<Value>()["id"].as_str().unwrap().to_owned();
		private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", "boss@example.com")
			.add_header("Tailscale-User-Name", "Boss")
			.add_header("x-canopy-session", &session)
			.json(&json!({"mode": "write"}))
			.await
			.assert_status_ok();
		let boss = [
			("Tailscale-User-Login", "boss@example.com"),
			("Tailscale-User-Name", "Boss"),
			(
				"x-canopy-session",
				Box::leak(session.into_boxed_str()) as &str,
			),
		];
		let user = [
			("Tailscale-User-Login", "user@example.com"),
			("Tailscale-User-Name", "User"),
			("x-canopy-session", "00000000-0000-0000-0000-000000000000"),
		];
		let with = |path: &str, who: [(&'static str, &'static str); 3], body: Value| {
			let mut request = private.post(path);
			for (name, value) in who {
				request = request.add_header(name, value);
			}
			request.json(&body)
		};

		let recorded = with(
			"/api/memory/record",
			boss,
			json!({ "title": "t", "body": "b" }),
		)
		.await;
		recorded.assert_status_ok();
		let id = recorded.json::<Value>()["id"].as_str().unwrap().to_owned();

		for (path, body) in [
			("/api/memory/record", json!({ "title": "t", "body": "b" })),
			(
				"/api/memory/amend",
				json!({ "id": id, "revision": 1, "title": "x" }),
			),
			("/api/memory/retire", json!({ "id": id, "reason": "r" })),
			(
				"/api/memory/restore",
				json!({ "id": id, "revision": 1, "to_revision": 1 }),
			),
		] {
			with(path, user, body).await.assert_status_forbidden();
		}
		with("/api/memory/list", user, json!({}))
			.await
			.assert_status_ok();
		with("/api/memory/get", user, json!({ "id": id }))
			.await
			.assert_status_ok();
		assert_eq!(log(&mut conn, "memory_entry_recorded").await.len(), 1);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_change_to_notes_is_kept_in_the_log_and_audited_without_the_text() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		private
			.post("/api/fleet/groups/update")
			.json(&json!({ "server_group_id": GROUP, "data": { "notes": "key is in the safe" } }))
			.await
			.assert_status_ok();
		// The same text again records nothing.
		private
			.post("/api/fleet/groups/update")
			.json(&json!({ "server_group_id": GROUP, "data": { "notes": "key is in the safe" } }))
			.await
			.assert_status_ok();
		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "notes": "rack 4" }))
			.await
			.assert_status_ok();
		private
			.post("/api/fleet/applications/update")
			.json(&json!({ "server_id": APP, "data": { "notes": "primary" } }))
			.await
			.assert_status_ok();

		for (path, key, id, expect) in [
			(
				"/api/fleet/groups/notes_history",
				"server_group_id",
				GROUP,
				"key is in the safe",
			),
			(
				"/api/fleet/machines/notes_history",
				"machine_id",
				BOX,
				"rack 4",
			),
			(
				"/api/fleet/applications/notes_history",
				"server_id",
				APP,
				"primary",
			),
		] {
			let history: Value = private.post(path).json(&json!({ key: id })).await.json();
			let history = history.as_array().unwrap();
			assert_eq!(history.len(), 1, "{path}");
			assert_eq!(history[0]["notes"], expect);
			assert_eq!(history[0]["author"], "admin@localhost");
			assert_eq!(history[0]["via"], "operator");
		}

		let entries = log(&mut conn, "notes_changed").await;
		assert_eq!(entries.len(), 3, "one per record, none for the repeat");
		for entry in entries {
			let rendered = serde_json::to_string(&entry.detail).unwrap();
			assert!(
				!rendered.contains("safe") && !rendered.contains("rack"),
				"{rendered}"
			);
		}

		private
			.post("/api/fleet/groups/notes_history")
			.json(&json!({ "server_group_id": Uuid::new_v4() }))
			.await
			.assert_status_not_found();
	})
	.await
}
