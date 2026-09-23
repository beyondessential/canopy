use commons_types::{namespace::Namespace, server::app_type::ApplicationType};
use database::check_policies::{CheckPolicy, ScopedCheckPolicy};
use database::issues::Scope;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct StatusResult {
	#[diesel(sql_type = sql_types::Uuid)]
	server_id: Uuid,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
	device_id: Option<Uuid>,
	#[diesel(sql_type = sql_types::Jsonb)]
	extra: serde_json::Value,
}

#[derive(QueryableByName)]
struct ExtraOnly {
	#[diesel(sql_type = sql_types::Jsonb)]
	extra: serde_json::Value,
}

#[derive(QueryableByName)]
struct HealthResult {
	#[diesel(sql_type = sql_types::Bool)]
	healthy: bool,
	#[diesel(sql_type = sql_types::Jsonb)]
	health: serde_json::Value,
	#[diesel(sql_type = sql_types::Jsonb)]
	extra: serde_json::Value,
}

#[derive(QueryableByName, Debug)]
struct IssueRow {
	#[diesel(sql_type = sql_types::Bool)]
	escalates: bool,
	#[diesel(sql_type = sql_types::Bool)]
	active: bool,
	#[diesel(sql_type = sql_types::Text)]
	message: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	description: Option<String>,
	#[diesel(sql_type = sql_types::Bool)]
	is_resolved: bool,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	observed_result: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	effective_result: Option<String>,
}

async fn fetch_issue(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	source: &str,
	r#ref: &str,
) -> Option<IssueRow> {
	sql_query(
		r#"
		SELECT escalates, active, message, description, (resolved_at IS NOT NULL) AS is_resolved,
			observed_result, effective_result
		FROM issues
		WHERE application_id = $1 AND source = $2 AND ref = $3
"#,
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(source)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result(conn)
	.await
	.ok()
}

#[derive(QueryableByName, Debug)]
struct EventCount {
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

async fn count_issues_for_server(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
) -> i64 {
	let c: EventCount = sql_query("SELECT COUNT(*) AS count FROM issues WHERE application_id = $1")
		.bind::<sql_types::Uuid, _>(server_id)
		.get_result(conn)
		.await
		.expect("count issues");
	c.count
}

#[derive(QueryableByName, Debug)]
struct IncidentRow {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

/// Pre-seed (or update) a policy row so a check's grading is known
/// up-front, expressed in the old severity vocabulary the tests speak:
/// critical → failed + escalates, error → failed, warning → warning,
/// info → passed, debug → skipped. Ingestion would otherwise
/// auto-insert at the default warning ceiling, which only opens an
/// incident when one already exists.
async fn set_check_severity(
	conn: &mut diesel_async::AsyncPgConnection,
	check_name: &str,
	severity: &str,
) {
	let (ceiling, escalates) = match severity {
		"critical" => ("failed", true),
		"error" => ("failed", false),
		"warning" => ("warning", false),
		"info" => ("passed", false),
		"debug" => ("skipped", false),
		other => panic!("unknown severity {other}"),
	};
	// Every server that grades against this seed is a tamanu-central, so the
	// row has to land in the namespace ingest will look it up under.
	let (subject, application_type) =
		Namespace::for_application("alertd", check_name, &ApplicationType::TamanuCentral)
			.to_columns();
	sql_query(
		"INSERT INTO check_policies \
		 (source, subject, application_type, check_name, ceiling, escalates, reviewed_at, reviewed_by) \
		 VALUES ('alertd', $1, $2, $3, $4, $5, NOW(), 'test') \
		 ON CONFLICT (source, subject, application_type, check_name) DO UPDATE \
		 SET ceiling = EXCLUDED.ceiling, \
		     escalates = EXCLUDED.escalates, \
		     reviewed_at = EXCLUDED.reviewed_at, \
		     reviewed_by = EXCLUDED.reviewed_by",
	)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(subject)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(application_type)
	.bind::<sql_types::Text, _>(check_name)
	.bind::<sql_types::Text, _>(ceiling)
	.bind::<sql_types::Bool, _>(escalates)
	.execute(conn)
	.await
	.expect("seed catalog policy");
}

async fn fetch_open_incident(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
) -> Option<IncidentRow> {
	sql_query(
		"SELECT i.id FROM incidents i \
		 JOIN applications s ON i.server_group_id = s.group_id \
		 WHERE s.id = $1 AND i.closed_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.get_result(conn)
	.await
	.ok()
}

/// Recovery leaves the incident lingering (close-side grace) rather than
/// closing it on the spot; backdate the stamp past any window and sweep,
/// the test-speed way to let the linger elapse.
async fn expire_linger(conn: &mut diesel_async::AsyncPgConnection) {
	sql_query(
		"UPDATE incidents SET closing_at = closing_at - INTERVAL '1 hour' \
		 WHERE closing_at IS NOT NULL",
	)
	.execute(conn)
	.await
	.expect("expire linger");
	database::issues::sweep_lingering_incidents(conn)
		.await
		.expect("linger sweep");
}

/// Stand in for the monitor pod's reeval worker: drain every queued
/// server so the test sees the settled incident state that the ingest
/// request no longer computes inline.
async fn drain_reeval(conn: &mut diesel_async::AsyncPgConnection) {
	database::issues::process_incident_reeval_queue(conn, i64::MAX)
		.await
		.expect("drain incident reeval queue");
}

/// The ingest request records the status and per-check issues, but incident
/// (re-)evaluation — the part that takes the per-group lock — is deferred to
/// the reeval worker. Nothing opens an incident until the queue is drained.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_defers_incident_open_until_reeval_drained() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_severity(&mut conn, "database", "error").await;

			// Raw post (bypassing the auto-draining test helpers) so we can
			// observe the state between ingest and reeval.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"healthy": false,
					"health": [ { "check": "database", "healthy": false } ],
				}))
				.await
				.assert_status_ok();

			// The per-check issue is recorded synchronously on the request.
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health/database")
					.await
					.is_some(),
				"per-check issue must be recorded on the ingest request"
			);
			// The incident is NOT opened by the request itself.
			assert!(
				fetch_open_incident(&mut conn, server_id).await.is_none(),
				"incident open must be deferred off the ingest request"
			);

			// The worker drains the queue and the incident opens.
			drain_reeval(&mut conn).await;
			assert!(
				fetch_open_incident(&mut conn, server_id).await.is_some(),
				"incident opens once the reeval queue is drained"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				r#"
				WITH m AS (INSERT INTO machines (id, device_id) VALUES ($1, $2) RETURNING id) INSERT INTO applications (id, host, type, machine_id)
				VALUES ($1, 'https://test.example.com', 'tamanu-facility', $1)
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
			.execute(&mut conn)
			.await
			.expect("insert server");

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "uptime": 3600, "health": [] }))
				.await;
			response.assert_status_ok();
			response.assert_header("content-type", "application/json");

			// The response carries only the return-path fields; the stored
			// status record is not echoed back. Tags for this bare ungrouped
			// server are just the synthetic `canopy:type` and the two names it
			// replaced, which stay emitted for anything reading the earlier pair.
			// `names` is present but empty throughout: this server holds neither
			// grant and its group controls no domain, so it is entitled to nothing
			// — which is a fact worth stating on every push rather than an absence
			// the server has to infer. `applications` carries the same answer per
			// workload on the box: one entry here, since one application runs on
			// it.
			let body: serde_json::Value = response.json();
			assert_eq!(
				body,
				serde_json::json!({
					"backup_now": [],
					"check_severities": {},
					"names": {
						"may_manage_dns": false,
						"may_manage_tls": false,
						"paused": false,
						"domains": [],
						"registered_names": [],
						"certificates": [],
						"applications": [{
							"type": "tamanu-facility",
							"may_manage_dns": false,
							"may_manage_tls": false,
							"paused": false,
							"domains": [],
							"registered_names": [],
							"certificates": [],
						}],
					},
					"tags": {
						"canopy:type": "tamanu-facility",
						"canopy:product": "tamanu",
						"canopy:kind": "facility",
					},
				}),
			);

			// Verify the status was actually stored in the database
			let db_status: StatusResult = sql_query(
				r#"
				SELECT server_id, device_id, version, extra
				FROM statuses
				WHERE server_id = $1
				ORDER BY created_at DESC
				LIMIT 1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch created status");

			assert_eq!(db_status.server_id, server_id);
			assert_eq!(db_status.device_id, Some(device_id));
			assert_eq!(
				db_status.extra.get("uptime").and_then(|v| v.as_i64()),
				Some(3600)
			);
		},
	)
	.await
}

