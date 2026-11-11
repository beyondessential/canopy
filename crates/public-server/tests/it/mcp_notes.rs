//! Writing notes and incident memory through the fleet query interface: held
//! by the notes grant, refused with the current state on a stale revision,
//! limited to what agents wrote, and audited without the text.

use axum_test::TestServer;
use database::{
	audit::{self, Actor, Filter},
	mcp_tokens::McpToken,
	notes::{AuthorKind, IncidentNote},
};
use diesel_async::{AsyncPgConnection, SimpleAsyncConnection as _};
use serde_json::{Value, json};
use uuid::Uuid;

const ACCEPT: &str = "application/json, text/event-stream";
const PROTO: &str = "2025-06-18";

async fn call(public: &TestServer, token: &str, tool: &str, arguments: Value) -> Value {
	let resp = public
		.post("/mcp")
		.add_header("accept", ACCEPT)
		.add_header("mcp-protocol-version", PROTO)
		.add_header("authorization", format!("Bearer {token}"))
		.json(&json!({
			"jsonrpc": "2.0", "id": 2, "method": "tools/call",
			"params": { "name": tool, "arguments": arguments }
		}))
		.await;
	serde_json::from_str(&resp.text()).expect("json body")
}

fn ok(envelope: &Value) -> &Value {
	assert!(envelope.get("error").is_none(), "rpc error: {envelope}");
	assert_ne!(envelope["result"]["isError"], json!(true), "{envelope}");
	&envelope["result"]["structuredContent"]
}

fn refused(envelope: &Value) -> bool {
	envelope.get("error").is_some() || envelope["result"]["isError"] == json!(true)
}

async fn noting(conn: &mut AsyncPgConnection) -> (McpToken, String) {
	McpToken::mint(conn, "scribe", "test@example.com", &["notes".to_owned()])
		.await
		.unwrap()
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

struct Seeded {
	group: Uuid,
	machine: Uuid,
	issue: Uuid,
	incident: Uuid,
}

async fn seed(conn: &mut AsyncPgConnection) -> Seeded {
	let s = Seeded {
		group: Uuid::new_v4(),
		machine: Uuid::new_v4(),
		issue: Uuid::new_v4(),
		incident: Uuid::new_v4(),
	};
	let device = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{device}', 'machine'); \
		 INSERT INTO server_groups (id, name) VALUES ('{g}', 'Pacific'); \
		 INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{m}', '{g}', '{device}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
			('{m}', 'https://example.com', 'tamanu-central', '{g}', 'production', '{m}'); \
		 INSERT INTO issues \
			(id, application_id, device_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen) \
		   VALUES ('{i}', '{m}', '{device}', 'alertd', 'r', 'disk-space', 'failed', 'failed', 'm', true, NOW(), NOW()); \
		 INSERT INTO incidents (id, server_group_id, rank, opened_at) VALUES ('{inc}', '{g}', 'production', NOW());",
		g = s.group,
		m = s.machine,
		i = s.issue,
		inc = s.incident,
	))
	.await
	.expect("seed");
	s
}

