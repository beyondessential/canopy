//! Endpoint tests for the operator-facing `/api/restore_replicas/*` fns —
//! parameter validation against the consumer's advertised schema, and that
//! declared parameter values round-trip back through the group view.

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use commons_types::backup::IntentDescriptor;
use database::RestoreConsumerCapability;
use uuid::Uuid;

async fn insert_group(conn: &mut AsyncPgConnection) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{id}', 'rr-test')"
	))
	.await
	.expect("insert group");
	id
}

async fn insert_consumer(conn: &mut AsyncPgConnection) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{id}', 'backup-restore')"
	))
	.await
	.expect("insert device");
	id
}

async fn insert_server(conn: &mut AsyncPgConnection, group_id: Uuid) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"WITH m AS (INSERT INTO machines (id, group_id) VALUES ('{id}', '{group_id}') RETURNING id) INSERT INTO applications (id, name, host, type, rank, group_id, machine_id) VALUES
			('{id}', 'rr-test-server', 'https://{id}.example.com', 'tamanu-facility',
			 'production', '{group_id}', '{id}')"
	))
	.await
	.expect("insert server");
	id
}

/// Advertise an `analytics` intent accepting a boolean `anonymisation` param.
async fn advertise_analytics(conn: &mut AsyncPgConnection, consumer: Uuid) {
	let descriptor: IntentDescriptor = serde_json::from_value(serde_json::json!({
		"intent": "analytics",
		"semantics": ["check", "url"],
		"params": { "anonymisation": {"type": "boolean", "default": true} },
	}))
	.unwrap();
	RestoreConsumerCapability::register(conn, consumer, &[descriptor])
		.await
		.expect("register caps");
}

/// Advertise a `verify` intent with no parameters.
async fn advertise_verify(conn: &mut AsyncPgConnection, consumer: Uuid) {
	let descriptor: IntentDescriptor = serde_json::from_value(serde_json::json!({
		"intent": "verify",
		"semantics": ["check"],
		"params": {},
	}))
	.unwrap();
	RestoreConsumerCapability::register(conn, consumer, &[descriptor])
		.await
		.expect("register caps");
}

/// Advertise a `sizing` intent accepting a duration `minimum_uptime` param
/// (with a default) and a bytes `max_size` param.
async fn advertise_sizing(conn: &mut AsyncPgConnection, consumer: Uuid) {
	let descriptor: IntentDescriptor = serde_json::from_value(serde_json::json!({
		"intent": "sizing",
		"semantics": ["check"],
		"params": {
			"minimum_uptime": {"type": "duration", "default": 7200},
			"max_size": {"type": "bytes"},
		},
	}))
	.unwrap();
	RestoreConsumerCapability::register(conn, consumer, &[descriptor])
		.await
		.expect("register caps");
}

/// Advertise both `verify` (no params) and `analytics` (boolean `anonymisation`)
/// on the same consumer, so a declaration can be retargeted between them.
async fn advertise_verify_and_analytics(conn: &mut AsyncPgConnection, consumer: Uuid) {
	let descriptors: Vec<IntentDescriptor> = serde_json::from_value(serde_json::json!([
		{ "intent": "verify", "semantics": ["check"], "params": {} },
		{
			"intent": "analytics",
			"semantics": ["check", "url"],
			"params": { "anonymisation": {"type": "boolean", "default": true} },
		},
	]))
	.unwrap();
	RestoreConsumerCapability::register(conn, consumer, &descriptors)
		.await
		.expect("register caps");
}

#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_wrong_typed_param() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_analytics(&mut conn, consumer).await;

		// `anonymisation` is a boolean; a string value is rejected as 400.
		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "an-decl",
				"overdue_after": null,
				"params": { "anonymisation": "yes" },
			}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_stores_valid_params_and_they_round_trip() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_analytics(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "an-decl",
				"overdue_after": "2h",
				"params": { "anonymisation": false },
			}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert_eq!(rows[0]["overdue_after"], "2h");
		assert_eq!(rows[0]["params"]["anonymisation"], false);
		assert_eq!(rows[0]["gap"], false, "advertised intent is not a gap");

		// The bound is stored as a raw interval, not the display string.
		let stored = database::RestoreReplica::list_for_group(&mut conn, group)
			.await
			.expect("list replicas");
		assert_eq!(stored.len(), 1);
		assert_eq!(stored[0].overdue_after.as_ref().unwrap().0.as_secs(), 7200);
	})
	.await;
}

