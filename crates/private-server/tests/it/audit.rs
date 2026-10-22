//! What the operator surface records in the audit log, and how it reads back
//! (spec `AUD`).

use commons_tests::diesel_async::SimpleAsyncConnection;
use database::audit::{self, Actor, Entry, Filter, Status};
use serde_json::{Value, json};

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

async fn log(conn: &mut database::diesel_async::AsyncPgConnection, filter: Filter) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			limit: 100,
			..filter
		},
	)
	.await
	.unwrap()
}

fn in_an_hour() -> String {
	(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1)).to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn declaring_amending_and_lifting_maintenance_are_recorded_with_what_they_acted_on() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		let window: Value = private
			.post("/api/maintenance/declare")
			.json(&json!({
				"server_group_id": GROUP,
				"expected_end": in_an_hour(),
				"note": "patching",
			}))
			.await
			.json();
		// Declaring over a target that has a window amends it.
		private
			.post("/api/maintenance/declare")
			.json(&json!({
				"server_group_id": GROUP,
				"expected_end": in_an_hour(),
				"note": "patching, still",
			}))
			.await
			.assert_status_ok();
		private
			.post("/api/maintenance/lift")
			.json(&json!({ "id": window["id"] }))
			.await
			.assert_status_ok();

		let entries = log(&mut conn, Filter::default()).await;
		let actions: Vec<&str> = entries.iter().rev().map(|e| e.action.as_str()).collect();
		assert_eq!(
			actions,
			[
				"maintenance_declared",
				"maintenance_amended",
				"maintenance_lifted"
			]
		);

		let declared = entries.last().unwrap();
		assert!(matches!(
			&declared.actor,
			Actor::Operator { login, .. } if login == "admin@localhost"
		));
		assert_eq!(declared.detail["note"], "patching");
		assert!(
			declared.records.iter().any(|r| r.name == "kamaka"),
			"the target is named: {:?}",
			declared.records
		);
		assert!(
			declared
				.records
				.iter()
				.any(|r| r.id == window["id"].as_str().unwrap())
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entry_keeps_the_name_a_record_had_and_never_blocks_deleting_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		private
			.post("/api/maintenance/declare")
			.json(&json!({ "machine_id": BOX, "expected_end": in_an_hour() }))
			.await
			.assert_status_ok();

		conn.batch_execute(&format!(
			"UPDATE machines SET name = 'renamed' WHERE id = '{BOX}'"
		))
		.await
		.unwrap();
		let entries = log(&mut conn, Filter::default()).await;
		assert!(entries[0].records.iter().any(|r| r.name == "box"));

		// Nothing references the entry's records, so removing them is not refused.
		conn.batch_execute(&format!(
			"DELETE FROM maintenance_windows; DELETE FROM applications; DELETE FROM machines WHERE id = '{BOX}'"
		))
		.await
		.unwrap();
		let entries = log(&mut conn, Filter { acted_on: Some(BOX.into()), ..Default::default() }).await;
		assert_eq!(entries.len(), 1, "found by the record it acted on");
		assert!(entries[0].records.iter().any(|r| r.name == "box"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn taking_over_and_serving_an_environment_are_recorded_and_the_read_is_routine() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		let lease: Value = private
			.post("/api/ansible/take_lease")
			.json(&json!({ "server_group_id": GROUP, "intent": "configure", "note": "run" }))
			.await
			.json();
		// A variable of the environment's own, whose value never reaches the log.
		private
			.post("/api/ansible_variables/set")
			.json(&json!({
				"server_group_id": GROUP,
				"name": "ntp_server",
				"value": "ntp.example.org",
			}))
			.await
			.assert_status_ok();
		private
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": lease["id"] }))
			.await
			.assert_status_ok();
		private
			.post("/api/ansible/release_lease")
			.json(&json!({ "lease_id": lease["id"] }))
			.await
			.assert_status_ok();

		let important = log(&mut conn, Filter::default()).await;
		let actions: Vec<&str> = important.iter().rev().map(|e| e.action.as_str()).collect();
		// The two a run's start records share a moment, so their order is not
		// one to rely on.
		let mut starting = actions[2..4].to_vec();
		starting.sort_unstable();
		assert_eq!(
			(&actions[..2], starting.as_slice(), &actions[4..]),
			(
				&["inventory_lease_taken", "inventory_variable_set"][..],
				&["ansible_run_started", "maintenance_declared"][..],
				&["inventory_lease_released"][..],
			),
			"a read is routine and left out by default, and the run starting is not"
		);
		let variable = important
			.iter()
			.find(|e| e.action == "inventory_variable_set")
			.unwrap();
		assert!(!variable.detail.to_string().contains("ntp.example.org"));

		let everything = log(
			&mut conn,
			Filter {
				include_routine: true,
				..Default::default()
			},
		)
		.await;
		let read = everything
			.iter()
			.find(|e| e.action == "inventory_read")
			.expect("the read is recorded");
		assert_eq!(read.status, Status::Done);
		assert_eq!(read.detail["intent"], "configure");

		let named = log(
			&mut conn,
			Filter {
				action: Some("inventory_read".into()),
				..Default::default()
			},
		)
		.await;
		assert_eq!(named.len(), 1, "naming a routine action shows it");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_variable_is_recorded_ahead_of_the_store_and_settled_without_its_value() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		private
			.post("/api/ansible_variables/set")
			.json(&json!({
				"server_group_id": GROUP,
				"name": "db_password",
				"value": "hunter2",
				"secret": true,
			}))
			.await
			.assert_status_ok();

		let entries = log(&mut conn, Filter::default()).await;
		assert_eq!(entries.len(), 1);
		assert_eq!(entries[0].action, "inventory_variable_set");
		assert_eq!(entries[0].status, Status::Done);
		assert_eq!(entries[0].detail["secret"], true);
		assert!(
			!serde_json::to_string(&entries[0])
				.unwrap()
				.contains("hunter2")
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn issuing_a_device_credential_records_its_identifier_and_never_the_key() {
	commons_tests::server::run(async |mut conn, _, private| {
		let issued: Value = private
			.post("/api/devices/provision_credential")
			.json(&json!({ "role": "machine" }))
			.await
			.json();

		let entries = log(&mut conn, Filter::default()).await;
		assert_eq!(entries.len(), 1);
		assert_eq!(entries[0].action, "device_credential_provisioned");
		assert!(
			entries[0]
				.records
				.iter()
				.any(|r| r.id == issued["key_id"].as_str().unwrap())
		);
		assert!(
			entries[0]
				.records
				.iter()
				.any(|r| r.id == issued["device_id"].as_str().unwrap())
		);
		let everything = serde_json::to_string(&entries[0]).unwrap();
		for secret in ["passphrase", "key_age_base64"] {
			assert!(!everything.contains(issued[secret].as_str().unwrap()));
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_log_page_lists_newest_first_and_filters_and_the_tool_reads_the_same() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		for note in ["first", "second"] {
			private
				.post("/api/maintenance/declare")
				.json(&json!({ "machine_id": BOX, "expected_end": in_an_hour(), "note": note }))
				.await
				.assert_status_ok();
		}
		private
			.post("/api/maintenance/lift")
			.json(&json!({
				"id": private
					.post("/api/maintenance/list_open")
					.json(&json!({}))
					.await
					.json::<Vec<Value>>()[0]["window"]["id"]
			}))
			.await
			.assert_status_ok();

		let list = async |args: Value| -> Vec<Value> {
			private.post("/api/audit/list").json(&args).await.json()
		};
		let all = list(json!({})).await;
		assert_eq!(all.len(), 3);
		assert_eq!(all[0]["action"], "maintenance_lifted", "newest first");
		assert_eq!(all[0]["action_label"], "Lift maintenance");

		assert_eq!(
			list(json!({ "action": "maintenance_amended" })).await.len(),
			1
		);
		assert_eq!(
			list(json!({ "actor_id": "admin@localhost" })).await.len(),
			3
		);
		assert_eq!(list(json!({ "actor_id": "someone@else" })).await.len(), 0);
		assert_eq!(list(json!({ "record": BOX })).await.len(), 3);
		assert_eq!(
			list(json!({ "record": "box" })).await.len(),
			3,
			"by name too"
		);
		assert_eq!(list(json!({ "record": "nothing like it" })).await.len(), 0);

		let filters: Value = private
			.post("/api/audit/filters")
			.json(&json!({}))
			.await
			.json();
		assert!(
			filters["actions"]
				.as_array()
				.unwrap()
				.iter()
				.any(|a| a["name"] == "maintenance_lifted" && a["label"] == "Lift maintenance")
		);
		assert_eq!(filters["actors"][0]["login"], "admin@localhost");
	})
	.await
}

/// A week long gone, laid out as the provisioning lays out the live ones, so an
/// entry old enough to be outside the ninety days can be written.
async fn old_entries(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(
		"CREATE TABLE audit_entries_old PARTITION OF audit_entries \
		   FOR VALUES FROM (now() - interval '120 days') TO (now() - interval '100 days') \
		   PARTITION BY LIST (retention); \
		 CREATE TABLE audit_entries_old_standard PARTITION OF audit_entries_old FOR VALUES IN ('standard'); \
		 CREATE TABLE audit_entries_old_year PARTITION OF audit_entries_old FOR VALUES IN ('year'); \
		 INSERT INTO audit_entries (recorded_at, retention, actor_kind, actor_id, action) VALUES \
		   (now() - interval '110 days', 'year', 'operator', 'old@example.com', 'upgrade_plan_withdrawn');",
	)
	.await
	.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn nothing_beyond_ninety_days_is_read_from_the_database_by_the_page_or_the_tool() {
	commons_tests::server::run(async |mut conn, _, private| {
		old_entries(&mut conn).await;

		let entries: Vec<Value> = private
			.post("/api/audit/list")
			.json(&json!({ "since": "2000-01-01T00:00:00Z" }))
			.await
			.json();
		assert!(
			entries.is_empty(),
			"a year-kept entry past ninety days is not read: {entries:?}"
		);

		let found = private
			.post("/api/mcp")
			.add_header("accept", "application/json, text/event-stream")
			.add_header("mcp-protocol-version", "2025-06-18")
			.json(&json!({
				"jsonrpc": "2.0", "id": 2, "method": "tools/call",
				"params": { "name": "find_audit_entries", "arguments": { "window_days": 90 } }
			}))
			.await
			.text();
		assert!(found.contains("\"entries\""), "{found}");
		assert!(!found.contains("old@example.com"), "{found}");

		let refused = private
			.post("/api/mcp")
			.add_header("accept", "application/json, text/event-stream")
			.add_header("mcp-protocol-version", "2025-06-18")
			.json(&json!({
				"jsonrpc": "2.0", "id": 2, "method": "tools/call",
				"params": { "name": "find_audit_entries", "arguments": { "window_days": 91 } }
			}))
			.await
			.text();
		assert!(
			refused.contains("at most 90"),
			"a longer window is refused, naming the limit: {refused}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn only_an_administrator_reads_the_log() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var("CANOPY_TRUST_TAILSCALE_HEADERS", "1") };

	commons_tests::server::run(async |mut conn, _, private| {
		database::admins::Admin::add(&mut conn, "boss@example.com")
			.await
			.unwrap();

		private
			.post("/api/audit/list")
			.add_header("Tailscale-User-Login", "stranger@example.com")
			.add_header("Tailscale-User-Name", "Stranger")
			.json(&json!({}))
			.await
			.assert_status_forbidden();
		private
			.post("/api/audit/list")
			.add_header("Tailscale-User-Login", "boss@example.com")
			.add_header("Tailscale-User-Name", "Boss")
			.json(&json!({}))
			.await
			.assert_status_ok();
	})
	.await
}

/// Every operation on the operator surface and the device API carries an audit
/// declaration, and every action one names is in the catalogue.
#[test]
fn every_route_declares_whether_it_is_audited_and_names_real_actions() {
	use canopy_utoipa_axum::router::OpenApiRouter;
	use commons_servers::audit::AuditMap;
	use utoipa::OpenApi;

	let (_, private) = OpenApiRouter::with_openapi(private_server::openapi::ApiDoc::openapi())
		.merge(private_server::fns::routes())
		.split_for_parts();
	let (_, public) = OpenApiRouter::with_openapi(public_server::openapi::ApiDoc::openapi())
		.merge(public_server::routes())
		.split_for_parts();

	for (surface, spec) in [("operator", &private), ("device", &public)] {
		let operations: usize = spec
			.paths
			.paths
			.values()
			.map(|item| {
				[&item.get, &item.put, &item.post, &item.delete, &item.patch]
					.iter()
					.filter(|operation| operation.is_some())
					.count()
			})
			.sum();
		let map = AuditMap::from_openapi(spec);
		assert_eq!(
			map.len(),
			operations,
			"every {surface} operation declares whether it is audited"
		);
		assert!(map.audited() > 0, "the {surface} surface audits something");
		for name in map.action_names() {
			assert!(
				database::audit::CATALOGUE.contains(&name),
				"{surface} names {name}, which is not an action"
			);
		}
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn the_tool_leaves_routine_actions_out_unless_asked_for() {
	use database::audit::{Action, Actor, Record, RecordKind};

	commons_tests::server::run(async |mut conn, _, private| {
		audit::record(
			&mut conn,
			&Actor::canopy("test"),
			&Action::BackupCredentialsIssued {
				group: Record::new(RecordKind::Group, uuid::Uuid::new_v4(), "routine-group"),
				machine: Record::new(RecordKind::Machine, uuid::Uuid::new_v4(), "box"),
				backup_type: "tamanu-postgres".into(),
				purpose: "backup".into(),
				run_id: None,
			},
		)
		.await
		.unwrap();

		let call = async |arguments: Value| -> String {
			private
				.post("/api/mcp")
				.add_header("accept", "application/json, text/event-stream")
				.add_header("mcp-protocol-version", "2025-06-18")
				.json(&json!({
					"jsonrpc": "2.0", "id": 2, "method": "tools/call",
					"params": { "name": "find_audit_entries", "arguments": arguments }
				}))
				.await
				.text()
		};
		assert!(!call(json!({})).await.contains("routine-group"));
		assert!(
			call(json!({ "include_routine": true }))
				.await
				.contains("routine-group")
		);
		assert!(
			call(json!({ "action": "backup_credentials_issued" }))
				.await
				.contains("routine-group"),
			"naming the action shows it"
		);
	})
	.await
}
