//! HTTP tests for the managed-restore endpoints (backup-restore role). The
//! worklist/capability paths run against the standard harness (no STS/kube
//! needed); restore-credentials is covered for its authz (403) and the
//! authorized-but-unconfigured (502) paths.

use diesel::{sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn make_group(conn: &mut AsyncPgConnection) -> Uuid {
	let id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'restore-test-group')")
		.bind::<sql_types::Uuid, _>(id)
		.execute(conn)
		.await
		.expect("insert group");
	id
}

async fn make_config(conn: &mut AsyncPgConnection, group_id: Uuid, status: &str) {
	sql_query(
		"INSERT INTO server_group_backup_config \
		 (group_id, bucket, prefix, target_role_arn, maintenance_role_arn, region, repo_password_ref, status) \
		 VALUES ($1, 'grp-bucket', '', 'arn:aws:iam::123456789012:role/grp', 'arn:aws:iam::123456789012:role/grp-maint', 'ap-southeast-2', 'grp-repo-pw', $2)",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Text, _>(status)
	.execute(conn)
	.await
	.expect("insert config");
}

/// A machine and the one application on it, sharing an id.
///
/// The shared id is deliberate: it lets a test name one value where the code
/// wants a machine (a replica, a report) or an application (a candidate
/// version) without threading two around.
async fn make_server(conn: &mut AsyncPgConnection, group_id: Uuid) -> Uuid {
	let server_id = Uuid::new_v4();
	let host = format!("https://srv-{server_id}.example.com");
	sql_query(
		"WITH m AS (INSERT INTO machines (id, group_id) VALUES ($1, $3) RETURNING id) INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES ($1, $2, 'tamanu-central', 'production', $3, $1)",
	)
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Text, _>(host)
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert server");
	server_id
}

/// A successful `backup` run = the snapshot the worklist should surface.
async fn make_success_run(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
	group_id: Uuid,
	server_id: Uuid,
	snapshot_id: &str,
) {
	sql_query(
		"INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, snapshot_id) \
		 VALUES ($1, $2, $3, $4, 'tamanu-postgres', 'backup', 'success', $5)",
	)
	.bind::<sql_types::Uuid, _>(Uuid::new_v4())
	.bind::<sql_types::Uuid, _>(device_id)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(snapshot_id)
	.execute(conn)
	.await
	.expect("insert run");
}

async fn declare_replica(
	conn: &mut AsyncPgConnection,
	consumer: Uuid,
	group_id: Uuid,
	intent: &str,
) -> Uuid {
	sql_query(
		"INSERT INTO restore_replicas (consumer_device_id, group_id, type, intent, name) \
		 VALUES ($1, $2, 'tamanu-postgres', $3, $4) RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(consumer)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Text, _>(intent)
	.bind::<sql_types::Text, _>(format!("{intent}-decl"))
	.get_result::<RowId>(conn)
	.await
	.expect("insert declaration")
	.id
}

async fn declare_replica_server(
	conn: &mut AsyncPgConnection,
	consumer: Uuid,
	group_id: Uuid,
	machine_id: Uuid,
	intent: &str,
) {
	sql_query(
		"INSERT INTO restore_replicas (consumer_device_id, group_id, machine_id, type, intent, name) \
		 VALUES ($1, $2, $3, 'tamanu-postgres', $4, $5)",
	)
	.bind::<sql_types::Uuid, _>(consumer)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(intent)
	.bind::<sql_types::Text, _>(format!("{intent}-server-decl"))
	.execute(conn)
	.await
	.expect("insert server declaration");
}

#[tokio::test(flavor = "multi_thread")]
async fn capabilities_register_then_worklist_filters_by_intent() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;

			// Two whole-group declarations, different intents.
			declare_replica(&mut conn, device_id, group, "verify").await;
			declare_replica(&mut conn, device_id, group, "analytics").await;

			// Register only `verify`.
			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(
					&serde_json::json!({ "intents": [{"intent": "verify", "semantics": ["check", "once"]}] }),
				)
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();
			// Only the `verify` declaration is dispatched; `analytics` is a gap.
			assert_eq!(entries.len(), 1, "got {entries:?}");
			assert_eq!(entries[0]["intent"], "verify");
			assert_eq!(entries[0]["machine_id"], server.to_string());
			assert_eq!(entries[0]["snapshot_id"], "snap-1");
			assert_eq!(entries[0]["bucket"], "grp-bucket");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worklist_expands_group_wide_to_each_server() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server_a = make_server(&mut conn, group).await;
			let server_b = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server_a, "snap-a").await;
			make_success_run(&mut conn, device_id, group, server_b, "snap-b").await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(
					&serde_json::json!({ "intents": [{"intent": "verify", "semantics": ["check", "once"]}] }),
				)
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();
			// One whole-group declaration → one entry per live server, each with
			// its own latest snapshot.
			assert_eq!(entries.len(), 2, "got {entries:?}");
			let mut by_server: std::collections::HashMap<String, String> = entries
				.iter()
				.map(|e| {
					(
						e["machine_id"].as_str().unwrap().to_owned(),
						e["snapshot_id"].as_str().unwrap().to_owned(),
					)
				})
				.collect();
			assert_eq!(
				by_server.remove(&server_a.to_string()).as_deref(),
				Some("snap-a")
			);
			assert_eq!(
				by_server.remove(&server_b.to_string()).as_deref(),
				Some("snap-b")
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worklist_dispatches_every_named_replica_of_a_server() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			// Both a whole-group and a server-specific declaration of the same
			// (type, intent) cover this server, under names of their own.
			declare_replica(&mut conn, device_id, group, "verify").await;
			declare_replica_server(&mut conn, device_id, group, server, "verify").await;
			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(
					&serde_json::json!({ "intents": [{"intent": "verify", "semantics": ["check", "once"]}] }),
				)
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();
			// Two named replicas of that server, so two entries: the name is what
			// tells them apart, and neither stands in for the other.
			assert_eq!(entries.len(), 2, "got {entries:?}");
			assert!(
				entries
					.iter()
					.all(|e| e["machine_id"] == server.to_string())
			);
			let mut names: Vec<&str> = entries
				.iter()
				.map(|e| e["name"].as_str().unwrap())
				.collect();
			names.sort_unstable();
			assert_eq!(names, ["verify-decl", "verify-server-decl"]);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worklist_empty_without_registered_capabilities() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			make_server(&mut conn, group).await;
			declare_replica(&mut conn, device_id, group, "verify").await;

			// No capabilities registered → nothing dispatched.
			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();
			assert!(entries.is_empty(), "got {entries:?}");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worklist_once_suppresses_verified_snapshot_until_newer() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"intents": [{"intent": "verify", "semantics": ["check", "once"]}]
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			// Before verification, snap-1 is on the worklist.
			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "got {entries:?}");

			// Report snap-1 verified healthy → a `once` intent drops it. The
			// report names the replica it came from, as a consumer's does: the
			// name is part of a replica's identity, so a report that named no
			// declaration settles nothing for one.
			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entries[0]["replica_id"],
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": "snap-1",
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-06-30T00:00:00Z",
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert!(
				entries.is_empty(),
				"verified snapshot suppressed: {entries:?}"
			);

			// A newer snapshot brings the entry back.
			make_success_run(&mut conn, device_id, group, server, "snap-2").await;
			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "newer snapshot reappears: {entries:?}");
			assert_eq!(entries[0]["snapshot_id"], "snap-2");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn worklist_resolves_params_with_defaults_and_nulls() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			declare_replica(&mut conn, device_id, group, "analytics").await;
			// Advertise analytics with a defaulted duration and an unset bytes cap.
			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"intents": [{
						"intent": "analytics",
						"semantics": ["check", "url"],
						"params": {
							"minimum_uptime": {"type": "duration", "default": 7200},
							"max_size": {"type": "bytes"},
						},
					}]
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "got {entries:?}");
			// Unset-with-default → default; unset-without-default → null.
			assert_eq!(entries[0]["params"]["minimum_uptime"], 7200);
			assert_eq!(entries[0]["params"]["max_size"], serde_json::Value::Null);
		},
	)
	.await;
}