/// Advertise an `analytics` intent that can redact, carrying the three
/// masking parameters Canopy takes over.
async fn advertise_redacting_analytics(conn: &mut AsyncPgConnection, consumer: Uuid) {
	let descriptor: IntentDescriptor = serde_json::from_value(serde_json::json!({
		"intent": "analytics",
		"semantics": ["check", "url", "redact"],
		"params": {
			"anonymisation": {"type": "boolean", "default": true},
			"redaction_manifest_url": {"type": "text"},
			"redaction_version_query": {"type": "text"},
			"redaction_version_fallback_to_base": {"type": "boolean", "default": false},
		},
	}))
	.unwrap();
	RestoreConsumerCapability::register(conn, consumer, &[descriptor])
		.await
		.expect("register caps");
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redacting_declaration_round_trips_its_flag() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_redacting_analytics(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "redacted-decl",
				"redacts": true,
			}))
			.await
			.assert_status_ok();

		let rows: Vec<serde_json::Value> = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await
			.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert_eq!(rows[0]["redacts"], true);
		assert_eq!(rows[0]["can_redact"], true);
	})
	.await;
}

/// The manifest parameters belong to Canopy for any intent that can redact,
/// in both states: were an operator value kept, a declaration could redact
/// with its flag off and the flag would stop answering on its own whether an
/// unmasked replica is a finding.
#[tokio::test(flavor = "multi_thread")]
async fn an_operator_cannot_set_the_masking_parameters() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_redacting_analytics(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "sneaky-decl",
				"params": {
					"anonymisation": false,
					"redaction_manifest_url": "https://evil.example/manifest.json",
				},
			}))
			.await
			.assert_status_ok();

		let stored = database::RestoreReplica::list_for_group(&mut conn, group)
			.await
			.expect("list replicas");
		assert_eq!(
			stored[0].params["anonymisation"], false,
			"other params keep"
		);
		assert!(
			stored[0].params.get("redaction_manifest_url").is_none(),
			"canopy owns the manifest URL, so an operator value is dropped"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_intent_that_cannot_redact_refuses_the_flag() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "cant-redact",
				"redacts": true,
			}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

/// The publisher mark is unique per group, and a machine-scoped or redacting
/// declaration is one no build is dispatched to. Accepting the mark on one
/// takes the group's only slot and leaves the operator unable to declare the
/// publisher that would work.
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_no_build_is_dispatched_to_cannot_publish() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		let server = insert_server(&mut conn, group).await;

		// The intent is one the consumer has not advertised, so the descriptor
		// this would otherwise be checked against does not exist yet.
		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": server,
				"type": "tamanu-postgres",
				"intent": "reporting-schema",
				"name": "one-box-publisher",
				"publishes_schemas": true,
			}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

/// A server whose product publishes no manifest is withheld from the
/// worklist, so the operator is shown which of the declaration's replicas
/// aren't being restored and why.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_that_cannot_be_redacted_shows_as_a_gap() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_redacting_analytics(&mut conn, consumer).await;
		let senaite = Uuid::new_v4();
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (id, group_id) VALUES ('{senaite}', '{group}') RETURNING id) INSERT INTO applications (id, name, host, type, rank, group_id, machine_id) VALUES
				('{senaite}', 'lims', 'https://{senaite}.example.com', 'senaite', 'production', '{group}', '{senaite}')"
		))
		.await
		.expect("insert senaite server");

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "group-wide-redacted",
				"redacts": true,
			}))
			.await
			.assert_status_ok();

		let rows: Vec<serde_json::Value> = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await
			.json();
		let gaps = rows[0]["redaction_gaps"].as_array().expect("gaps");
		assert_eq!(gaps.len(), 1, "got {gaps:?}");
		assert_eq!(gaps[0]["server_id"], senaite.to_string());
		assert_eq!(gaps[0]["reason"], "product_has_no_manifest");
	})
	.await;
}

