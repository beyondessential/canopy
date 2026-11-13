//! Alert triage through the fleet query interface: triage_alert reads an
//! incident's context and serves the guidance, recording is held by the notes
//! grant and refused against a changed incident, and every write is audited.

use axum_test::TestServer;
use database::{
	audit::{self, Filter},
	mcp_tokens::McpToken,
	notes::IncidentNote,
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

/// A production incident with one documented failing check, its application
/// sharing its machine's id.
async fn seed(conn: &mut AsyncPgConnection) -> (Uuid, Uuid) {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let incident = Uuid::new_v4();
	let issue = Uuid::new_v4();
	let device = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{device}', 'machine'); \
		 INSERT INTO server_groups (id, name) VALUES ('{group}', 'Pacific'); \
		 INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{machine}', '{group}', '{device}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
			('{machine}', 'https://example.com', 'tamanu-central', '{group}', 'production', '{machine}'); \
		 INSERT INTO check_policies (source, check_name, ceiling, escalates, documentation) \
			VALUES ('alertd', 'disk-space', 'failed', false, 'The disk is filling'); \
		 INSERT INTO issues \
			(id, application_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen, degraded_since) \
		   VALUES ('{issue}', '{machine}', 'alertd', 'r', 'disk-space', 'failed', 'failed', 'disk at 97%', true, NOW(), NOW(), NOW()); \
		 INSERT INTO incidents (id, server_group_id, rank, opened_at) VALUES ('{incident}', '{group}', 'production', NOW()); \
		 INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ('{incident}', '{issue}', NOW());"
	))
	.await
	.expect("seed");
	(incident, machine)
}

#[tokio::test(flavor = "multi_thread")]
async fn triage_reads_the_required_escalation_and_records_a_verdict() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let (incident, machine) = seed(&mut conn).await;
		let (_, reader) = McpToken::mint(&mut conn, "reader", "test@example.com", &[])
			.await
			.unwrap();
		let (_, scribe) = McpToken::mint(
			&mut conn,
			"triage",
			"test@example.com",
			&["notes".to_owned()],
		)
		.await
		.unwrap();

		let triage = call(
			&public,
			&reader,
			"triage_alert",
			json!({ "incident_id": incident }),
		)
		.await;
		let triage = ok(&triage).clone();
		assert_eq!(triage["incident"]["target"], json!("Pacific"));
		assert_eq!(triage["changes"], json!(null));
		assert_eq!(
			triage["checks"][0]["documentation"],
			json!("The disk is filling")
		);
		assert_eq!(
			triage["live_issues"][0]["scope"]["grain"],
			json!("application")
		);
		assert_eq!(triage["live_issues"][0]["scope"]["id"], json!(machine));
		assert!(
			triage["guidance"]
				.as_str()
				.unwrap()
				.contains("record_alert_triage")
		);
		let revision = triage["revision"].as_str().unwrap().to_owned();

		let record = |verdict: &str, revision: &str| {
			json!({
				"incident_id": incident,
				"revision": revision,
				"verdict": verdict,
				"reasoning": "Production disk filling, no known pattern.",
			})
		};

		// Reading needs nothing; recording needs the notes grant.
		assert!(refused(
			&call(
				&public,
				&reader,
				"record_alert_triage",
				record("escalated", &revision)
			)
			.await
		));

		// A revision from before the incident changed is refused too.
		let stale = revision.replacen(|c: char| c.is_ascii_hexdigit(), "z", 1);
		assert!(refused(
			&call(
				&public,
				&scribe,
				"record_alert_triage",
				record("escalated", &stale)
			)
			.await
		));
		assert!(
			IncidentNote::list_for_incident(&mut conn, incident, 10, false)
				.await
				.unwrap()
				.is_empty()
		);

		let recorded = call(
			&public,
			&scribe,
			"record_alert_triage",
			record("escalated", &revision),
		)
		.await;
		let recorded = ok(&recorded).clone();
		assert_eq!(recorded["verdict"]["verdict"], json!("escalated"));
		let verdict_id = recorded["verdict"]["id"].as_str().unwrap().to_owned();
		let notes = IncidentNote::list_for_incident(&mut conn, incident, 10, false)
			.await
			.unwrap();
		assert_eq!(notes.len(), 1);
		assert_eq!(notes[0].author, "triage");
		assert_eq!(logged(&mut conn, "alert_triage_recorded").await.len(), 1);
		assert_eq!(logged(&mut conn, "incident_note_added").await.len(), 1);

		let attached = call(
			&public,
			&scribe,
			"attach_alert_triage_report",
			json!({ "verdict_id": verdict_id, "link": "https://slack.example/archives/C1/p1" }),
		)
		.await;
		assert_eq!(
			ok(&attached)["report_link"],
			json!("https://slack.example/archives/C1/p1")
		);
		assert_eq!(logged(&mut conn, "alert_triage_reported").await.len(), 1);

		// The same alert again finds the verdict already given.
		let again = call(
			&public,
			&reader,
			"triage_alert",
			json!({ "incident_id": incident }),
		)
		.await;
		let again = ok(&again);
		assert_eq!(
			again["already_triaged"]["report_link"],
			json!("https://slack.example/archives/C1/p1")
		);

		let day = call(&public, &reader, "find_alert_triage_verdicts", json!({})).await;
		let day = ok(&day);
		assert_eq!(day["escalated_count"], json!(1));
		assert_eq!(day["verdicts"][0]["target"], json!("Pacific"));
	})
	.await;
}