/// A server of a type with no masking manifest, so a redacting
/// declaration covering it has nothing to redact with.
async fn make_senaite_server(conn: &mut AsyncPgConnection, group_id: Uuid) -> Uuid {
	let server_id = Uuid::new_v4();
	let host = format!("https://lims-{server_id}.example.com");
	sql_query(
		"WITH m AS (INSERT INTO machines (id, group_id) VALUES ($1, $3) RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ($1, $2, 'senaite', $3, $1)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group_id)
	.execute(conn)
	.await
	.expect("insert senaite server");
	server_id
}

/// Advertise `analytics` with `redact` and the three masking parameters, as
/// the restore consumer does.
async fn register_redact_intent(public: &axum_test::TestServer, cert: &str) {
	public
		.post("/restore-capabilities")
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
		.json(&serde_json::json!({
			"intents": [{
				"intent": "analytics",
				"semantics": ["check", "url", "redact"],
				"params": {
					"redaction_manifest_url": {"type": "text"},
					"redaction_version_query": {"type": "text"},
					"redaction_version_fallback_to_base": {"type": "boolean", "default": false},
				},
			}]
		}))
		.await
		.assert_status(http::StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_redacting_declaration_carries_the_products_masking_manifest() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			declare_replica(&mut conn, device_id, group, "analytics").await;
			sql_query("UPDATE restore_replicas SET redacts = true")
				.execute(&mut conn)
				.await
				.expect("turn redaction on");
			register_redact_intent(&public, &cert).await;

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "got {entries:?}");
			let params = &entries[0]["params"];
			assert_eq!(
				params["redaction_manifest_url"],
				"https://docs.data.bes.au/tamanu/v{version}/manifest.json"
			);
			assert!(
				params["redaction_version_query"]
					.as_str()
					.expect("a version query")
					.contains("local_system_facts"),
				"got {params:?}"
			);
			assert_eq!(params["redaction_version_fallback_to_base"], true);
		},
	)
	.await;
}