/// Declare a replica via the HTTP endpoint and return its id.
async fn create_replica(
	private: &commons_tests::axum_test::TestServer,
	consumer: Uuid,
	group: Uuid,
	intent: &str,
	params: serde_json::Value,
) -> Uuid {
	let resp = private
		.post("/api/restore_replicas/create")
		.json(&serde_json::json!({
			"consumer_device_id": consumer,
			"group_id": group,
			"machine_id": null,
			"type": "tamanu-postgres",
			"intent": intent,
			"name": format!("{intent}-decl"),
			"overdue_after": null,
			"params": params,
		}))
		.await;
	resp.assert_status_ok();
	let body: serde_json::Value = resp.json();
	body["id"].as_str().unwrap().parse().unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn update_changes_intent_and_round_trips() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify_and_analytics(&mut conn, consumer).await;

		let id = create_replica(&private, consumer, group, "verify", serde_json::json!({})).await;

		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": id,
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "verify-decl",
				"overdue_after": null,
				"params": { "anonymisation": true },
				"enabled": true,
			}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert_eq!(rows[0]["intent"], "analytics");
		assert_eq!(rows[0]["params"]["anonymisation"], true);
		assert_eq!(rows[0]["gap"], false);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_name_collides_not_the_scope() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify_and_analytics(&mut conn, consumer).await;

		create_replica(&private, consumer, group, "verify", serde_json::json!({})).await;
		let b = create_replica(
			&private,
			consumer,
			group,
			"analytics",
			serde_json::json!({}),
		)
		.await;

		// Retargeting b's intent onto a's (consumer, group, type, intent, server)
		// scope is allowed: the two keep their own names.
		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": b,
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "analytics-decl",
				"overdue_after": null,
				"params": {},
				"enabled": true,
			}))
			.await
			.assert_status_ok();

		// Taking the other declaration's name is what's refused.
		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": b,
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "verify-decl",
				"overdue_after": null,
				"params": {},
				"enabled": true,
			}))
			.await
			.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn update_revalidates_params_against_new_intent() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify_and_analytics(&mut conn, consumer).await;

		let id = create_replica(&private, consumer, group, "verify", serde_json::json!({})).await;

		// `anonymisation` is a boolean; a string value fails validation against
		// analytics' schema.
		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": id,
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "verify-decl",
				"overdue_after": null,
				"params": { "anonymisation": "yes" },
				"enabled": true,
			}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn update_can_change_consumer_and_server_scope() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let server = insert_server(&mut conn, group).await;
		let consumer_a = insert_consumer(&mut conn).await;
		let consumer_b = insert_consumer(&mut conn).await;
		advertise_verify(&mut conn, consumer_a).await;
		advertise_verify(&mut conn, consumer_b).await;

		let id = create_replica(&private, consumer_a, group, "verify", serde_json::json!({})).await;

		// Reassign the declaration to a different consumer and narrow it from
		// "every server in the group" to one specific server.
		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": id,
				"consumer_device_id": consumer_b,
				"group_id": group,
				"machine_id": server,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "verify-decl",
				"overdue_after": null,
				"params": {},
				"enabled": true,
			}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert_eq!(rows[0]["consumer_device_id"], consumer_b.to_string());
		assert_eq!(rows[0]["machine_id"], server.to_string());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn update_to_unadvertised_intent_creates_gap() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		// This consumer only advertises `verify`.
		advertise_verify(&mut conn, consumer).await;

		let id = create_replica(&private, consumer, group, "verify", serde_json::json!({})).await;

		// Retargeting to `analytics`, which the consumer doesn't advertise, is
		// accepted with the params passing through unvalidated — same as create,
		// which allows declaring ahead of the consumer registering support. The
		// declaration surfaces as a gap.
		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": id,
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "verify-decl",
				"overdue_after": null,
				"params": { "not_in_any_schema": "kept-as-is" },
				"enabled": true,
			}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert_eq!(rows[0]["intent"], "analytics");
		assert_eq!(rows[0]["gap"], true, "unadvertised intent is a gap");
		assert_eq!(rows[0]["params"]["not_in_any_schema"], "kept-as-is");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn checks_reports_duration_and_surfaces_unreported_restores() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		let member_device = Uuid::new_v4();
		let server = Uuid::new_v4();
		let reported_run = Uuid::new_v4();
		let inflight_run = Uuid::new_v4();
		let member_run = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{member_device}', 'machine');
			 WITH m AS (INSERT INTO machines (id, group_id, device_id) VALUES ('{server}', '{group}', '{member_device}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES
				('{server}', 'https://s.test', 'tamanu-central', '{group}', '{server}');
			 -- A reported check plus the issuance that started it 5 minutes before
			 -- the report → the row carries a ~300s duration.
			 INSERT INTO backup_restore_checks
				(consumer_device_id, group_id, machine_id, type, intent, outcome, replica_healthy, observed_at, reported_at, run_id)
				VALUES ('{consumer}', '{group}', '{server}', 'tamanu-postgres', 'verify', 'success', true, now(), now(), '{reported_run}');
			 INSERT INTO backup_credential_issuances
				(device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix, run_id)
				VALUES ('{consumer}', '{group}', 'tamanu-postgres', now() - interval '300 seconds', now() + interval '3300 seconds', 'restore', 'arn:test', 'b', '', '{reported_run}');
			 -- A consumer restore whose creds are still valid but which never
			 -- reported → surfaces as in progress.
			 INSERT INTO backup_credential_issuances
				(device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix, run_id)
				VALUES ('{consumer}', '{group}', 'tamanu-postgres', now() - interval '30 seconds', now() + interval '3570 seconds', 'restore', 'arn:test', 'b', '', '{inflight_run}');
			 -- A member-server restore issuance (manual restore) belongs to the
			 -- backup panel, not this table.
			 INSERT INTO backup_credential_issuances
				(device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix, run_id)
				VALUES ('{member_device}', '{group}', 'tamanu-postgres', now() - interval '30 seconds', now() + interval '3570 seconds', 'restore', 'arn:test', 'b', '', '{member_run}');"
		))
		.await
		.expect("seed restore activity");

		let resp = private
			.post("/api/restore_replicas/checks")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let rows = body.as_array().unwrap();
		// Reported check + in-flight consumer restore; the member-device issuance is
		// excluded (it belongs to the backup panel).
		assert_eq!(rows.len(), 2, "got {rows:?}");

		let reported = rows.iter().find(|r| r["status"] == "reported").unwrap();
		let dur = reported["duration_seconds"].as_i64().expect("duration");
		assert!((250..=350).contains(&dur), "≈300s from issuance→report, got {dur}");
		assert_eq!(reported["intent"], "verify");

		let inflight = rows.iter().find(|r| r["status"] == "in_progress").unwrap();
		assert!(inflight["machine_id"].is_null(), "inferred row has no server");
		assert!(inflight["duration_seconds"].is_null());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_resolves_unit_strings_and_stores_raw_values() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_sizing(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "sizing",
				"name": "sizing-decl",
				"overdue_after": "1d 12h",
				"params": { "minimum_uptime": "2h 30m", "max_size": "20G" },
			}))
			.await
			.assert_status_ok();

		// Storage is raw: whole seconds and bytes, "20G" read as 1024-based.
		let stored = database::RestoreReplica::list_for_group(&mut conn, group)
			.await
			.expect("list replicas");
		assert_eq!(stored.len(), 1);
		assert_eq!(
			stored[0].overdue_after.as_ref().unwrap().0.as_secs(),
			129600
		);
		assert_eq!(stored[0].params["minimum_uptime"], serde_json::json!(9000));
		assert_eq!(
			stored[0].params["max_size"],
			serde_json::json!(20i64 * 1024 * 1024 * 1024)
		);

		// The view formats everything back as display strings.
		let resp = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert_eq!(rows[0]["overdue_after"], "1d 12h");
		assert_eq!(rows[0]["params"]["minimum_uptime"], "2h 30m");
		assert_eq!(rows[0]["params"]["max_size"], "20Gi");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_accepts_raw_integers_for_unit_params() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_sizing(&mut conn, consumer).await;

		// Raw integer seconds/bytes (as the view once returned, and as gap
		// declarations still carry) are accepted unchanged.
		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "sizing",
				"name": "raw-decl",
				"overdue_after": null,
				"params": { "minimum_uptime": 5400, "max_size": 1536 },
			}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/restore_replicas/for_group")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		assert!(rows[0]["overdue_after"].is_null());
		assert_eq!(rows[0]["params"]["minimum_uptime"], "1h 30m");
		// 1536 bytes isn't an exact multiple of 1024, so it displays raw.
		assert_eq!(rows[0]["params"]["max_size"], "1536");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_bad_unit_strings() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_sizing(&mut conn, consumer).await;

		for (name, params) in [
			(
				"bad duration",
				serde_json::json!({ "minimum_uptime": "banana" }),
			),
			(
				"calendar unit",
				serde_json::json!({ "minimum_uptime": "1mo" }),
			),
			("negative", serde_json::json!({ "minimum_uptime": "-1h" })),
			(
				"sub-second",
				serde_json::json!({ "minimum_uptime": "0.5s" }),
			),
			("bare number", serde_json::json!({ "minimum_uptime": "20" })),
			("bad size unit", serde_json::json!({ "max_size": "20T" })),
			("size junk", serde_json::json!({ "max_size": "lots" })),
		] {
			let resp = private
				.post("/api/restore_replicas/create")
				.json(&serde_json::json!({
					"consumer_device_id": consumer,
					"group_id": group,
					"machine_id": null,
					"type": "tamanu-postgres",
					"intent": "sizing",
					"name": "bad-decl",
					"overdue_after": null,
					"params": params,
				}))
				.await;
			assert_eq!(resp.status_code(), 400, "case {name:?}");
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_rejects_bad_overdue_bound_and_allows_blank() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "bad-bound",
				"overdue_after": "soon",
				"params": {},
			}))
			.await
			.assert_status_bad_request();

		// A blank bound means no bound, same as null.
		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "no-bound",
				"overdue_after": "  ",
				"params": {},
			}))
			.await
			.assert_status_ok();

		let stored = database::RestoreReplica::list_for_group(&mut conn, group)
			.await
			.expect("list replicas");
		assert_eq!(stored.len(), 1);
		assert!(stored[0].overdue_after.is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn consumers_format_unit_param_defaults_for_display() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let consumer = insert_consumer(&mut conn).await;
		advertise_sizing(&mut conn, consumer).await;

		let resp = private
			.post("/api/restore_replicas/consumers")
			.json(&serde_json::json!({}))
			.await;
		resp.assert_status_ok();
		let rows: Vec<serde_json::Value> = resp.json();
		assert_eq!(rows.len(), 1, "got {rows:?}");
		let params = &rows[0]["intents"][0]["params"];
		assert_eq!(params["minimum_uptime"]["default"], "2h");
		assert!(params["max_size"].get("default").is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_and_update_reject_a_name_the_consumer_already_uses() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify_and_analytics(&mut conn, consumer).await;

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "verify",
				"name": "nightly",
				"overdue_after": null,
				"params": {},
			}))
			.await
			.assert_status_ok();

		// A different intent is a different scope, so the scope check passes —
		// but the name is the consumer's already, and two same-named replicas
		// are indistinguishable to consumer and operator alike.
		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "nightly",
				"overdue_after": null,
				"params": {},
			}))
			.await
			.assert_status_conflict();

		private
			.post("/api/restore_replicas/create")
			.json(&serde_json::json!({
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "",
				"overdue_after": null,
				"params": {},
			}))
			.await
			.assert_status_bad_request();

		let other = create_replica(
			&private,
			consumer,
			group,
			"analytics",
			serde_json::json!({}),
		)
		.await;
		private
			.post("/api/restore_replicas/update")
			.json(&serde_json::json!({
				"id": other,
				"consumer_device_id": consumer,
				"group_id": group,
				"machine_id": null,
				"type": "tamanu-postgres",
				"intent": "analytics",
				"name": "nightly",
				"overdue_after": null,
				"params": {},
				"enabled": true,
			}))
			.await
			.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn delete_succeeds_for_a_declaration_with_restore_health_history() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group = insert_group(&mut conn).await;
		let server = insert_server(&mut conn, group).await;
		let consumer = insert_consumer(&mut conn).await;
		advertise_verify(&mut conn, consumer).await;
		let id = create_replica(&private, consumer, group, "verify", serde_json::json!({})).await;

		conn.batch_execute(&format!(
			"INSERT INTO backup_restore_checks \
			 (replica_id, consumer_device_id, group_id, machine_id, type, intent, \
			  snapshot_id, outcome, replica_healthy, observed_at) \
			 VALUES ('{id}', '{consumer}', '{group}', '{server}', 'tamanu-postgres', \
			  'verify', 'snap-1', 'success', true, now())"
		))
		.await
		.expect("insert report");

		private
			.post("/api/restore_replicas/delete")
			.json(&serde_json::json!({ "id": id }))
			.await
			.assert_status_ok();

		let checks = database::BackupRestoreCheck::list_recent_for_group(&mut conn, group, 10)
			.await
			.expect("list checks");
		assert_eq!(checks.len(), 1, "the report outlives the declaration");
		assert_eq!(checks[0].replica_id, None);
	})
	.await;
}