/// The `tags` field on the status-push response must be exactly what the
/// standalone `GET /tags` endpoint serves — same merge, same synthetic
/// `canopy:` tags, same billing labels.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_returns_effective_tags_matching_tags_endpoint() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			let server_id = Uuid::new_v4();
			sql_query(
				"INSERT INTO server_groups (id, name, tags) \
				 VALUES ($1, 'status-tags-cluster', '{\"region\": \"au\", \"env\": \"group\"}'::jsonb)",
			)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.expect("insert group");
			sql_query(
				"WITH m AS (INSERT INTO machines (id, group_id, device_id) VALUES ($1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, group_id, rank, tags, machine_id) \
				 VALUES ($1, 'https://tagged.example.com', 'tamanu-central', $3, 'production', \
				 '{\"env\": \"server\"}'::jsonb, $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await
			.expect("insert server");

			let tags_response = public
				.get("/tags")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			tags_response.assert_status_ok();
			let standalone_tags: serde_json::Value = tags_response.json();

			let response = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			response.assert_status_ok();
			let body: serde_json::Value = response.json();
			assert_eq!(body.get("tags"), Some(&standalone_tags));

			// Spot-check the merge itself so the equality above can't pass
			// vacuously: server tag wins the collision, group tag carries
			// through, and the synthetic tags are present.
			let tags = body.get("tags").and_then(|t| t.as_object()).expect("tags");
			assert_eq!(tags.get("env").and_then(|v| v.as_str()), Some("server"));
			assert_eq!(tags.get("region").and_then(|v| v.as_str()), Some("au"));
			assert_eq!(
				tags.get("canopy:kind").and_then(|v| v.as_str()),
				Some("central")
			);
			assert_eq!(
				tags.get("canopy:group-id").and_then(|v| v.as_str()),
				Some(group_id.to_string().as_str())
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_geolocation() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				r#"
				WITH m AS (INSERT INTO machines (id, device_id) VALUES ($1, $2) RETURNING id) INSERT INTO applications (id, host, type, geolocation, machine_id)
				VALUES ($1, 'https://test.example.com', 'tamanu-facility', ARRAY[-41.2865, 174.7762], $1)
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
			.execute(&mut conn)
			.await
			.expect("insert server with geolocation");

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "uptime": 7200, "version": "2.8.1", "health": [] }))
				.await;
			response.assert_status_ok();
			response.assert_header("content-type", "application/json");

			// Verify the status was actually stored in the database
			let db_status: StatusResult = sql_query(
				r#"
				SELECT server_id, device_id, version, extra
				FROM statuses
				WHERE server_id = $1
				ORDER BY created_at DESC
				LIMIT 1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch created status");

			assert_eq!(db_status.server_id, server_id);
			assert_eq!(db_status.device_id, Some(device_id));
			assert_eq!(
				db_status.extra.get("uptime").and_then(|v| v.as_i64()),
				Some(7200)
			);
			assert_eq!(
				db_status.extra.get("version").and_then(|v| v.as_str()),
				Some("2.8.1")
			);

			// Verify server still has geolocation
			#[derive(QueryableByName)]
			struct GeoCheck {
				#[diesel(sql_type = sql_types::Bool)]
				has_geolocation: bool,
			}

			let server_with_geo: GeoCheck = sql_query(
				r#"
				SELECT geolocation IS NOT NULL as has_geolocation
				FROM applications
				WHERE id = $1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch server geolocation status");

			assert!(
				server_with_geo.has_geolocation,
				"Application should have geolocation"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_cloud() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				r#"
				WITH m AS (INSERT INTO machines (id, device_id) VALUES ($1, $2) RETURNING id) INSERT INTO applications (id, host, type, cloud, machine_id)
				VALUES ($1, 'https://cloud.example.com', 'tamanu-central', true, $1)
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
			.execute(&mut conn)
			.await
			.expect("insert server with cloud");

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "uptime": 4800, "platform": "Linux", "health": [] }))
				.await;
			response.assert_status_ok();
			response.assert_header("content-type", "application/json");

			// Verify the status was actually stored in the database
			let db_status: StatusResult = sql_query(
				r#"
				SELECT server_id, device_id, version, extra
				FROM statuses
				WHERE server_id = $1
				ORDER BY created_at DESC
				LIMIT 1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch created status");

			assert_eq!(db_status.server_id, server_id);
			assert_eq!(db_status.device_id, Some(device_id));
			assert_eq!(
				db_status.extra.get("uptime").and_then(|v| v.as_i64()),
				Some(4800)
			);
			assert_eq!(
				db_status.extra.get("platform").and_then(|v| v.as_str()),
				Some("Linux")
			);

			// Verify server still has cloud field set to true
			#[derive(QueryableByName)]
			struct CloudCheck {
				#[diesel(sql_type = sql_types::Nullable<sql_types::Bool>)]
				cloud: Option<bool>,
			}

			let server_with_cloud: CloudCheck = sql_query(
				r#"
				SELECT cloud
				FROM applications
				WHERE id = $1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch server cloud status");

			assert_eq!(
				server_with_cloud.cloud,
				Some(true),
				"Application should have cloud=true"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_geolocation_and_cloud() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				r#"
				WITH m AS (INSERT INTO machines (id, device_id) VALUES ($1, $2) RETURNING id) INSERT INTO applications (id, host, type, geolocation, cloud, machine_id)
				VALUES ($1, 'https://full.example.com', 'tamanu-central', ARRAY[40.7128, -74.0060], false, $1)
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
			.execute(&mut conn)
			.await
			.expect("insert server with geolocation and cloud");

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(
					&serde_json::json!({ "uptime": 10000, "version": "3.0.0", "timezone": "America/New_York", "health": [] }),
				)
				.await;
			response.assert_status_ok();
			response.assert_header("content-type", "application/json");

			// Verify the status was actually stored in the database
			let db_status: StatusResult = sql_query(
				r#"
				SELECT server_id, device_id, version, extra
				FROM statuses
				WHERE server_id = $1
				ORDER BY created_at DESC
				LIMIT 1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch created status");

			assert_eq!(db_status.server_id, server_id);
			assert_eq!(db_status.device_id, Some(device_id));
			assert_eq!(
				db_status.extra.get("uptime").and_then(|v| v.as_i64()),
				Some(10000)
			);
			assert_eq!(
				db_status.extra.get("version").and_then(|v| v.as_str()),
				Some("3.0.0")
			);
			assert_eq!(
				db_status.extra.get("timezone").and_then(|v| v.as_str()),
				Some("America/New_York")
			);

			// Verify server still has both geolocation and cloud fields
			#[derive(QueryableByName)]
			struct FullCheck {
				#[diesel(sql_type = sql_types::Nullable<sql_types::Array<sql_types::Float8>>)]
				geolocation: Option<Vec<f64>>,
				#[diesel(sql_type = sql_types::Nullable<sql_types::Bool>)]
				cloud: Option<bool>,
			}

			let server_check: FullCheck = sql_query(
				r#"
				SELECT geolocation, cloud
				FROM applications
				WHERE id = $1
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("fetch server geolocation and cloud status");

			assert!(
				server_check.geolocation.is_some(),
				"Application should have geolocation"
			);
			if let Some(geo) = &server_check.geolocation {
				assert_eq!(geo.len(), 2, "Geolocation should have 2 values");
				assert!(
					(geo[0] - 40.7128).abs() < 0.0001,
					"Latitude should be ~40.7128"
				);
				assert!(
					(geo[1] - (-74.0060)).abs() < 0.0001,
					"Longitude should be ~-74.0060"
				);
			}

			assert_eq!(
				server_check.cloud,
				Some(false),
				"Application should have cloud=false"
			);
		},
	)
	.await
}

/// Helper used by the health-payload tests below. Bare server with a device
/// attached, no extras. Returns the created server's id.
async fn insert_health_test_server(
	conn: &mut diesel_async::AsyncPgConnection,
	device_id: Uuid,
) -> Uuid {
	// Application gets a group so events promote to incidents normally.
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'health-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	let server_id = Uuid::new_v4();
	sql_query(
		r#"
		WITH m AS (INSERT INTO machines (id, group_id, device_id) VALUES ($1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id)
		VALUES ($1, 'https://health.example.com', 'tamanu-central', $3, $1)
	"#,
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
	.bind::<sql_types::Uuid, _>(group_id)
	.execute(conn)
	.await
	.expect("insert server");
	server_id
}

async fn fetch_latest_health(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
) -> HealthResult {
	sql_query(
		r#"
		SELECT healthy, health, extra
		FROM statuses
		WHERE server_id = $1
		ORDER BY created_at DESC
		LIMIT 1
	"#,
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.get_result(conn)
	.await
	.expect("fetch latest health")
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_legacy_no_healthy_field() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "uptime": 100, "health": [] }))
				.await;
			response.assert_status_ok();

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert!(row.healthy, "absent `healthy` ⇒ true (legacy compat)");
			assert_eq!(row.health, serde_json::json!([]));
			assert_eq!(row.extra.get("uptime").and_then(|v| v.as_i64()), Some(100));
			assert!(
				row.extra.get("healthy").is_none(),
				"`healthy` must not leak into `extra`"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_healthy_true_no_checks() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "healthy": true, "health": [] }))
				.await;
			response.assert_status_ok();

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert!(row.healthy);
			assert_eq!(row.health, serde_json::json!([]));
			assert_eq!(row.extra, serde_json::json!({}));
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_healthy_false_persists() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"healthy": false,
					"uptime": 42,
					"health": [],
				}))
				.await;
			response.assert_status_ok();

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert!(!row.healthy);
			assert_eq!(row.extra.get("uptime").and_then(|v| v.as_i64()), Some(42));
			assert!(row.extra.get("healthy").is_none());
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_health_checks_persists() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"healthy": true,
					"health": [
						{ "check": "database", "healthy": true, "latency_ms": 12 },
						{ "check": "disk", "healthy": false, "free_pct": 4, "message": "almost full" },
					],
					"timezone": "UTC",
				}))
				.await;
			response.assert_status_ok();

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert!(row.healthy);
			let arr = row.health.as_array().expect("array");
			assert_eq!(arr.len(), 2);
			assert_eq!(arr[0]["check"], "database");
			assert_eq!(arr[0]["latency_ms"], 12);
			assert_eq!(arr[1]["check"], "disk");
			assert_eq!(arr[1]["free_pct"], 4);
			assert_eq!(arr[1]["message"], "almost full");
			assert!(
				row.extra.get("health").is_none(),
				"`health` must not leak into `extra`"
			);
			assert_eq!(
				row.extra.get("timezone").and_then(|v| v.as_str()),
				Some("UTC")
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rejects_non_bool_healthy() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "healthy": "yes" }))
				.await;
			response.assert_status_bad_request();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rejects_non_array_health() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": { "check": "x", "healthy": true } }))
				.await;
			response.assert_status_bad_request();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rejects_health_entry_missing_check() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [ { "healthy": true } ]
				}))
				.await;
			response.assert_status_bad_request();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rejects_health_entry_missing_healthy() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [ { "check": "database" } ]
				}))
				.await;
			response.assert_status_bad_request();
		},
	)
	.await
}

// -----------------------------------------------------------------
// Legacy format (no `health` array): the tamanu/tasks heartbeat.
// -----------------------------------------------------------------

/// A legacy push — no `health` array — is accepted unconditionally and
/// transformed into the `tamanu` source reporting a single always-passing
/// `tasks` heartbeat check, with the push's extras recorded verbatim.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_legacy_transforms_to_tamanu_heartbeat() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "healthy": false, "uptime": 7 }),
			)
			.await;

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert_eq!(
				row.health,
				serde_json::json!([{ "check": "tasks", "result": "passed" }]),
			);
			assert_eq!(row.extra.get("uptime").and_then(|v| v.as_i64()), Some(7));

			// The heartbeat records healthy state under the tamanu source.
			let tasks = fetch_issue(&mut conn, server_id, "tamanu", "health/tasks")
				.await
				.expect("heartbeat state recorded");
			assert!(!tasks.active, "the heartbeat is not an issue");
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());

			#[derive(QueryableByName)]
			struct SourceRow {
				#[diesel(sql_type = sql_types::Text)]
				source: String,
			}
			let row: SourceRow = sql_query(
				"SELECT source FROM statuses WHERE server_id = $1 ORDER BY created_at DESC LIMIT 1",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("status row");
			assert_eq!(row.source, "tamanu");
		},
	)
	.await
}