/// The manifest URL is what turns redaction on for the consumer, so a
/// declaration that doesn't redact has to send it unset — including when an
/// operator's stored value says otherwise, since Canopy owns the parameter.
#[tokio::test(flavor = "multi_thread")]
async fn a_declaration_that_doesnt_redact_sends_no_manifest() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			declare_replica(&mut conn, device_id, group, "analytics").await;
			sql_query(
				"UPDATE restore_replicas SET params = '{\"redaction_manifest_url\": \
				 \"https://evil.example/manifest.json\"}'::jsonb",
			)
			.execute(&mut conn)
			.await
			.expect("store a manifest URL behind canopy's back");
			register_redact_intent(&public, &cert).await;

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "got {entries:?}");
			assert_eq!(
				entries[0]["params"]["redaction_manifest_url"],
				serde_json::Value::Null,
				"a stored value must not make a non-redacting replica redact"
			);
		},
	)
	.await;
}

/// A replica that can't be redacted isn't restored at all: an unredacted
/// replica standing in for a redacted one is worse than no replica.
#[tokio::test(flavor = "multi_thread")]
async fn a_server_whose_product_has_no_manifest_is_withheld() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let tamanu = make_server(&mut conn, group).await;
			let senaite = make_senaite_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, tamanu, "snap-1").await;
			make_success_run(&mut conn, device_id, group, senaite, "snap-2").await;
			declare_replica(&mut conn, device_id, group, "analytics").await;
			sql_query("UPDATE restore_replicas SET redacts = true")
				.execute(&mut conn)
				.await
				.expect("turn redaction on");
			register_redact_intent(&public, &cert).await;

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "got {entries:?}");
			assert_eq!(
				entries[0]["machine_id"],
				tamanu.to_string(),
				"only the server with a manifest is dispatched"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_credentials_without_declaration_is_403() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, _device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let resp = public
				.post("/restore-credentials")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "group": group, "type": "tamanu-postgres" }))
				.await;
			resp.assert_status(http::StatusCode::FORBIDDEN);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_credentials_authorized_but_unconfigured_is_502() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			declare_replica(&mut conn, device_id, group, "verify").await;

			// Authorization passes; the harness has no STS client, so issuance
			// fails upstream rather than 403.
			let resp = public
				.post("/restore-credentials")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "group": group, "type": "tamanu-postgres" }))
				.await;
			resp.assert_status(http::StatusCode::BAD_GATEWAY);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_endpoints_reject_non_consumer_role() {
	// A `server`-role device cannot reach the backup-restore endpoints.
	commons_tests::server::run_with_device_auth(
		"server",
		async |_conn, cert, _device_id, public, _| {
			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status(http::StatusCode::FORBIDDEN);
		},
	)
	.await;
}

#[derive(diesel::QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

async fn count(conn: &mut AsyncPgConnection, query: &str, group: Uuid) -> i64 {
	sql_query(query)
		.bind::<sql_types::Uuid, _>(group)
		.get_result::<Count>(conn)
		.await
		.expect("count")
		.count
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_verification_without_declaration_is_403() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, _device_id, public, _| {
			let group = make_group(&mut conn).await;
			let server = make_server(&mut conn, group).await;
			let resp = public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					// Nothing is declared, so authorization refuses before the
					// declaration this names is ever looked for.
					"replica_id": Uuid::new_v4(),
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"outcome": "failure",
					"replica_healthy": false,
					"observed_at": "2026-06-30T00:00:00Z",
				}))
				.await;
			resp.assert_status(http::StatusCode::FORBIDDEN);
		},
	)
	.await;
}