#[tokio::test(flavor = "multi_thread")]
async fn a_token_without_the_notes_grant_is_refused_every_write_with_no_effect() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let s = seed(&mut conn).await;
		let (_, token) = McpToken::mint(&mut conn, "reader", "test@example.com", &[])
			.await
			.unwrap();
		let note = IncidentNote::add(&mut conn, s.incident, "bot", AuthorKind::Agent, "x")
			.await
			.unwrap();

		for (tool, arguments) in [
			(
				"add_issue_note",
				json!({ "issue_id": s.issue, "body": "hi" }),
			),
			(
				"add_incident_note",
				json!({ "incident_id": s.incident, "body": "hi" }),
			),
			(
				"edit_note",
				json!({ "note_id": note.id, "revision": 1, "body": "hi" }),
			),
			("delete_note", json!({ "note_id": note.id })),
			(
				"replace_notes",
				json!({ "id": s.group, "notes": "hi", "revision": 0 }),
			),
			("record_memory_entry", json!({ "title": "t", "body": "b" })),
			(
				"amend_memory_entry",
				json!({ "id": Uuid::new_v4(), "revision": 1, "title": "t" }),
			),
			(
				"retire_memory_entry",
				json!({ "id": Uuid::new_v4(), "reason": "r" }),
			),
			(
				"restore_memory_entry",
				json!({ "id": Uuid::new_v4(), "revision": 1, "to_revision": 1 }),
			),
		] {
			let env = call(&public, &token, tool, arguments).await;
			assert!(env.get("error").is_some(), "{tool} refused: {env}");
		}
		for action in [
			"issue_note_added",
			"incident_note_added",
			"note_edited",
			"note_deleted",
			"notes_changed",
			"memory_entry_recorded",
		] {
			assert!(logged(&mut conn, action).await.is_empty(), "{action}");
		}
		let kept = IncidentNote::get(&mut conn, note.id).await.unwrap();
		assert!(kept.deleted_at.is_none() && kept.body == "x");

		// A reading token still reads.
		let read = call(&public, &token, "get_memory_index", json!({})).await;
		ok(&read);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_adds_edits_and_deletes_its_own_notes_but_not_an_operators() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let s = seed(&mut conn).await;
		let (token, plaintext) = noting(&mut conn).await;
		let operator_note =
			IncidentNote::add(&mut conn, s.incident, "alice", AuthorKind::Operator, "mine")
				.await
				.unwrap();

		let added = call(
			&public,
			&plaintext,
			"add_incident_note",
			json!({ "incident_id": s.incident, "body": "disk is 98% full" }),
		)
		.await;
		let note = ok(&added).clone();
		assert_eq!(note["author_kind"], "agent");
		assert_eq!(note["author"], "scribe");
		let id = note["id"].as_str().unwrap().to_owned();

		let edited = call(
			&public,
			&plaintext,
			"edit_note",
			json!({ "note_id": id, "revision": 1, "body": "disk is 99% full" }),
		)
		.await;
		assert_eq!(ok(&edited)["revision"], 2);

		let stale = call(
			&public,
			&plaintext,
			"edit_note",
			json!({ "note_id": id, "revision": 1, "body": "again" }),
		)
		.await;
		assert!(refused(&stale));
		assert_eq!(
			stale["result"]["structuredContent"]["current"]["revision"],
			2
		);
		assert_eq!(
			stale["result"]["structuredContent"]["current"]["body"],
			"disk is 99% full"
		);
		let text = stale["result"]["content"][1]["text"].as_str().unwrap();
		assert!(
			text.contains("disk is 99% full"),
			"the text carries the current note too: {text}"
		);

		for tool in ["edit_note", "delete_note"] {
			let env = call(
				&public,
				&plaintext,
				tool,
				json!({ "note_id": operator_note.id, "revision": 1, "body": "x" }),
			)
			.await;
			assert!(refused(&env), "{tool}: {env}");
			let message = env["result"]["content"][0]["text"].as_str().unwrap();
			assert!(message.contains("operator"), "names the reason: {message}");
		}
		assert!(
			IncidentNote::get(&mut conn, operator_note.id)
				.await
				.unwrap()
				.deleted_at
				.is_none()
		);

		let incident = call(
			&public,
			&plaintext,
			"get_incident",
			json!({ "incident_id": s.incident }),
		)
		.await;
		let notes = ok(&incident)["notes"].as_array().unwrap().clone();
		assert_eq!(notes.len(), 2);
		assert!(notes.iter().any(|n| n["author_kind"] == "operator"));

		let deleted = call(&public, &plaintext, "delete_note", json!({ "note_id": id })).await;
		ok(&deleted);
		let incident = call(
			&public,
			&plaintext,
			"get_incident",
			json!({ "incident_id": s.incident }),
		)
		.await;
		assert_eq!(ok(&incident)["notes"].as_array().unwrap().len(), 1);

		let issue_note = call(
			&public,
			&plaintext,
			"add_issue_note",
			json!({ "issue_id": s.issue, "body": "seen before" }),
		)
		.await;
		ok(&issue_note);
		let issue = call(
			&public,
			&plaintext,
			"get_issue",
			json!({ "issue_id": s.issue }),
		)
		.await;
		assert_eq!(ok(&issue)["notes"].as_array().unwrap().len(), 1);

		let nobody = "00000000-0000-0000-0000-000000000000";
		for (tool, field, kind) in [
			("add_issue_note", "issue_id", "issue"),
			("add_incident_note", "incident_id", "incident"),
		] {
			let env = call(
				&public,
				&plaintext,
				tool,
				json!({ field: nobody, "body": "x" }),
			)
			.await;
			assert_eq!(env["result"]["isError"], true, "{tool}: {env}");
			assert_eq!(
				env["result"]["content"][0]["text"],
				format!("no {kind} with id {nobody}")
			);
		}

		for action in [
			"incident_note_added",
			"issue_note_added",
			"note_edited",
			"note_deleted",
		] {
			let entries = logged(&mut conn, action).await;
			assert!(!entries.is_empty(), "{action}");
			assert!(
				entries
					.iter()
					.all(|e| matches!(&e.actor, Actor::Token { id, .. } if *id == token.id)),
				"{action} is the token's"
			);
			let rendered = serde_json::to_string(&entries[0].detail).unwrap();
			assert!(
				!rendered.contains("disk is"),
				"no note text in {action}: {rendered}"
			);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn replacing_notes_states_the_revision_and_is_logged() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let s = seed(&mut conn).await;
		let (_, token) = noting(&mut conn).await;

		let group = call(&public, &token, "get_group", json!({ "group_id": s.group })).await;
		assert_eq!(ok(&group)["notes_revision"], 0);

		let first = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.group, "notes": "VPN needed", "revision": 0 }),
		)
		.await;
		assert_eq!(ok(&first)["revision"], 1);

		let stale = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.group, "notes": "other", "revision": 0 }),
		)
		.await;
		assert!(refused(&stale));
		assert_eq!(
			stale["result"]["structuredContent"]["current"]["notes"],
			"VPN needed"
		);
		assert_eq!(
			stale["result"]["structuredContent"]["current"]["revision"],
			1
		);

		let machine = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.machine, "kind": "machine", "notes": "rack 4", "revision": 0 }),
		)
		.await;
		ok(&machine);

		let history = call(
			&public,
			&token,
			"get_notes_history",
			json!({ "id": s.group }),
		)
		.await;
		let history = ok(&history);
		assert_eq!(history["kind"], "group");
		assert_eq!(history["revisions"][0]["via"], "token");
		assert_eq!(history["revisions"][0]["author"], "scribe");

		let entries = logged(&mut conn, "notes_changed").await;
		assert_eq!(entries.len(), 2);
		let rendered = serde_json::to_string(&entries[0].detail).unwrap();
		assert!(!rendered.contains("VPN") && !rendered.contains("rack"));

		let missing = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": Uuid::new_v4(), "notes": "x", "revision": 0 }),
		)
		.await;
		assert!(refused(&missing));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_id_shared_by_a_machine_and_its_application_needs_a_kind() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let s = seed(&mut conn).await;
		let (_, token) = noting(&mut conn).await;

		let ambiguous = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.machine, "notes": "x", "revision": 0 }),
		)
		.await;
		assert!(refused(&ambiguous));
		assert_eq!(
			ambiguous["result"]["structuredContent"]["current"]["kinds"],
			json!(["machine", "application"])
		);

		let history = call(
			&public,
			&token,
			"get_notes_history",
			json!({ "id": s.machine }),
		)
		.await;
		assert!(refused(&history));

		let app = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.machine, "kind": "application", "notes": "app notes", "revision": 0 }),
		)
		.await;
		assert_eq!(ok(&app)["revision"], 1);

		let machine = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.machine, "kind": "machine", "notes": "box notes", "revision": 0 }),
		)
		.await;
		assert_eq!(ok(&machine)["revision"], 1);

		for (kind, text) in [("application", "app notes"), ("machine", "box notes")] {
			let history = call(
				&public,
				&token,
				"get_notes_history",
				json!({ "id": s.machine, "kind": kind }),
			)
			.await;
			let history = ok(&history);
			assert_eq!(history["kind"], kind);
			assert_eq!(history["revisions"][0]["notes"], text);
		}

		let wrong = call(
			&public,
			&token,
			"replace_notes",
			json!({ "id": s.machine, "kind": "group", "notes": "x", "revision": 0 }),
		)
		.await;
		assert!(refused(&wrong));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn memory_is_recorded_amended_retired_restored_and_read_back() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let s = seed(&mut conn).await;
		let (_, token) = noting(&mut conn).await;

		let recorded = call(
			&public,
			&token,
			"record_memory_entry",
			json!({
				"title": "Pacific disks fill on Mondays",
				"body": "The weekly export is the cause.",
				"link": { "kind": "group", "id": s.group },
			}),
		)
		.await;
		let entry = ok(&recorded).clone();
		assert_eq!(entry["link"]["name"], "Pacific");
		let id = entry["id"].as_str().unwrap().to_owned();

		let found = call(&public, &token, "find_memory", json!({ "query": "mond" })).await;
		assert_eq!(ok(&found)["total"], 1);

		let amended = call(
			&public,
			&token,
			"amend_memory_entry",
			json!({ "id": id, "revision": 1, "clear_link": true, "title": "Disks fill on Mondays" }),
		)
		.await;
		assert_eq!(ok(&amended)["revision"], 2);
		assert!(ok(&amended)["link"].is_null());

		let stale = call(
			&public,
			&token,
			"amend_memory_entry",
			json!({ "id": id, "revision": 1, "title": "again" }),
		)
		.await;
		assert!(refused(&stale));
		assert_eq!(
			stale["result"]["structuredContent"]["current"]["revision"],
			2
		);

		let over = call(
			&public,
			&token,
			"record_memory_entry",
			json!({ "title": "t".repeat(513), "body": "b" }),
		)
		.await;
		assert!(
			over.get("error")
				.is_some_and(|e| e.to_string().contains("512")),
			"{over}"
		);

		let retired = call(
			&public,
			&token,
			"retire_memory_entry",
			json!({ "id": id, "reason": "fixed upstream" }),
		)
		.await;
		assert_eq!(ok(&retired)["revision"], 3);
		let hidden = call(&public, &token, "find_memory", json!({})).await;
		assert_eq!(ok(&hidden)["total"], 0);
		let shown = call(
			&public,
			&token,
			"find_memory",
			json!({ "include_retired": true }),
		)
		.await;
		assert_eq!(ok(&shown)["total"], 1);

		let amend_retired = call(
			&public,
			&token,
			"amend_memory_entry",
			json!({ "id": id, "revision": 3, "title": "x" }),
		)
		.await;
		assert!(amend_retired.get("error").is_some() || refused(&amend_retired));

		let restored = call(
			&public,
			&token,
			"restore_memory_entry",
			json!({ "id": id, "revision": 3, "to_revision": 1 }),
		)
		.await;
		let restored = ok(&restored);
		assert_eq!(restored["revision"], 4);
		assert_eq!(restored["link"]["name"], "Pacific");
		assert!(restored["retired_at"].is_null());

		let detail = call(&public, &token, "get_memory_entry", json!({ "id": id })).await;
		assert_eq!(ok(&detail)["history"].as_array().unwrap().len(), 4);

		let nobody = "00000000-0000-0000-0000-000000000000";
		for (tool, args, message) in [
			(
				"restore_memory_entry",
				json!({ "id": id, "revision": 4, "to_revision": 99 }),
				"the entry has no revision 99".to_owned(),
			),
			(
				"retire_memory_entry",
				json!({ "id": nobody, "reason": "x" }),
				format!("no memory entry with id {nobody}"),
			),
			(
				"record_memory_entry",
				json!({ "title": "x", "body": "x", "link": { "kind": "group", "id": nobody } }),
				format!("no group with id {nobody}"),
			),
		] {
			let env = call(&public, &token, tool, args).await;
			assert_eq!(env["result"]["isError"], true, "{tool}: {env}");
			assert_eq!(env["result"]["content"][0]["text"], message, "{tool}");
		}

		for action in [
			"memory_entry_recorded",
			"memory_entry_amended",
			"memory_entry_retired",
			"memory_entry_restored",
		] {
			let entries = logged(&mut conn, action).await;
			assert_eq!(entries.len(), 1, "{action}");
			let rendered = serde_json::to_string(&entries[0].detail).unwrap();
			assert!(!rendered.contains("weekly export"), "{action}: {rendered}");
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_index_counts_incidents_and_shows_the_gaps() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let s = seed(&mut conn).await;
		let (_, token) = noting(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO check_policies (source, check_name, ceiling, escalates) VALUES ('alertd', 'disk-space', 'failed', true); \
			 INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ('{}', '{}', NOW()); \
			 INSERT INTO incident_notes (incident_id, author, body) VALUES ('{}', 'op', 'n');",
			s.incident, s.issue, s.incident
		))
		.await
		.unwrap();

		let index = call(&public, &token, "get_memory_index", json!({})).await;
		let index = ok(&index);
		assert_eq!(index["checks"][0]["name"], "disk-space");
		assert_eq!(index["checks"][0]["incidents"], 1);
		assert_eq!(index["checks"][0]["linked_entries"], 0);
		assert_eq!(index["groups"][0]["name"], "Pacific");
		assert_eq!(index["incidents_with_notes"][0]["notes"], 1);

		let entry = call(
			&public,
			&token,
			"record_memory_entry",
			json!({
				"title": "Disk space",
				"body": "b",
				"link": { "kind": "check", "source": "alertd", "name": "disk-space" },
			}),
		)
		.await;
		ok(&entry);
		let index = call(&public, &token, "get_memory_index", json!({})).await;
		assert_eq!(ok(&index)["checks"][0]["linked_entries"], 1);
	})
	.await
}