/// A legacy push must not disturb another source's checks: with per-source
/// scoping, the tamanu heartbeat says nothing about alertd's open issues.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_legacy_leaves_other_sources_alone() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// New-style push with a failing check files a per-check issue.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "disk", "healthy": false, "free_pct": 4 } ],
				}),
			)
			.await;
			let before = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("per-check issue filed");
			assert!(before.active);

			// Legacy push: heartbeat under tamanu only.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "healthy": true, "uptime": 99 }),
			)
			.await;

			let after = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("per-check issue still present");
			assert!(after.active, "legacy push must not close the failing check");
		},
	)
	.await
}

// -----------------------------------------------------------------
// Phase 3 — event filing on healthy transitions.
// -----------------------------------------------------------------

async fn post_status(
	public: &axum_test::TestServer,
	cert: &str,
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	body: serde_json::Value,
) {
	let response = public
		.post(&format!("/status/{}", server_id))
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
		.json(&body)
		.await;
	response.assert_status_ok();
	// Incident evaluation is deferred off the ingest request to the reeval
	// worker; drain it here so callers observe the settled incident state,
	// as they would shortly after a real push.
	drain_reeval(conn).await;
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_ingest_gating() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			use commons_types::source::IngestMode;
			use database::source_policies::SourcePolicy;

			let server_id = insert_health_test_server(&mut conn, device_id).await;
			let body = serde_json::json!({
				"source": "alertd",
				"healthy": true,
				"health": [{ "check": "db", "result": "passed" }],
			});

			// deny: the push is rejected and nothing is recorded.
			SourcePolicy::set_ingest(&mut conn, "alertd", IngestMode::Deny)
				.await
				.expect("set deny");
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&body)
				.await
				.assert_status_forbidden();
			assert_eq!(count_issues_for_server(&mut conn, server_id).await, 0);

			// ignore: the push is accepted but the source's data isn't
			// recorded — while the backflow still comes back. A catalogued
			// check (independent of this push) proves the response is still
			// computed from server state, not short-circuited to empty.
			set_check_severity(&mut conn, "db", "warning").await;
			SourcePolicy::set_ingest(&mut conn, "alertd", IngestMode::Ignore)
				.await
				.expect("set ignore");
			let ignored = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&body)
				.await;
			ignored.assert_status_ok();
			assert_eq!(count_issues_for_server(&mut conn, server_id).await, 0);
			let backflow: serde_json::Value = ignored.json();
			assert_eq!(
				backflow["check_severities"]["db"], "warn",
				"an ignored push still returns backflow: {backflow}"
			);

			// allow: the push is ingested and its check recorded.
			SourcePolicy::set_ingest(&mut conn, "alertd", IngestMode::Allow)
				.await
				.expect("set allow");
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&body)
				.await
				.assert_status_ok();
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health/db")
					.await
					.is_some(),
				"an allowed source's check is recorded"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_legacy_files_no_health_issues() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "uptime": 1, "health": [] }),
			)
			.await;

			assert_eq!(
				count_issues_for_server(&mut conn, server_id).await,
				0,
				"legacy push (no healthy field) must not open any status/* issue"
			);
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_warning_check_only() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": true,
					"health": [ { "check": "disk", "healthy": false, "free_pct": 4 } ],
				}),
			)
			.await;

			let per_check = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("per-check issue filed");
			assert_eq!(per_check.effective_result.as_deref(), Some("warning"));
			assert!(per_check.active);
			assert!(!per_check.is_resolved);
			assert!(
				per_check
					.description
					.as_ref()
					.is_some_and(|d| d.contains("disk"))
			);
			assert!(per_check.message.contains("free_pct"));

			// Rollup is retired — never filed in any case.
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health")
					.await
					.is_none()
			);
			// Warning alone doesn't cross the incident threshold.
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_unhealthy_with_checks_opens_incident() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Operator has elevated these to Error in the catalog. Without
			// this seed, both would default to Warning and not open an
			// incident on their own.
			set_check_severity(&mut conn, "database", "error").await;
			set_check_severity(&mut conn, "disk", "error").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [
						{ "check": "database", "healthy": false, "lag_ms": 5000 },
						{ "check": "disk", "healthy": false },
						{ "check": "tls", "healthy": true },
					],
				}),
			)
			.await;

			// The (status, health) rollup was retired — only per-check issues now.
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health")
					.await
					.is_none(),
				"rollup issue must not be created"
			);

			for check in ["database", "disk"] {
				let i = fetch_issue(&mut conn, server_id, "alertd", &format!("health/{check}"))
					.await
					.unwrap_or_else(|| panic!("per-check issue for {check} missing"));
				assert_eq!(i.effective_result.as_deref(), Some("failed"), "{check}");
				assert!(!i.escalates, "{check}");
				assert!(i.active, "{check}");
			}
			// Passing checks get a state row too — but an inactive one that
			// never degraded, which the issue listings exclude.
			let tls = fetch_issue(&mut conn, server_id, "alertd", "health/tls")
				.await
				.expect("passing check records state");
			assert!(!tls.active, "passing state is not an active issue");
			// Per-check failures at the catalog's Error severity open an incident.
			assert!(fetch_open_incident(&mut conn, server_id).await.is_some());
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_unhealthy_no_checks_files_nothing() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "healthy": false, "health": [] }),
			)
			.await;

			// The retired rollup is no longer filed and there are no per-check
			// failures to file individual issues against, so the unhealthy flag
			// on its own produces nothing.
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health")
					.await
					.is_none(),
				"rollup issue must not be created"
			);
			assert_eq!(count_issues_for_server(&mut conn, server_id).await, 0);
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());
		},
	)
	.await
}

/// All failing checks are silenced at server scope → per-check issues
/// are still recorded (silence doesn't suppress the row) but none of
/// them join an incident. The retired rollup no longer factors in.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_all_failing_checks_silenced_opens_no_incident() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Pre-silence both failing checks at server scope.
			for check in ["database", "disk"] {
				ScopedCheckPolicy::silence(
					&mut conn,
					Scope::Application(server_id),
					"alertd",
					&Namespace::for_application("alertd", check, &ApplicationType::TamanuCentral),
					check,
					None,
				)
				.await
				.expect("seed silence");
			}

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [
						{ "check": "database", "healthy": false },
						{ "check": "disk", "healthy": false },
					],
				}),
			)
			.await;

			// `healthy` is persisted as-is now that the silence-proxy
			// correction is gone — operators see what bestool said.
			let row = fetch_latest_health(&mut conn, server_id).await;
			assert!(!row.healthy);

			// Check state still records (silence doesn't gate row creation)
			// with the observation intact, but the silence grades the
			// effective result to skipped so nothing raises.
			for check in ["database", "disk"] {
				let i = fetch_issue(&mut conn, server_id, "alertd", &format!("health/{check}"))
					.await
					.unwrap_or_else(|| panic!("per-check state for {check} missing"));
				assert!(!i.active, "{check} is silenced, so it must not raise");
				assert_eq!(i.observed_result.as_deref(), Some("failed"), "{check}");
				assert_eq!(i.effective_result.as_deref(), Some("skipped"), "{check}");
			}
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());
		},
	)
	.await
}

/// Partial silence: the unsilenced failing check (operator-elevated to
/// Error in the catalog) is enough to open an incident.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_with_partial_silence_opens_incident_for_unsilenced() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Silence only one of the two failing checks.
			ScopedCheckPolicy::silence(
				&mut conn,
				Scope::Application(server_id),
				"alertd",
				&Namespace::for_application("alertd", "database", &ApplicationType::TamanuCentral),
				"database",
				None,
			)
			.await
			.expect("seed silence");

			// Operator has elevated the unsilenced check to Error.
			set_check_severity(&mut conn, "disk", "error").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [
						{ "check": "database", "healthy": false },
						{ "check": "disk", "healthy": false },
					],
				}),
			)
			.await;

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert!(!row.healthy);
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health")
					.await
					.is_none(),
				"rollup must not be created"
			);
			// Disk is unsilenced and configured at Error severity → opens an incident.
			let disk = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("disk issue filed");
			assert_eq!(disk.effective_result.as_deref(), Some("failed"));
			assert!(!disk.escalates);
			assert!(fetch_open_incident(&mut conn, server_id).await.is_some());
		},
	)
	.await
}