/// A report has to say which replica it is about, and that replica has to be
/// one that still exists: several replicas can share a scope, so a report
/// naming a retired declaration could not be attributed to any of them, and
/// recording it would hold a finding against a replica nothing declares.
#[tokio::test(flavor = "multi_thread")]
async fn restore_verification_naming_a_retired_declaration_is_404() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			let server = make_server(&mut conn, group).await;
			// A second declaration keeps the consumer authorized for the
			// (group, type) after the one being reported on is retired.
			declare_replica(&mut conn, device_id, group, "verify").await;
			let retired = declare_replica(&mut conn, device_id, group, "analytics").await;
			sql_query("DELETE FROM restore_replicas WHERE id = $1")
				.bind::<sql_types::Uuid, _>(retired)
				.execute(&mut conn)
				.await
				.expect("retire declaration");

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": retired,
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "analytics",
					"outcome": "failure",
					"replica_healthy": false,
					"observed_at": "2026-06-30T00:00:00Z",
				}))
				.await
				.assert_status(http::StatusCode::NOT_FOUND);

			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM backup_restore_checks WHERE group_id = $1",
					group,
				)
				.await,
				0,
				"nothing recorded",
			);
		},
	)
	.await;
}

/// The name resolved from the declaration is what separates a scope's
/// replicas, so a consumer may only speak for its own declarations — otherwise
/// a report would grade someone else's replica on a restore that was never its.
#[tokio::test(flavor = "multi_thread")]
async fn restore_verification_naming_another_consumers_declaration_is_403() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			let server = make_server(&mut conn, group).await;
			declare_replica(&mut conn, device_id, group, "verify").await;

			let other: RowId =
				sql_query("INSERT INTO devices (role) VALUES ('backup-restore') RETURNING id")
					.get_result(&mut conn)
					.await
					.expect("other consumer");
			let theirs = declare_replica(&mut conn, other.id, group, "analytics").await;

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": theirs,
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "analytics",
					"outcome": "failure",
					"replica_healthy": false,
					"observed_at": "2026-06-30T00:00:00Z",
				}))
				.await
				.assert_status(http::StatusCode::FORBIDDEN);

			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM backup_restore_checks WHERE group_id = $1",
					group,
				)
				.await,
				0,
				"nothing recorded",
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn restore_verification_records_and_raises_alert() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			let server = make_server(&mut conn, group).await;
			let replica = declare_replica(&mut conn, device_id, group, "verify").await;

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": replica,
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": "snap-1",
					"outcome": "failure",
					"error": "restore failed",
					"replica_healthy": false,
					"observed_at": "2026-06-30T00:00:00Z",
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM backup_restore_checks WHERE group_id = $1",
					group,
				)
				.await,
				1,
			);

			// The report is recorded on ingest; the checks it feeds are filed by
			// the sweep, which is their sole filer (see BackupRestoreCheck::
			// record_report).
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep");
			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM issues i \
					 JOIN machines m ON m.id = i.machine_id \
					 WHERE m.group_id = $1 AND i.ref = 'restore-verification' AND i.active = true",
					group,
				)
				.await,
				1,
			);
			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM issues i \
					 JOIN machines m ON m.id = i.machine_id \
					 WHERE m.group_id = $1 AND i.ref = 'redaction'",
					group,
				)
				.await,
				0,
				"a report carrying no redaction files no redaction check at all",
			);
		},
	)
	.await;
}

/// A partial redaction is live and mostly masked, so the restore is healthy
/// and the finding belongs to the redaction — two signals from one report.
#[tokio::test(flavor = "multi_thread")]
async fn a_partial_redaction_warns_while_the_restore_stays_healthy() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			let server = make_server(&mut conn, group).await;
			let replica = declare_replica(&mut conn, device_id, group, "analytics").await;

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": replica,
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "analytics",
					"snapshot_id": "snap-1",
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-06-30T00:00:00Z",
					"redaction": {
						"outcome": "partial",
						"manifest_version": "2.41.3",
						"columns_masked": 118,
						"columns_skipped": 3,
					},
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM backup_restore_checks \
					 WHERE group_id = $1 AND redaction_outcome = 'partial' \
					 AND redaction_columns_skipped = 3",
					group,
				)
				.await,
				1,
				"the reported redaction is stored first-class",
			);

			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep");
			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM issues i \
					 JOIN machines m ON m.id = i.machine_id \
					 WHERE m.group_id = $1 AND i.ref = 'redaction' AND i.active = true",
					group,
				)
				.await,
				1,
				"a partial redaction raises the redaction check",
			);
			assert_eq!(
				count(
					&mut conn,
					"SELECT count(*) AS count FROM issues i \
					 JOIN machines m ON m.id = i.machine_id \
					 WHERE m.group_id = $1 AND i.ref = 'restore-verification' AND i.active = true",
					group,
				)
				.await,
				0,
				"the backup restored, so restore health is untouched",
			);
		},
	)
	.await;
}

/// A failed redaction is reported when it settles, before any switchover:
/// the restore succeeded and says so, and the replica stays on its previous
/// data rather than serving anything unmasked.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_redaction_warns_and_then_recovers_when_it_applies() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			let server = make_server(&mut conn, group).await;
			let replica = declare_replica(&mut conn, device_id, group, "analytics").await;

			let report = |outcome: &'static str, error: Option<&'static str>| {
				serde_json::json!({
					"replica_id": replica,
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "analytics",
					"snapshot_id": "snap-1",
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-06-30T00:00:00Z",
					"redaction": { "outcome": outcome, "error": error },
				})
			};

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&report("failed", Some("manifest host unreachable")))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep");

			const ACTIVE_REDACTION_CHECKS: &str = "SELECT count(*) AS count FROM issues i \
				 JOIN machines m ON m.id = i.machine_id \
				 WHERE m.group_id = $1 AND i.ref = 'redaction' AND i.active = true";
			assert_eq!(
				count(&mut conn, ACTIVE_REDACTION_CHECKS, group).await,
				1,
				"a failed redaction warns",
			);

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&report("complete", None))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep");
			assert_eq!(
				count(&mut conn, ACTIVE_REDACTION_CHECKS, group).await,
				0,
				"a redaction that fully applies recovers the check",
			);
		},
	)
	.await;
}

/// A published version, so a server has something to be migration-tested
/// against.
async fn publish_version(conn: &mut AsyncPgConnection, minor: i32, patch: i32) -> Uuid {
	let version_id = Uuid::new_v4();
	sql_query(
		"INSERT INTO versions (id, major, minor, patch, status, changelog)
		 VALUES ($1, 2, $2, $3, 'published', '')",
	)
	.bind::<sql_types::Uuid, _>(version_id)
	.bind::<sql_types::Integer, _>(minor)
	.bind::<sql_types::Integer, _>(patch)
	.execute(conn)
	.await
	.expect("publish version");
	version_id
}