/// In v1, per-check severity comes from the catalog and is therefore
/// independent of bestool's top-level `healthy` flag. Two consecutive
/// pushes — one with healthy=false and one with healthy=true — file
/// the same check at the same severity.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_per_check_severity_is_catalog_driven() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// First push: bestool reports overall unhealthy, but the
			// catalog hasn't been touched yet — disk defaults to Warning,
			// not Error.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "disk", "healthy": false } ],
				}),
			)
			.await;
			let after_first = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("per-check issue");
			assert_eq!(after_first.effective_result.as_deref(), Some("warning"));

			// Second push: bestool reports overall healthy. Same severity —
			// the catalog is the source of truth, not the top-level flag.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": true,
					"health": [ { "check": "disk", "healthy": false } ],
				}),
			)
			.await;
			let disk = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("per-check issue still present");
			assert_eq!(disk.effective_result.as_deref(), Some("warning"));
			assert!(disk.active, "still failing, must stay active");

			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health")
					.await
					.is_none(),
				"rollup is retired and must not exist"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_check_recovery_explicit() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "db", "healthy": false } ],
				}),
			)
			.await;
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": true,
					"health": [ { "check": "db", "healthy": true } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("per-check issue exists");
			assert!(!issue.active, "explicit healthy=true closes the issue");
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_check_recovery_via_drop() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "db", "healthy": false } ],
				}),
			)
			.await;
			// Second push omits `db` from health[] entirely — trust the
			// reporter, treat as recovered.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": true,
					"health": [],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("per-check issue exists");
			assert!(!issue.active);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_full_recovery_closes_incident() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Both checks need to be at Error for the initial push to open
			// an incident on its own; catalog default of Warning wouldn't.
			set_check_severity(&mut conn, "db", "error").await;
			set_check_severity(&mut conn, "disk", "error").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [
						{ "check": "db", "healthy": false },
						{ "check": "disk", "healthy": false },
					],
				}),
			)
			.await;
			assert!(fetch_open_incident(&mut conn, server_id).await.is_some());

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": true,
					"health": [
						{ "check": "db", "healthy": true },
						{ "check": "disk", "healthy": true },
					],
				}),
			)
			.await;

			expire_linger(&mut conn).await;
			assert!(
				fetch_open_incident(&mut conn, server_id).await.is_none(),
				"incident must auto-close when every contributing issue resolves"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_reachability_to_health_handoff() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// db is at Error in the catalog so the per-check is a
			// meaningful contributor that can hold the incident open
			// once reachability resolves.
			set_check_severity(&mut conn, "db", "error").await;

			// Initial state: server silent → reachability sweep files a
			// canopy/reachability issue at Critical (severity for `Gone`),
			// which opens an incident on the server's group.
			database::statuses::Status::sweep_staleness(&mut conn)
				.await
				.expect("reachability sweep");
			let reach_before = fetch_issue(&mut conn, server_id, "canopy", "reachability")
				.await
				.expect("reachability issue opened");
			assert!(reach_before.active);
			let incident_before = fetch_open_incident(&mut conn, server_id)
				.await
				.expect("incident opened by reachability");

			// Bump `db` to Error so that when reachability recovers later,
			// the per-check issue can hold the incident open on its own.
			// (Default Warning would let the incident close once reachability
			// leaves — see the severity-≥-error close rule.)
			set_check_severity(&mut conn, "db", "error").await;

			// Application pings in with a failing per-check; the per-check
			// issue (Error severity) joins the existing incident rather
			// than opening a separate one.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "db", "healthy": false } ],
				}),
			)
			.await;

			let per_check = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("per-check issue opened");
			assert!(per_check.active);
			let incident_after_push = fetch_open_incident(&mut conn, server_id)
				.await
				.expect("incident still open after health push");
			assert_eq!(
				incident_before.id, incident_after_push.id,
				"same incident absorbs both contributors"
			);

			// Reachability sweep runs again. Application's latest status is fresh
			// so the sweep closes the reachability issue. The incident must
			// stay open because the per-check issue is still contributing.
			database::statuses::Status::sweep_staleness(&mut conn)
				.await
				.expect("reachability sweep (recovery)");

			let reach_after = fetch_issue(&mut conn, server_id, "canopy", "reachability")
				.await
				.expect("reachability issue still queryable");
			assert!(!reach_after.active, "reachability auto-recovered");
			let incident_final = fetch_open_incident(&mut conn, server_id)
				.await
				.expect("incident stays open across reachability close");
			assert_eq!(incident_before.id, incident_final.id);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_keeps_incident_open_when_failure_swaps() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Operator has elevated db and disk to Error in the catalog so
			// either failing check opens an incident on its own.
			set_check_severity(&mut conn, "db", "error").await;
			set_check_severity(&mut conn, "disk", "error").await;

			// Prior state: db failing.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "db", "healthy": false } ],
				}),
			)
			.await;
			let incident_before = fetch_open_incident(&mut conn, server_id)
				.await
				.expect("incident opened by initial push");

			// New push: db has recovered, disk newly failing. Mix of opens
			// and closes in one push — the incident must stay open because
			// disk is now contributing in db's place.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [
						{ "check": "db", "healthy": true },
						{ "check": "disk", "healthy": false },
					],
				}),
			)
			.await;

			let incident_after = fetch_open_incident(&mut conn, server_id)
				.await
				.expect("incident must remain open");
			assert_eq!(
				incident_before.id, incident_after.id,
				"same incident, not a close+reopen flicker"
			);

			let db = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("db issue");
			assert!(!db.active, "db has recovered");
			let disk = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("disk issue");
			assert!(disk.active, "disk is new failure");
		},
	)
	.await
}

#[derive(QueryableByName, Debug)]
struct CatalogRow {
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Text)]
	ceiling: String,
	#[diesel(sql_type = sql_types::Bool)]
	pending_review: bool,
}

async fn fetch_catalog(
	conn: &mut diesel_async::AsyncPgConnection,
	check_name: &str,
) -> Option<CatalogRow> {
	sql_query(
		"SELECT source, ceiling, reviewed_at IS NULL AS pending_review \
		 FROM check_policies WHERE check_name = $1",
	)
	.bind::<sql_types::Text, _>(check_name)
	.get_result(conn)
	.await
	.ok()
}

/// A status push with a never-before-seen check name inserts a catalog
/// row at the default Warning severity, marked as pending review.
/// Applies to both failing and passing checks: operators should be able
/// to pre-configure severities before a check ever fails.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_seeds_catalog_for_new_checks() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			assert!(fetch_catalog(&mut conn, "brand_new_check").await.is_none());
			assert!(fetch_catalog(&mut conn, "passing_check").await.is_none());

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": true,
					"health": [
						{ "check": "brand_new_check", "healthy": false },
						{ "check": "passing_check", "healthy": true },
					],
				}),
			)
			.await;

			let failing = fetch_catalog(&mut conn, "brand_new_check")
				.await
				.expect("failing check seeded in catalog");
			assert_eq!(failing.source, "alertd");
			assert_eq!(failing.ceiling, "warning");
			assert!(failing.pending_review);

			let passing = fetch_catalog(&mut conn, "passing_check")
				.await
				.expect("passing check seeded in catalog");
			assert_eq!(passing.source, "alertd");
			assert_eq!(passing.ceiling, "warning");
			assert!(passing.pending_review);
		},
	)
	.await
}

/// Editing the catalog severity for a check changes the severity used
/// by subsequent status pushes' per-check issue filings.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_uses_catalog_severity_on_failure() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			set_check_severity(&mut conn, "tunable_check", "critical").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "tunable_check", "healthy": false } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/tunable_check")
				.await
				.expect("per-check issue filed");
			assert_eq!(issue.effective_result.as_deref(), Some("failed"));
			assert!(issue.escalates);
		},
	)
	.await
}

// ── v2 conditional rules ─────────────────────────────────────────────────

/// Write a raw JsonLogic blob to the catalog row's `rules` column. The
/// helper bypasses the typed API on purpose: tests want to assert the
/// ingestion path's behaviour for the constrained shape itself.
async fn set_check_rules(
	conn: &mut diesel_async::AsyncPgConnection,
	check_name: &str,
	rules: serde_json::Value,
) {
	// Ensure the catalog row exists, in the namespace ingest will read it
	// back out of.
	CheckPolicy::upsert_default(
		conn,
		"alertd",
		&Namespace::for_application("alertd", check_name, &ApplicationType::TamanuCentral),
		check_name,
	)
	.await
	.expect("ensure catalog row");
	// Setting rules reviews the policy in production (via update_rules), and
	// a pending-review policy is hard-capped at warning regardless of its
	// rules — so stamp the review here to mirror the real path.
	sql_query(
		"UPDATE check_policies SET rules = $1::jsonb, reviewed_at = NOW(), reviewed_by = 'test' \
		 WHERE check_name = $2",
	)
	.bind::<sql_types::Text, _>(rules.to_string())
	.bind::<sql_types::Text, _>(check_name)
	.execute(conn)
	.await
	.expect("set rules");
}

async fn set_server_tags(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	tags: serde_json::Value,
) {
	sql_query("UPDATE applications SET tags = $1::jsonb WHERE id = $2")
		.bind::<sql_types::Text, _>(tags.to_string())
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(conn)
		.await
		.expect("set server tags");
}

/// A rule predicated on `check.<field>` raises the per-check issue
/// to its branch severity, overriding the catalog base. Below-threshold
/// pushes fall back to base.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rule_on_check_extra_overrides_base() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_rules(
				&mut conn,
				"disk_space",
				serde_json::json!({"if": [
					{">": [{"var": "check.used_pct"}, 90]}, "failed"
				]}),
			)
			.await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [{"check": "disk_space", "healthy": false, "used_pct": 95}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/disk_space")
				.await
				.expect("per-check issue filed");
			assert_eq!(issue.effective_result.as_deref(), Some("failed"));

			assert!(!issue.escalates);

			// Below-threshold push falls back to base (default warning).
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [{"check": "disk_space", "healthy": false, "used_pct": 50}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/disk_space")
				.await
				.expect("per-check issue still present");
			assert_eq!(issue.effective_result.as_deref(), Some("warning"));
		},
	)
	.await
}

/// `status.bestoolVersion` from the top-level extras drives an in_range
/// rule. Outside the range falls back to base.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rule_on_bestool_version_range() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_rules(
				&mut conn,
				"tamanu_service",
				serde_json::json!({"if": [
					{"in_range": [{"var": "status.bestoolVersion"}, ">=2.4.0 <2.5.4"]},
					"warning"
				]}),
			)
			.await;
			// Bump base to Error so the contrast is visible.
			set_check_severity(&mut conn, "tamanu_service", "error").await;

			// Inside range → warning.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"bestoolVersion": "2.4.7",
					"health": [{"check": "tamanu_service", "healthy": false}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/tamanu_service")
				.await
				.expect("per-check issue filed");
			assert_eq!(issue.effective_result.as_deref(), Some("warning"));

			// Outside range → falls back to base (error).
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"bestoolVersion": "2.6.0",
					"health": [{"check": "tamanu_service", "healthy": false}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/tamanu_service")
				.await
				.expect("per-check issue still present");
			assert_eq!(issue.effective_result.as_deref(), Some("failed"));
			assert!(!issue.escalates);
		},
	)
	.await
}

/// A rule on `tag.<key>` resolves against the server's merged tag map.
/// Servers without the tag fall through to base.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_rule_on_server_tag() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_server_tags(
				&mut conn,
				server_id,
				serde_json::json!({"environment": "prod"}),
			)
			.await;
			set_check_rules(
				&mut conn,
				"cert_expiry",
				serde_json::json!({"if": [
					{"==": [{"var": "tag.environment"}, "prod"]}, "failed"
				]}),
			)
			.await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [{"check": "cert_expiry", "healthy": false}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/cert_expiry")
				.await
				.expect("per-check issue filed");
			assert_eq!(
				issue.effective_result.as_deref(),
				Some("failed"),
				"tag.environment=prod fires the rule"
			);
		},
	)
	.await
}

/// Two-branch ladder; first-match-wins.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_tiered_ladder() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_rules(
				&mut conn,
				"cert_expiry",
				serde_json::json!({"if": [
					{"<": [{"var": "check.days_remaining"}, 7]},  "failed",
					{"<": [{"var": "check.days_remaining"}, 30]}, "warning"
				]}),
			)
			.await;

			// Within 7 days → error.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [{"check": "cert_expiry", "healthy": false, "days_remaining": 3}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/cert_expiry")
				.await
				.expect("issue");
			assert_eq!(issue.effective_result.as_deref(), Some("failed"));

			assert!(!issue.escalates);

			// Within 30 days but not 7 → warning.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [{"check": "cert_expiry", "healthy": false, "days_remaining": 15}],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/cert_expiry")
				.await
				.expect("issue");
			assert_eq!(issue.effective_result.as_deref(), Some("warning"));
		},
	)
	.await
}