/// What the server reports itself as running, which is what a candidate is
/// measured against.
async fn report_version(conn: &mut AsyncPgConnection, server_id: Uuid, version: &str) {
	sql_query(
		"INSERT INTO application_reported_detail (application_id, source, extra, version)
		 VALUES ($1, 'test', '{}'::jsonb, $2)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(version)
	.execute(conn)
	.await
	.expect("report version");
}

/// The production environment's open plan, which is what names the version to
/// migrate to.
async fn plan_upgrade(conn: &mut AsyncPgConnection, group: Uuid, version_id: Uuid) {
	sql_query(
		"INSERT INTO upgrade_plans (group_id, rank, target_version_id, created_by)
		 VALUES ($1, 'production', $2, 'test@example.com')",
	)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Uuid, _>(version_id)
	.execute(conn)
	.await
	.expect("plan upgrade");
}

/// One intent that both verifies the restore and applies the migrations: the
/// `migrate` semantic rides on the verifying intent so a single restore answers
/// both questions.
async fn register_migrate_intent(public: &axum_test::TestServer, cert: &str) {
	public
		.post("/restore-capabilities")
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
		.json(&serde_json::json!({
			"intents": [{"intent": "verify", "semantics": ["check", "once", "migrate"]}]
		}))
		.await
		.assert_status(http::StatusCode::NO_CONTENT);
}

#[tokio::test(flavor = "multi_thread")]
async fn a_migrate_entry_names_the_planned_version() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			publish_version(&mut conn, 62, 0).await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();

			assert_eq!(entries.len(), 1, "got {entries:?}");
			assert_eq!(entries[0]["target_version"], "2.63.2");
			assert_eq!(entries[0]["target_version_id"], planned.to_string());
			assert_eq!(entries[0]["snapshot_id"], "snap-1");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_with_no_plan_gets_no_migrate_entry() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			publish_version(&mut conn, 63, 2).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();

			assert!(
				entries.is_empty(),
				"nobody has said this group is moving, so no entry: got {entries:?}"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_verdict_settles_the_snapshot_and_version_pair() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let before: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(before.len(), 1, "dispatched once: got {before:?}");

			// A failing migration is a settled answer for this snapshot: the
			// same run against the same data would fail the same way.
			database::migration_tests::MigrationTest::record(
				&mut conn,
				database::restore::NewBackupRestoreCheck {
					replica_id: None,
					replica_name: None,
					consumer_device_id: device_id,
					group_id: group,
					machine_id: Some(server),
					r#type: commons_types::backup::BackupType::TamanuPostgres,
					intent: commons_types::backup::RestoreIntent::from("verify"),
					snapshot_id: Some("snap-1".into()),
					// The restore itself was fine; only the migration failed.
					outcome: commons_types::backup::RunOutcome::Success,
					error: None,
					replica_healthy: true,
					postgres_version: Some("18".into()),
					observed_at: jiff::Timestamp::now(),
					s3_sent_raw_bytes: None,
					s3_sent_payload_bytes: None,
					s3_received_raw_bytes: None,
					s3_received_payload_bytes: None,
					health_details: None,
					run_id: None,
					redaction_outcome: None,
					redaction_manifest_version: None,
					redaction_columns_masked: None,
					redaction_columns_skipped: None,
					redaction_error: None,
				},
				database::migration_tests::NewMigrationTest {
					application_id: server,
					target_version_id: planned,
					total_elapsed: database::pg_duration::PgDuration(
						jiff::SignedDuration::from_secs(30),
					),
					failed_migration: Some("backfillNoteTypeIds".into()),
					data_bytes_before: 10,
					data_bytes_after: 10,
					timings: vec![],
				},
			)
			.await
			.expect("record failing test");

			let after: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert!(
				after.is_empty(),
				"a failure settles the pair rather than retrying: got {after:?}"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_reported_migration_test_lands_and_settles_the_entry() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let dispatched: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(dispatched.len(), 1, "got {dispatched:?}");
			let entry = &dispatched[0];

			// Report it back the way a consumer would: naming the version by its
			// semver, not echoing the identifier.
			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entry["replica_id"],
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": entry["snapshot_id"],
					"outcome": "success",
					"replica_healthy": true,
					"postgres_version": "18",
					"observed_at": "2026-07-30T00:00:00Z",
					"migration": {
						"target_version": entry["target_version"],
						"total_elapsed_seconds": 900,
						"data_bytes_before": 200_000_000_000i64,
						"data_bytes_after": 260_000_000_000i64,
						"timings": [
							{"name": "addIndexToFhirJobs", "elapsed_seconds": 12},
							{"name": "backfillNoteTypeIds", "elapsed_seconds": 880},
						],
					},
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			// The verdict is readable, and the timings survived the round trip.
			assert_eq!(
				database::migration_tests::verdict(&mut conn, server, planned)
					.await
					.expect("verdict"),
				database::migration_tests::Verdict::Passed
			);

			// And the pair is settled, so it is not dispatched again.
			let after: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert!(after.is_empty(), "got {after:?}");
		},
	)
	.await;
}

/// An older consumer that echoes the version's identifier instead of its semver
/// is still recorded: the endpoint resolves the version from whichever the
/// report carries.
#[tokio::test(flavor = "multi_thread")]
async fn a_migration_report_by_version_id_still_lands() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let dispatched: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			let entry = &dispatched[0];

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entry["replica_id"],
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": entry["snapshot_id"],
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-07-30T00:00:00Z",
					"migration": {
						"target_version_id": entry["target_version_id"],
						"total_elapsed_seconds": 900,
						"data_bytes_before": 10,
						"data_bytes_after": 10,
						"timings": [],
					},
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			assert_eq!(
				database::migration_tests::verdict(&mut conn, server, planned)
					.await
					.expect("verdict"),
				database::migration_tests::Verdict::Passed
			);
		},
	)
	.await;
}