// -----------------------------------------------------------------
// `result` enum form (passed / warning / failed / broken / skipped).
// See docs/plans/healthcheck-result-enum.md.
// -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_validation() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Every recognised value is accepted.
			for result in ["passed", "warning", "failed", "broken", "skipped"] {
				post_status(
					&public,
					&cert,
					&mut conn,
					server_id,
					serde_json::json!({
						"health": [ { "check": "db", "result": result } ],
					}),
				)
				.await;
			}

			// Unknown value → 400 (strict: canopy ships before any
			// bestool that adds enum values).
			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [ { "check": "db", "result": "exploded" } ],
				}))
				.await;
			response.assert_status_bad_request();

			// Non-string result → 400.
			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [ { "check": "db", "result": true } ],
				}))
				.await;
			response.assert_status_bad_request();

			// Both forms on one entry → 400.
			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [ { "check": "db", "result": "passed", "healthy": true } ],
				}))
				.await;
			response.assert_status_bad_request();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_failed_uses_catalog_severity() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_severity(&mut conn, "db", "error").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "failed", "lag_ms": 5000 } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("per-check issue filed");
			assert_eq!(issue.effective_result.as_deref(), Some("failed"));
			assert!(!issue.escalates);
			assert!(issue.active);
			assert!(
				issue
					.description
					.as_ref()
					.is_some_and(|d| d.contains("failed"))
			);
			assert!(issue.message.contains("lag_ms"));
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_warning_ignores_catalog_severity() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			// Even with the catalog at Critical, a warning result lands
			// at fixed Warning — the catalog column is for failures.
			set_check_severity(&mut conn, "db", "critical").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "warning" } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("per-check issue filed");
			assert_eq!(issue.effective_result.as_deref(), Some("warning"));
			assert!(issue.active);
			assert!(
				issue
					.description
					.as_ref()
					.is_some_and(|d| d.contains("warned"))
			);
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());
		},
	)
	.await
}

/// Custom rules can condition on the normalised `check.result` — and
/// they win over both the observed result and the ceiling. A warning
/// graded down to passed files nothing at all.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_rule_on_check_result() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_rules(
				&mut conn,
				"db",
				serde_json::json!({"if": [
					{"==": [{"var": "check.result"}, "warning"]}, "passed"
				]}),
			)
			.await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "warning" } ],
				}),
			)
			.await;

			let db = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("state row recorded");
			assert!(
				!db.active,
				"a warning graded to passed records healthy state, not an issue",
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_broken_warns_on_the_check_ref() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			// The ceiling grades definite results; a broken check with no
			// prior contribution warns regardless.
			set_check_severity(&mut conn, "db", "critical").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "broken", "error": "config not found" } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("broken files on the check's own ref");
			assert_eq!(
				issue.effective_result.as_deref(),
				Some("broken"),
				"nothing to retain: brokenness itself counts as a warning",
			);
			assert!(issue.active);
			assert!(
				issue
					.description
					.as_ref()
					.is_some_and(|d| d.contains("broken"))
			);
			assert!(issue.message.contains("config not found"));
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());

			// Recovery closes it.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "passed" } ],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("issue still exists");
			assert!(!issue.active);
			assert!(issue.message.contains("recovered"));
		},
	)
	.await
}

/// failed→broken: the issue stays open at the failure's contribution
/// (the broken check can't confirm the failure either way), and a later
/// definite pass closes it.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_failed_then_broken_retains_the_failure() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			set_check_severity(&mut conn, "db", "error").await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "failed" } ],
				}),
			)
			.await;
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "broken" } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("issue exists");
			assert!(issue.active, "broken must not close the failure");
			assert_eq!(
				issue.effective_result.as_deref(),
				Some("failed"),
				"the failure's contribution is retained while broken",
			);
			assert!(
				issue
					.description
					.as_ref()
					.is_some_and(|d| d.contains("broken")),
				"the headline says the check is broken",
			);
			assert!(
				fetch_open_incident(&mut conn, server_id).await.is_some(),
				"the retained error keeps the incident open",
			);

			// passed closes it and the incident follows.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "passed" } ],
				}),
			)
			.await;
			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("issue exists");
			assert!(!issue.active);
			expire_linger(&mut conn).await;
			assert!(fetch_open_incident(&mut conn, server_id).await.is_none());
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_skipped_closes_failure_with_message() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "cert", "result": "failed" } ],
				}),
			)
			.await;
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "cert", "result": "skipped" } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/cert")
				.await
				.expect("per-check issue exists");
			assert!(!issue.active, "skipped closes the failure");
			assert!(
				issue.message.contains("skipped"),
				"close message says skipped, not recovered: {}",
				issue.message
			);

			// A skipped check on its own files nothing.
			assert_eq!(count_issues_for_server(&mut conn, server_id).await, 1);
		},
	)
	.await
}

/// Skipped also closes a prior broken issue — the check is no longer
/// reporting itself broken.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_skipped_closes_broken() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "cert", "result": "broken" } ],
				}),
			)
			.await;
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "cert", "result": "skipped" } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/cert")
				.await
				.expect("issue exists");
			assert!(!issue.active);
		},
	)
	.await
}

/// Stored legacy rows and new result-form pushes interoperate: a check
/// that was failing in the `healthy: bool` form closes when the new
/// form reports passed.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_legacy_to_result_transition() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"healthy": false,
					"health": [ { "check": "db", "healthy": false } ],
				}),
			)
			.await;
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [ { "check": "db", "result": "passed" } ],
				}),
			)
			.await;

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/db")
				.await
				.expect("per-check issue exists");
			assert!(!issue.active, "result form closes legacy-form failure");
		},
	)
	.await
}

/// Passed/skipped/broken results still upsert catalog rows: a check
/// that's passing today might fail tomorrow, and operators should be
/// able to review its mapping in advance.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_result_all_kinds_upsert_catalog() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"health": [
						{ "check": "a_passed", "result": "passed" },
						{ "check": "b_skipped", "result": "skipped" },
						{ "check": "c_broken", "result": "broken" },
					],
				}),
			)
			.await;

			#[derive(QueryableByName)]
			struct NameRow {
				#[diesel(sql_type = sql_types::Text)]
				check_name: String,
			}
			// Reported checks only: canopy's own checks are catalogued at
			// startup and aren't what this push is being judged on.
			let rows: Vec<NameRow> = sql_query(
				"SELECT check_name FROM check_policies WHERE source <> 'canopy' \
				 ORDER BY check_name",
			)
			.get_results(&mut conn)
			.await
			.expect("list catalog");
			let names: Vec<&str> = rows.iter().map(|r| r.check_name.as_str()).collect();
			assert_eq!(names, vec!["a_passed", "b_skipped", "c_broken"]);
		},
	)
	.await
}

// --- backup_now signal on the status response -------------------------------

async fn seed_server_in_group(
	conn: &mut diesel_async::AsyncPgConnection,
	device_id: Uuid,
) -> (Uuid, Uuid) {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'backup-now-test')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	let server_id = Uuid::new_v4();
	sql_query(
		"WITH m AS (INSERT INTO machines (id, group_id, device_id) VALUES ($1, $3, $2) RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ($1, 'https://srv.example.com', 'tamanu-central', $3, $1)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(group_id))
	.execute(conn)
	.await
	.expect("insert server");
	(server_id, group_id)
}

async fn seed_backup_config(
	conn: &mut diesel_async::AsyncPgConnection,
	group_id: Uuid,
	status: &str,
) {
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

async fn enable_backup_capability(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#type: &str,
) {
	sql_query(
		"INSERT INTO machine_backup_capabilities (machine_id, type, enabled) VALUES ($1, $2, true)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(r#type)
	.execute(conn)
	.await
	.expect("insert capability");
}

fn backup_now(body: &serde_json::Value) -> Vec<String> {
	body["backup_now"]
		.as_array()
		.expect("backup_now array")
		.iter()
		.map(|v| v.as_str().expect("type string").to_string())
		.collect()
}

/// A box whose id differs from the workload's, which is every box the fleet
/// gains from here: only the migration's backfill made the two agree. The
/// backup instruction is the machine's, so it has to be read off the machine
/// even when an application id is sitting right there.
// spec: BKJ, STA#push
#[tokio::test(flavor = "multi_thread")]
async fn status_backup_now_reads_the_machine_not_the_application() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			let group_id = Uuid::new_v4();
			sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'unequal-ids')")
				.bind::<sql_types::Uuid, _>(group_id)
				.execute(&mut conn)
				.await
				.expect("insert group");
			let machine_id = Uuid::new_v4();
			let application_id = Uuid::new_v4();
			assert_ne!(machine_id, application_id);
			sql_query("INSERT INTO machines (id, group_id, device_id) VALUES ($1, $2, $3)")
				.bind::<sql_types::Uuid, _>(machine_id)
				.bind::<sql_types::Uuid, _>(group_id)
				.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
				.execute(&mut conn)
				.await
				.expect("insert machine");
			sql_query(
				"INSERT INTO applications (id, host, type, group_id, machine_id) \
				 VALUES ($1, 'https://unequal.example.com', 'tamanu-central', $2, $3)",
			)
			.bind::<sql_types::Uuid, _>(application_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.bind::<sql_types::Uuid, _>(machine_id)
			.execute(&mut conn)
			.await
			.expect("insert application");

			seed_backup_config(&mut conn, group_id, "ready").await;
			enable_backup_capability(&mut conn, machine_id, "tamanu-postgres").await;

			let resp = public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			resp.assert_status_ok();
			assert_eq!(
				backup_now(&resp.json()),
				vec!["tamanu-postgres"],
				"the due type is the machine's capability, not the application's id"
			);
		},
	)
	.await
}

/// An enabled type with no prior successful run is schedule-due (the seeded
/// 6h `tamanu-postgres` default applies), so the heartbeat names it.
#[tokio::test(flavor = "multi_thread")]
async fn status_signals_backup_now_for_due_schedule() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (server_id, group_id) = seed_server_in_group(&mut conn, device_id).await;
			seed_backup_config(&mut conn, group_id, "ready").await;
			enable_backup_capability(&mut conn, server_id, "tamanu-postgres").await;

			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			resp.assert_status_ok();
			assert_eq!(backup_now(&resp.json()), vec!["tamanu-postgres"]);
		},
	)
	.await
}