/// A migration report that names its version neither way cannot be attributed
/// to one, and is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_migration_report_naming_no_version_is_refused() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let dispatched: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			let entry = &dispatched[0];

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entry["replica_id"],
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": entry["snapshot_id"],
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-07-30T00:00:00Z",
					"migration": {
						"total_elapsed_seconds": 900,
						"data_bytes_before": 10,
						"data_bytes_after": 10,
						"timings": [],
					},
				}))
				.await
				.assert_status(http::StatusCode::BAD_REQUEST);
		},
	)
	.await;
}

/// A semver that matches no known version cannot be resolved, and the report is
/// refused rather than stored against nothing.
#[tokio::test(flavor = "multi_thread")]
async fn a_migration_report_with_an_unknown_version_is_refused() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			report_version(&mut conn, server, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let dispatched: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			let entry = &dispatched[0];

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entry["replica_id"],
					"group": group,
					"machine_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": entry["snapshot_id"],
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-07-30T00:00:00Z",
					"migration": {
						"target_version": "9.9.9",
						"total_elapsed_seconds": 900,
						"data_bytes_before": 10,
						"data_bytes_after": 10,
						"timings": [],
					},
				}))
				.await
				.assert_status(http::StatusCode::NOT_FOUND);
		},
	)
	.await;
}

/// A box running two workloads is one machine, so a whole-group declaration
/// gives it one replica of its one snapshot. Expanding over applications gave
/// it two, both restoring the same backup.
// spec: RST#declared-replicas
#[tokio::test(flavor = "multi_thread")]
async fn a_two_workload_box_gets_one_replica_not_one_per_workload() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let machine = make_server(&mut conn, group).await;

			// A second workload on the same box.
			let second = Uuid::new_v4();
			sql_query(
				"INSERT INTO applications (id, host, type, group_id, machine_id) \
				 VALUES ($1, $2, 'tamanu-central', $3, $4)",
			)
			.bind::<sql_types::Uuid, _>(second)
			.bind::<sql_types::Text, _>(format!("https://second-{second}.example.com"))
			.bind::<sql_types::Uuid, _>(group)
			.bind::<sql_types::Uuid, _>(machine)
			.execute(&mut conn)
			.await
			.expect("second application");

			make_success_run(&mut conn, device_id, group, machine, "snap-a").await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(
					&serde_json::json!({ "intents": [{"intent": "verify", "semantics": ["check"]}] }),
				)
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let resp = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let entries: Vec<serde_json::Value> = resp.json();
			assert_eq!(
				entries.len(),
				1,
				"one box, one snapshot, one replica — not one per workload: {entries:?}"
			);
			assert_eq!(entries[0]["machine_id"], machine.to_string());
			assert_eq!(entries[0]["snapshot_id"], "snap-a");
		},
	)
	.await;
}

/// The one place the grains genuinely interleave: a migrate entry names the
/// machine whose snapshot it restores *and* the type of application whose
/// candidate it carries, because a snapshot is a box's and a version is a
/// workload's. The workload is named by its type, Canopy's own identifier for
/// an application being internal and never on the wire.
// spec: RST#dispatching-a-migration-test
#[tokio::test(flavor = "multi_thread")]
async fn a_migrate_entry_names_both_the_machine_and_the_candidates_application() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let machine = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, machine, "snap-1").await;
			report_version(&mut conn, machine, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let dispatched: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(dispatched.len(), 1, "got {dispatched:?}");
			let entry = &dispatched[0];

			assert_eq!(
				entry["machine_id"],
				machine.to_string(),
				"the data under test is the box's snapshot"
			);
			assert_eq!(
				entry["application_type"], "tamanu-central",
				"and the version under test is the workload's candidate"
			);
			assert_eq!(entry["snapshot_id"], "snap-1");
			assert_eq!(entry["target_version"], "2.63.2");
		},
	)
	.await;
}

/// A consumer built before a machine and the software on it became separate
/// records knows the restored box as `server_id`. Both sides of that exchange
/// keep working: the worklist still names it, and a report that names only it
/// is understood.
#[tokio::test(flavor = "multi_thread")]
async fn the_earlier_server_id_shape_still_works_end_to_end() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			declare_replica(&mut conn, device_id, group, "verify").await;

			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"intents": [{"intent": "verify", "semantics": ["check", "once"]}]
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert_eq!(entries.len(), 1, "got {entries:?}");
			// The entry carries both names for the one box, so a consumer on
			// either shape finds what to restore.
			assert_eq!(entries[0]["machine_id"], server.to_string());
			assert_eq!(entries[0]["server_id"], server.to_string());

			// A report in the earlier shape settles the replica exactly as one
			// naming `machine_id` does.
			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entries[0]["replica_id"],
					"group": group,
					"server_id": server,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": "snap-1",
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-06-30T00:00:00Z",
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			assert!(
				entries.is_empty(),
				"a report on the earlier shape verified the snapshot: {entries:?}"
			);
		},
	)
	.await;
}