/// Only alertd runs backups: pushes from any other source (a named one,
/// or the legacy tamanu heartbeat) never receive the signal even when a
/// backup is due.
#[tokio::test(flavor = "multi_thread")]
async fn status_backup_now_only_for_alertd() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (server_id, group_id) = seed_server_in_group(&mut conn, device_id).await;
			seed_backup_config(&mut conn, group_id, "ready").await;
			enable_backup_capability(&mut conn, server_id, "tamanu-postgres").await;

			// A named non-alertd source.
			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "source": "seedling", "health": [] }))
				.await;
			resp.assert_status_ok();
			assert!(backup_now(&resp.json()).is_empty());

			// The legacy (no health array) push, attributed to tamanu.
			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "healthy": true }))
				.await;
			resp.assert_status_ok();
			assert!(backup_now(&resp.json()).is_empty());
		},
	)
	.await
}

/// A recent successful backup is within the interval ⇒ not due ⇒ no signal.
#[tokio::test(flavor = "multi_thread")]
async fn status_no_backup_now_when_recent_success() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (server_id, group_id) = seed_server_in_group(&mut conn, device_id).await;
			seed_backup_config(&mut conn, group_id, "ready").await;
			enable_backup_capability(&mut conn, server_id, "tamanu-postgres").await;
			sql_query(
				"INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, reported_at) \
				 VALUES ($1, $2, $3, $4, 'tamanu-postgres', 'backup', 'success', now())",
			)
			.bind::<sql_types::Uuid, _>(Uuid::new_v4())
			.bind::<sql_types::Uuid, _>(device_id)
			.bind::<sql_types::Uuid, _>(group_id)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(server_id))
			.execute(&mut conn)
			.await
			.expect("insert run");

			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			resp.assert_status_ok();
			assert!(backup_now(&resp.json()).is_empty());
		},
	)
	.await
}

/// The signal is gated on a `ready` config — a provisioning group emits nothing.
#[tokio::test(flavor = "multi_thread")]
async fn status_no_backup_now_until_config_ready() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (server_id, group_id) = seed_server_in_group(&mut conn, device_id).await;
			seed_backup_config(&mut conn, group_id, "provisioning").await;
			enable_backup_capability(&mut conn, server_id, "tamanu-postgres").await;

			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			resp.assert_status_ok();
			assert!(backup_now(&resp.json()).is_empty());
		},
	)
	.await
}

/// An operator one-off is surfaced, then cleared by `/backup-report` so the next
/// heartbeat stops naming it.
#[tokio::test(flavor = "multi_thread")]
async fn status_one_off_request_surfaced_then_cleared_by_report() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (server_id, group_id) = seed_server_in_group(&mut conn, device_id).await;
			seed_backup_config(&mut conn, group_id, "ready").await;
			// No enabled capability: the type appears only via the explicit request,
			// so clearing it (not a due schedule) is what empties the signal.
			sql_query(
				"INSERT INTO backup_requests (machine_id, type, purpose) VALUES ($1, 'tamanu-postgres', 'backup')",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.execute(&mut conn)
			.await
			.expect("enqueue request");

			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			resp.assert_status_ok();
			assert_eq!(backup_now(&resp.json()), vec!["tamanu-postgres"]);

			let report = public
				.post("/backup-report")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"run_id": Uuid::new_v4(),
					"type": "tamanu-postgres",
					"purpose": "backup",
					"outcome": "success",
				}))
				.await;
			report.assert_status(http::StatusCode::NO_CONTENT);

			let resp = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			resp.assert_status_ok();
			assert!(backup_now(&resp.json()).is_empty());
		},
	)
	.await
}

// -----------------------------------------------------------------
// Version sourcing: `tamanuVersion` payload extra supersedes the
// legacy `X-Version` header, and the header is now optional.
// -----------------------------------------------------------------

#[derive(QueryableByName)]
struct VersionRow {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	version: Option<String>,
}

async fn fetch_latest_version(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
) -> Option<String> {
	let row: VersionRow = sql_query(
		"SELECT version FROM statuses WHERE server_id = $1 ORDER BY created_at DESC LIMIT 1",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.get_result(conn)
	.await
	.expect("fetch latest status version");
	row.version
}

/// `run_with_device_auth` sends `X-Version: 3.4.5` by default. A payload
/// carrying `tamanuVersion` wins over that header.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_version_prefers_tamanu_version_over_header() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "tamanuVersion": "2.11.0", "health": [] }),
			)
			.await;

			assert_eq!(
				fetch_latest_version(&mut conn, server_id).await.as_deref(),
				Some("2.11.0"),
				"tamanuVersion in the payload supersedes the X-Version header"
			);
		},
	)
	.await
}

/// With no `tamanuVersion` in the payload, the version falls back to the
/// `X-Version` header (here the helper's default `3.4.5`).
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_version_falls_back_to_header() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "health": [] }),
			)
			.await;

			assert_eq!(
				fetch_latest_version(&mut conn, server_id).await.as_deref(),
				Some("3.4.5"),
				"absent tamanuVersion falls back to the X-Version header"
			);
		},
	)
	.await
}

/// The `X-Version` header is now optional: a push with no header but a
/// `tamanuVersion` in the body is accepted and records the payload version.
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_version_without_header_uses_payload() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Drop every default header (including the helper's X-Version),
			// then restore only what device auth / client IP need.
			let response = public
				.post(&format!("/status/{server_id}"))
				.clear_headers()
				.add_header("Forwarded", "for=192.0.1.60")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "tamanuVersion": "2.12.3", "health": [] }))
				.await;
			response.assert_status_ok();

			assert_eq!(
				fetch_latest_version(&mut conn, server_id).await.as_deref(),
				Some("2.12.3"),
				"missing X-Version is fine when the payload carries tamanuVersion"
			);
		},
	)
	.await
}

/// Neither source present: the push still succeeds and the row is
/// versionless (the column is nullable).
#[tokio::test(flavor = "multi_thread")]
async fn submit_status_versionless_when_neither_present() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{server_id}"))
				.clear_headers()
				.add_header("Forwarded", "for=192.0.1.60")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [] }))
				.await;
			response.assert_status_ok();

			assert_eq!(
				fetch_latest_version(&mut conn, server_id).await,
				None,
				"no tamanuVersion and no X-Version ⇒ versionless status"
			);
		},
	)
	.await
}

/// Two sources reporting disjoint check sets for the same server must not
/// close each other's issues: a push's "unmentioned means recovered" only
/// applies to the pushing source's own checks.
#[tokio::test(flavor = "multi_thread")]
async fn multi_source_pushes_do_not_flap() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// Source-less push: attributed to alertd, files its failure there.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "health": [{ "check": "db", "result": "failed" }] }))
				.await
				.assert_status_ok();
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health/db")
					.await
					.expect("alertd issue filed")
					.active
			);

			// A second source pushes a disjoint check set: its own failure
			// files under its name, and alertd's open issue is untouched.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "seedling",
					"health": [{ "check": "disk", "result": "failed" }],
				}))
				.await
				.assert_status_ok();
			assert!(
				fetch_issue(&mut conn, server_id, "seedling", "health/disk")
					.await
					.expect("seedling issue filed")
					.active
			);
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health/db")
					.await
					.expect("alertd issue still present")
					.active,
				"another source's push must not recover alertd's checks",
			);

			// The second source recovering only closes its own issue.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "source": "seedling", "health": [] }))
				.await
				.assert_status_ok();
			assert!(
				!fetch_issue(&mut conn, server_id, "seedling", "health/disk")
					.await
					.expect("seedling issue closed")
					.active
			);
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health/db")
					.await
					.expect("alertd issue survives")
					.active
			);

			// Each stored status row records the source that pushed it.
			#[derive(QueryableByName)]
			struct SourceRow {
				#[diesel(sql_type = sql_types::Text)]
				source: String,
			}
			let sources: Vec<SourceRow> = sql_query(
				"SELECT source FROM statuses WHERE server_id = $1 ORDER BY created_at, source",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.load(&mut conn)
			.await
			.expect("load status sources");
			let sources: Vec<&str> = sources.iter().map(|r| r.source.as_str()).collect();
			assert_eq!(sources, ["alertd", "seedling", "seedling"]);
		},
	)
	.await
}

/// The `source` field must be a non-empty, non-reserved string when present.
#[tokio::test(flavor = "multi_thread")]
async fn source_field_validation() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			for source in ["canopy", "Manual", "kubernetes", "Kubernetes", ""] {
				public
					.post(&format!("/status/{server_id}"))
					.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
					.json(&serde_json::json!({ "source": source, "health": [] }))
					.await
					.assert_status_bad_request();
			}

			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({ "source": 42, "health": [] }))
				.await
				.assert_status_bad_request();
		},
	)
	.await
}

/// Filings stamp the issue's check-state columns: the observed result,
/// the policy-effective result (which can diverge), and the check's
/// detail from the push.
#[tokio::test(flavor = "multi_thread")]
async fn filings_stamp_check_state_columns() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;
			// Ceiling warning (the default) grades failures down: observed
			// and effective diverge.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [{ "check": "db", "result": "failed", "latency_ms": 42 }],
				}))
				.await
				.assert_status_ok();

			#[derive(QueryableByName, Debug)]
			struct StateRow {
				#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
				check_name: Option<String>,
				#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
				observed_result: Option<String>,
				#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
				effective_result: Option<String>,
				#[diesel(sql_type = sql_types::Nullable<sql_types::Jsonb>)]
				detail: Option<serde_json::Value>,
			}
			let fetch = async |conn: &mut diesel_async::AsyncPgConnection| -> StateRow {
				sql_query(
					"SELECT check_name, observed_result, effective_result, detail \
					 FROM issues WHERE application_id = $1 AND ref = 'health/db'",
				)
				.bind::<sql_types::Uuid, _>(server_id)
				.get_result(conn)
				.await
				.expect("issue row")
			};

			let row = fetch(&mut conn).await;
			assert_eq!(row.check_name.as_deref(), Some("db"));
			assert_eq!(row.observed_result.as_deref(), Some("failed"));
			assert_eq!(
				row.effective_result.as_deref(),
				Some("warning"),
				"default warning ceiling grades the failure down",
			);
			assert_eq!(
				row.detail
					.as_ref()
					.and_then(|d| d.get("latency_ms"))
					.and_then(|v| v.as_i64()),
				Some(42),
			);

			// Recovery stamps the pass.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [{ "check": "db", "result": "passed" }],
				}))
				.await
				.assert_status_ok();
			let row = fetch(&mut conn).await;
			assert_eq!(row.observed_result.as_deref(), Some("passed"));
			assert_eq!(row.effective_result.as_deref(), Some("passed"));
		},
	)
	.await
}

/// Ingest keeps each source's current server-wide detail, so the live views
/// never have to search status history for it. A source's push replaces its
/// own row and leaves other sources' alone.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn push_records_the_source_s_current_detail() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = Uuid::new_v4();
			sql_query(
				"WITH m AS (INSERT INTO machines (id, device_id) VALUES ($1, $2) RETURNING id) INSERT INTO applications (id, host, type, machine_id) \
				 VALUES ($1, 'https://detail.example.com', 'tamanu-central', $1)",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
			.execute(&mut conn)
			.await
			.unwrap();

			// What a consumer sees: the application's own detail merged with
			// its machine's. Detail splits by grain on the way in, so
			// `bestoolVersion` lands on the box while `pgVersion` stays with
			// the workload; a reader is not meant to care which.
			let detail = async |conn: &mut diesel_async::AsyncPgConnection, source: &str| {
				sql_query(
					"SELECT COALESCE(m.extra, '{}'::jsonb) || COALESCE(d.extra, '{}'::jsonb) \
					   AS extra \
					 FROM applications a \
					 LEFT JOIN application_reported_detail d \
					   ON d.application_id = a.id AND d.source = $2 \
					 LEFT JOIN machine_reported_detail m \
					   ON m.machine_id = a.machine_id AND m.source = $2 \
					 WHERE a.id = $1 AND (d.source IS NOT NULL OR m.source IS NOT NULL)",
				)
				.bind::<sql_types::Uuid, _>(server_id)
				.bind::<sql_types::Text, _>(source.to_owned())
				.get_result::<ExtraOnly>(conn)
				.await
				.ok()
				.map(|row| row.extra)
			};

			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"health": [],
					"bestoolVersion": "2.10.5",
					"pgVersion": "PostgreSQL 16.3 on x86_64-pc-linux-gnu",
				}))
				.await
				.assert_status_ok();

			let recorded = detail(&mut conn, "alertd").await.expect("alertd recorded");
			assert_eq!(recorded["bestoolVersion"], "2.10.5");

			// A second source's push doesn't disturb the first's.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "tamanu",
					"health": [],
					"uptimeSecs": 42,
				}))
				.await
				.assert_status_ok();
			assert_eq!(
				detail(&mut conn, "alertd")
					.await
					.expect("alertd still there")["bestoolVersion"],
				"2.10.5",
			);
			assert_eq!(
				detail(&mut conn, "tamanu").await.expect("tamanu recorded")["uptimeSecs"],
				42,
			);

			// alertd pushes again without pgVersion: the push is its whole
			// truth, so the dropped field goes with it.
			public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"health": [],
					"bestoolVersion": "2.11.0",
				}))
				.await
				.assert_status_ok();
			let recorded = detail(&mut conn, "alertd")
				.await
				.expect("alertd re-recorded");
			assert_eq!(recorded["bestoolVersion"], "2.11.0");
			assert!(
				recorded.get("pgVersion").is_none(),
				"a field the source stopped reporting is no longer its current detail",
			);
		},
	)
	.await
}

/// Validation permits two `health[]` entries with the same `check` name.
/// The result came from a map (last wins) while the message, extras, and
/// severity-rule context came from a linear search (first wins), so the
/// filed issue mixed one entry's verdict with another entry's data.
#[tokio::test(flavor = "multi_thread")]
async fn duplicate_check_names_file_one_consistent_entry() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"health": [
						{ "check": "disk", "result": "passed", "free_pct": 80 },
						{ "check": "disk", "result": "failed", "free_pct": 2 },
					],
				}))
				.await
				.assert_status_ok();

			let issue = fetch_issue(&mut conn, server_id, "alertd", "health/disk")
				.await
				.expect("the check filed an issue");

			// The map keeps the last entry's result, so the message and extras
			// have to come from that same entry.
			assert_eq!(
				issue.observed_result.as_deref(),
				Some("failed"),
				"the later entry supersedes the earlier one",
			);
			assert!(
				issue.message.contains("`2`"),
				"the detail must come from the entry the result came from, got: {}",
				issue.message,
			);
			assert!(
				!issue.message.contains("`80`"),
				"the superseded entry's detail must not be reported: {}",
				issue.message,
			);
		},
	)
	.await
}

/// A unified push carries both grains' checks. The machine-subject ones file
/// against the box, the rest against the workload, from one payload.
// spec: STA
#[tokio::test(flavor = "multi_thread")]
async fn a_unified_push_files_each_check_at_its_own_grain() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			let response = public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"healthy": true,
					"health": [
						// The box's: disk and clock.
						{ "check": "disk_free", "result": "failed", "free_pct": 2 },
						{ "check": "time_sync", "result": "warning" },
						// The workload's: its database, and an error stream
						// that merely shares a prefix with the machine's `ips`.
						{ "check": "postgres", "result": "failed" },
						{ "check": "ips_errors", "result": "warning" },
					],
				}))
				.await;
			response.assert_status_ok();

			#[derive(diesel::QueryableByName)]
			struct Row {
				#[diesel(sql_type = sql_types::Text)]
				r#ref: String,
			}
			let on_machine: Vec<String> = sql_query(
				"SELECT i.ref FROM issues i JOIN applications a ON a.machine_id = i.machine_id \
				 WHERE a.id = $1 ORDER BY i.ref",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.load::<Row>(&mut conn)
			.await
			.expect("machine issues")
			.into_iter()
			.map(|r| r.r#ref)
			.collect();
			assert_eq!(
				on_machine,
				vec!["health/disk_free", "health/time_sync"],
				"the box's checks file against the box"
			);

			let on_application: Vec<String> =
				sql_query("SELECT ref FROM issues WHERE application_id = $1 ORDER BY ref")
					.bind::<sql_types::Uuid, _>(server_id)
					.load::<Row>(&mut conn)
					.await
					.expect("application issues")
					.into_iter()
					.map(|r| r.r#ref)
					.collect();
			assert_eq!(
				on_application,
				vec!["health/ips_errors", "health/postgres"],
				"the workload's stay with the workload; ips_errors is not ips"
			);
		},
	)
	.await
}

/// A machine check keeps the source that reported it. Recording it under
/// `canopy` would break per-source silences, the severities a push answers
/// with, and the rule that a source's push only recovers its own checks.
// spec: STA
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_check_keeps_its_reporters_source() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			public
				.post(&format!("/status/{}", server_id))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "memory", "result": "failed" }],
				}))
				.await
				.assert_status_ok();

			#[derive(diesel::QueryableByName)]
			struct Row {
				#[diesel(sql_type = sql_types::Text)]
				source: String,
			}
			let sources: Vec<String> = sql_query(
				"SELECT i.source FROM issues i JOIN applications a ON a.machine_id = i.machine_id \
				 WHERE a.id = $1 AND i.ref = 'health/memory'",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.load::<Row>(&mut conn)
			.await
			.expect("machine issue")
			.into_iter()
			.map(|r| r.source)
			.collect();
			assert_eq!(
				sources,
				vec!["alertd"],
				"the reporter's source, not canopy's"
			);
		},
	)
	.await
}

/// The three push shapes are told apart by their health field, and an empty set
/// of checks is not the absence of one: a source with nothing to report is
/// still that source describing the target, and recovers what it last reported
/// rather than being re-attributed to Tamanu.
// spec: STA#transitional-unified-pushes
#[tokio::test(flavor = "multi_thread")]
async fn an_empty_health_set_is_the_source_reporting_nothing_not_a_legacy_push() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// alertd reports a failing check, then reports nothing at all.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "health": [ { "check": "disk", "result": "failed" } ] }),
			)
			.await;
			assert!(
				fetch_issue(&mut conn, server_id, "alertd", "health/disk")
					.await
					.expect("per-check issue filed")
					.active
			);

			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "health": [], "uptime": 7 }),
			)
			.await;

			let row = fetch_latest_health(&mut conn, server_id).await;
			assert_eq!(
				row.health,
				serde_json::json!([]),
				"an empty set stays empty rather than becoming the Tamanu heartbeat",
			);
			assert_eq!(row.extra.get("uptime").and_then(|v| v.as_i64()), Some(7));
			assert!(
				!fetch_issue(&mut conn, server_id, "alertd", "health/disk")
					.await
					.expect("the check state survives")
					.active,
				"reporting no checks recovers the ones the source last reported",
			);
			assert!(
				fetch_issue(&mut conn, server_id, "tamanu", "health/tasks")
					.await
					.is_none(),
				"and no heartbeat is synthesised under the tamanu source",
			);

			// Omitting the field entirely is the legacy shape, and does become
			// the heartbeat.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({ "uptime": 8 }),
			)
			.await;
			assert!(
				fetch_issue(&mut conn, server_id, "tamanu", "health/tasks")
					.await
					.is_some(),
			);
		},
	)
	.await
}

/// A reporter names its checks bare, so the same name arrives from workloads of
/// different products meaning different things. Ingestion catalogues each under
/// the namespace of the type that reported it, so an operator grades the two
/// separately.
// spec: STA#health-and-detail, CHK
#[tokio::test(flavor = "multi_thread")]
async fn a_bare_check_name_is_catalogued_under_the_reporting_types_namespace() {
	commons_tests::server::run_with_device_auth(
		"admin",
		async |mut conn, cert, device_id, public, _| {
			let central = insert_health_test_server(&mut conn, device_id).await;

			// Two workloads of different products on the same box, each
			// reporting the same bare check name. A push is addressed to the
			// box and names which of its workloads it speaks for, so the
			// second one comes into being by reporting.
			for kind in ["central", "facility"] {
				post_status(
					&public,
					&cert,
					&mut conn,
					central,
					serde_json::json!({
						"health": [ { "check": "disk", "result": "warning" } ],
						"tamanuServerKind": kind,
					}),
				)
				.await;
			}

			#[derive(QueryableByName)]
			struct CatalogRow {
				#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
				subject: Option<String>,
				#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
				application_type: Option<String>,
			}
			let rows: Vec<CatalogRow> = sql_query(
				"SELECT subject, application_type FROM check_policies \
				 WHERE source = 'alertd' AND check_name = 'disk' \
				 ORDER BY application_type",
			)
			.load(&mut conn)
			.await
			.expect("catalog rows");

			let namespaces: Vec<_> = rows
				.iter()
				.map(|r| (r.subject.as_deref(), r.application_type.as_deref()))
				.collect();
			assert_eq!(
				namespaces,
				vec![
					(Some("application"), Some("tamanu-central")),
					(Some("application"), Some("tamanu-facility")),
				],
				"one bare name, one catalog row per reporting type",
			);
		},
	)
	.await
}