/// Naming the box twice is refused rather than resolved by preference: a
/// reporter that disagrees with itself about what it restored has not been
/// understood, and guessing would grade a replica on the wrong restore.
#[tokio::test(flavor = "multi_thread")]
async fn a_report_naming_both_machine_and_server_is_refused() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let server = make_server(&mut conn, group).await;
			make_success_run(&mut conn, device_id, group, server, "snap-1").await;
			declare_replica(&mut conn, device_id, group, "verify").await;

			public
				.post("/restore-capabilities")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"intents": [{"intent": "verify", "semantics": ["check", "once"]}]
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			let entries: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();

			for body in [
				serde_json::json!({"machine_id": server, "server_id": server}),
				serde_json::json!({}),
			] {
				let mut args = serde_json::json!({
					"replica_id": entries[0]["replica_id"],
					"group": group,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": "snap-1",
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-06-30T00:00:00Z",
				});
				for (k, v) in body.as_object().unwrap() {
					args[k] = v.clone();
				}
				public
					.post("/restore-verification")
					.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
					.json(&args)
					.await
					.assert_status(http::StatusCode::BAD_REQUEST);
			}
		},
	)
	.await;
}

/// A consumer that predates the worklist entry carrying the application sends a
/// migration report without one. The box runs two workloads, so the sole-
/// application shortcut does not apply and the finding is resolved from the
/// machine and the version instead.
// spec: RST#dispatching-a-migration-test
#[tokio::test(flavor = "multi_thread")]
async fn a_migration_report_omitting_the_application_resolves_it_from_machine_and_version() {
	commons_tests::server::run_with_device_auth(
		"backup-restore",
		async |mut conn, cert, device_id, public, _| {
			let group = make_group(&mut conn).await;
			make_config(&mut conn, group, "ready").await;
			let machine = make_server(&mut conn, group).await;

			// A second workload on the same box, of a product with no upgrade
			// train through Tamanu's migrations.
			let second = Uuid::new_v4();
			sql_query(
				"INSERT INTO applications (id, host, type, group_id, machine_id) \
				 VALUES ($1, $2, 'senaite', $3, $4)",
			)
			.bind::<sql_types::Uuid, _>(second)
			.bind::<sql_types::Text, _>(format!("https://second-{second}.example.com"))
			.bind::<sql_types::Uuid, _>(group)
			.bind::<sql_types::Uuid, _>(machine)
			.execute(&mut conn)
			.await
			.expect("second application");

			make_success_run(&mut conn, device_id, group, machine, "snap-1").await;
			report_version(&mut conn, machine, "2.62.0").await;
			let planned = publish_version(&mut conn, 63, 2).await;
			plan_upgrade(&mut conn, group, planned).await;
			declare_replica(&mut conn, device_id, group, "verify").await;
			register_migrate_intent(&public, &cert).await;

			let dispatched: Vec<serde_json::Value> = public
				.get("/restore-worklist")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await
				.json();
			let entry = &dispatched[0];

			public
				.post("/restore-verification")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"replica_id": entry["replica_id"],
					"group": group,
					"machine_id": machine,
					"type": "tamanu-postgres",
					"intent": "verify",
					"snapshot_id": entry["snapshot_id"],
					"outcome": "success",
					"replica_healthy": true,
					"observed_at": "2026-07-30T00:00:00Z",
					"migration": {
						"target_version": entry["target_version"],
						"total_elapsed_seconds": 60,
						"data_bytes_before": 10,
						"data_bytes_after": 10,
						"timings": [],
					},
				}))
				.await
				.assert_status(http::StatusCode::NO_CONTENT);

			assert_eq!(
				database::migration_tests::verdict(&mut conn, machine, planned)
					.await
					.expect("verdict"),
				database::migration_tests::Verdict::Passed,
				"the finding lands on the workload whose candidate the version is",
			);
			assert_eq!(
				database::migration_tests::verdict(&mut conn, second, planned)
					.await
					.expect("verdict"),
				database::migration_tests::Verdict::NotTested,
				"and not on the other workload sharing the box",
			);
		},
	)
	.await;
}