/// The transition is not a cutover: bestools already in the field push the
/// unified shape, with no `machine` section and detail spread flat across the
/// envelope. Canopy separates that push itself, so the box's checks and fields
/// reach the machine and the workload's reach the application.
// spec: STA#transitional-unified-pushes
#[tokio::test(flavor = "multi_thread")]
async fn a_unified_push_from_a_fielded_agent_still_files_its_checks() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_health_test_server(&mut conn, device_id).await;

			// The shape a bestool in the field sends today: one flat set of
			// checks mixing machine-subject (disk_free) and application-subject
			// (database) material, and detail fields on the envelope.
			post_status(
				&public,
				&cert,
				&mut conn,
				server_id,
				serde_json::json!({
					"source": "alertd",
					"healthy": false,
					"health": [
						{ "check": "database", "result": "passed" },
						{ "check": "disk_free", "result": "warning", "free_pct": 4 },
					],
					"uptimeSecs": 4321,
					"timezone": "Pacific/Auckland",
				}),
			)
			.await;

			let database = fetch_issue(&mut conn, server_id, "alertd", "health/database")
				.await
				.expect("the workload's check files against the application");
			assert!(!database.active, "a passing check is not an open issue");

			let disk: IssueRow = sql_query(
				r#"
				SELECT escalates, active, message, description,
					(resolved_at IS NOT NULL) AS is_resolved,
					observed_result, effective_result
				FROM issues
				WHERE machine_id = $1 AND application_id IS NULL
				  AND source = 'alertd' AND ref = 'health/disk_free'
			"#,
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("the box's check files against the machine");
			assert!(disk.active);
			assert_eq!(disk.observed_result.as_deref(), Some("warning"));

			// Detail separates on the same axis, without the agent knowing.
			let machine: ExtraOnly = sql_query(
				"SELECT extra FROM machine_reported_detail \
				 WHERE machine_id = $1 AND source = 'alertd'",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("machine detail recorded");
			assert_eq!(machine.extra["uptimeSecs"], 4321);
			assert!(machine.extra.get("timezone").is_none());

			let application: ExtraOnly = sql_query(
				"SELECT extra FROM application_reported_detail \
				 WHERE application_id = $1 AND source = 'alertd'",
			)
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(&mut conn)
			.await
			.expect("application detail recorded");
			assert_eq!(application.extra["timezone"], "Pacific/Auckland");
			assert!(application.extra.get("uptimeSecs").is_none());

			// The push is still recorded whole as history.
			let row = fetch_latest_health(&mut conn, server_id).await;
			assert_eq!(row.health.as_array().expect("array").len(), 2);
			assert_eq!(row.extra["uptimeSecs"], 4321);
		},
	)
	.await
}

// -----------------------------------------------------------------
// Which application a unified push is about.
//
// The id on the wire is the machine's, so Canopy has to work out which
// workload on that box the push describes. It correlates on the type the
// push names, falls back to the box's one application where it names none,
// and refuses rather than guessing when neither answers.
// spec: STA#transitional-unified-pushes, FLT#applications-come-from-reports
// -----------------------------------------------------------------

/// A bare box bound to the authenticated identity, carrying no applications.
async fn insert_bare_machine(conn: &mut diesel_async::AsyncPgConnection, device_id: Uuid) -> Uuid {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'bare-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	let machine_id = Uuid::new_v4();
	sql_query("INSERT INTO machines (id, group_id, device_id) VALUES ($1, $2, $3)")
		.bind::<sql_types::Uuid, _>(machine_id)
		.bind::<sql_types::Uuid, _>(group_id)
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(device_id))
		.execute(conn)
		.await
		.expect("insert machine");
	machine_id
}

#[derive(QueryableByName)]
struct TypeRow {
	#[diesel(sql_type = sql_types::Text)]
	#[diesel(column_name = "type")]
	r#type: String,
}

async fn application_types_on(
	conn: &mut diesel_async::AsyncPgConnection,
	machine_id: Uuid,
) -> Vec<String> {
	let rows: Vec<TypeRow> = sql_query(
		"SELECT type FROM applications WHERE machine_id = $1 AND deleted_at IS NULL ORDER BY type",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.load(conn)
	.await
	.expect("load applications");
	rows.into_iter().map(|r| r.r#type).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn push_naming_a_type_creates_the_application_it_describes() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = insert_bare_machine(&mut conn, device_id).await;
			assert!(application_types_on(&mut conn, machine_id).await.is_empty());

			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "db", "result": "passed" }],
					"tamanuServerKind": "facility",
				}))
				.await
				.assert_status_ok();

			assert_eq!(
				application_types_on(&mut conn, machine_id).await,
				vec!["tamanu-facility".to_string()],
				"a report is the only thing that creates an application"
			);

			// A second workload reporting from the same box is a second
			// application, not a correction of the first.
			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "db", "result": "passed" }],
					"tamanuServerKind": "central",
				}))
				.await
				.assert_status_ok();

			assert_eq!(
				application_types_on(&mut conn, machine_id).await,
				vec!["tamanu-central".to_string(), "tamanu-facility".to_string()]
			);

			// And reporting again as one of them adopts it rather than
			// minting a third.
			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "db", "result": "passed" }],
					"tamanuServerKind": "facility",
				}))
				.await
				.assert_status_ok();

			assert_eq!(application_types_on(&mut conn, machine_id).await.len(), 2);
		},
	)
	.await
}

/// The fleet holds boxes that run nothing Canopy models, and their agents push
/// the same unified shape as everyone else. There is no second grain for such a
/// push to split into, so it is the box's in full.
// spec: STA#transitional-unified-pushes
#[tokio::test(flavor = "multi_thread")]
async fn a_push_from_a_bare_box_is_the_machines_in_full() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = insert_bare_machine(&mut conn, device_id).await;

			// Nothing in the push says what it is, and the box runs nothing
			// Canopy could attribute it to.
			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [
						{ "check": "db", "result": "warning" },
						{ "check": "disk_free", "result": "passed" },
					],
					"uptimeSecs": 4321,
					"timezone": "Pacific/Auckland",
				}))
				.await
				.assert_status_ok();

			assert!(
				application_types_on(&mut conn, machine_id).await.is_empty(),
				"a box with no workload gets no invented one"
			);

			// Both checks file at machine scope, including the one whose name
			// is an application's anywhere else: there is no application here
			// for it to belong to.
			for check in ["db", "disk_free"] {
				let issue: IssueRow = sql_query(
					r#"
					SELECT escalates, active, message, description,
						(resolved_at IS NOT NULL) AS is_resolved,
						observed_result, effective_result
					FROM issues
					WHERE machine_id = $1 AND application_id IS NULL
					  AND source = 'alertd' AND ref = $2
				"#,
				)
				.bind::<sql_types::Uuid, _>(machine_id)
				.bind::<sql_types::Text, _>(format!("health/{check}"))
				.get_result(&mut conn)
				.await
				.unwrap_or_else(|e| panic!("{check} files against the machine: {e}"));
				assert_eq!(issue.active, check == "db");
			}

			// Detail has nowhere else to go either, so all of it is the box's.
			let machine: ExtraOnly = sql_query(
				"SELECT extra FROM machine_reported_detail \
				 WHERE machine_id = $1 AND source = 'alertd'",
			)
			.bind::<sql_types::Uuid, _>(machine_id)
			.get_result(&mut conn)
			.await
			.expect("machine detail recorded");
			assert_eq!(machine.extra["uptimeSecs"], 4321);
			assert_eq!(machine.extra["timezone"], "Pacific/Auckland");

			assert!(
				sql_query("SELECT extra FROM application_reported_detail")
					.get_result::<ExtraOnly>(&mut conn)
					.await
					.is_err(),
				"no application, so no application detail"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn push_naming_no_type_against_a_two_workload_box_is_refused() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			let machine_id = insert_bare_machine(&mut conn, device_id).await;
			sql_query(
				"INSERT INTO applications (id, type, machine_id) \
				 VALUES (gen_random_uuid(), 'tamanu-central', $1), \
				        (gen_random_uuid(), 'tamanu-facility', $1)",
			)
			.bind::<sql_types::Uuid, _>(machine_id)
			.execute(&mut conn)
			.await
			.expect("insert two applications");

			// Attributing the box's whole picture to an arbitrary one of its
			// workloads is the failure this card exists to stop.
			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "db", "result": "passed" }],
				}))
				.await
				.assert_status_conflict();

			assert_eq!(application_types_on(&mut conn, machine_id).await.len(), 2);

			// Naming which one it is resolves it.
			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "db", "result": "passed" }],
					"tamanuServerKind": "facility",
				}))
				.await
				.assert_status_ok();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ignored_source_push_creates_no_application() {
	commons_tests::server::run_with_device_auth(
		"machine",
		async |mut conn, cert, device_id, public, _| {
			use commons_types::source::IngestMode;
			use database::source_policies::SourcePolicy;

			let machine_id = insert_bare_machine(&mut conn, device_id).await;
			sql_query(
				"INSERT INTO applications (id, type, machine_id) \
				 VALUES (gen_random_uuid(), 'tamanu-central', $1)",
			)
			.bind::<sql_types::Uuid, _>(machine_id)
			.execute(&mut conn)
			.await
			.expect("insert application");

			SourcePolicy::set_ingest(&mut conn, "alertd", IngestMode::Ignore)
				.await
				.expect("set ignore");

			// An ignored source records nowhere, so it reads without creating:
			// a type it names that the box does not run resolves to the one
			// application there rather than minting a second.
			public
				.post(&format!("/status/{machine_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"source": "alertd",
					"healthy": true,
					"health": [{ "check": "db", "result": "passed" }],
					"tamanuServerKind": "facility",
				}))
				.await
				.assert_status_ok();

			assert_eq!(
				application_types_on(&mut conn, machine_id).await,
				vec!["tamanu-central".to_string()]
			);
		},
	)
	.await
}
