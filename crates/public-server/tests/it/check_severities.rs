//! Device-facing effective check-severity mapping: the dedicated
//! `GET /status/{server_id}/check-severities` endpoint and the
//! `check_severities` field riding along status-push responses.

use commons_types::{namespace::Namespace, server::app_type::ApplicationType};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

use database::silenced_refs::{ServerGroupSilencedRef, ServerSilencedRef};

async fn insert_group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'check-severities-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	group_id
}

async fn insert_server(
	conn: &mut diesel_async::AsyncPgConnection,
	device_id: Option<Uuid>,
	group_id: Option<Uuid>,
) -> Uuid {
	let server_id = Uuid::new_v4();
	sql_query("INSERT INTO machines (id, group_id, device_id) VALUES ($1, $2, $3)")
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(device_id)
		.execute(conn)
		.await
		.expect("insert machine");
	sql_query(
		"INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ($1, 'https://checks.example.com', 'tamanu-facility', $2, $1)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
	.execute(conn)
	.await
	.expect("insert server");
	server_id
}

/// Seed a policy row at a given ceiling, optionally with a conditional
/// rules ladder (which the mapping must ignore).
async fn seed_catalog(
	conn: &mut diesel_async::AsyncPgConnection,
	check_name: &str,
	ceiling: &str,
	rules: Option<serde_json::Value>,
) {
	// The servers here are facilities, so that is the namespace their pushes
	// will read these ceilings back out of.
	let (subject, application_type) =
		Namespace::for_application("alertd", check_name, &ApplicationType::TamanuFacility)
			.to_columns();
	sql_query(
		"INSERT INTO check_policies \
		 (source, subject, application_type, check_name, ceiling, rules, reviewed_at, reviewed_by) \
		 VALUES ('alertd', $1, $2, $3, $4, $5, NOW(), 'test') \
		 ON CONFLICT (source, subject, application_type, check_name) DO UPDATE \
		 SET ceiling = EXCLUDED.ceiling, rules = EXCLUDED.rules",
	)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(subject)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(application_type)
	.bind::<sql_types::Text, _>(check_name)
	.bind::<sql_types::Text, _>(ceiling)
	.bind::<sql_types::Nullable<sql_types::Jsonb>, _>(rules)
	.execute(conn)
	.await
	.expect("seed catalog row");
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_maps_catalog_severities_and_silences() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = insert_group(&mut conn).await;
			let server_id = insert_server(&mut conn, Some(device_id), Some(group_id)).await;

			seed_catalog(&mut conn, "disk_space", "failed", None).await;
			seed_catalog(&mut conn, "cert_expiry", "warning", None).await;
			seed_catalog(&mut conn, "chatty", "passed", None).await;
			seed_catalog(&mut conn, "verbose", "skipped", None).await;
			seed_catalog(&mut conn, "flaky", "failed", None).await;
			seed_catalog(&mut conn, "groupwide", "failed", None).await;
			// A conditional ladder that would grade up to failed at push
			// time must not leak into the static mapping.
			seed_catalog(
				&mut conn,
				"ruled",
				"warning",
				Some(serde_json::json!({"if": [
					{"==": [{"var": "check.result"}, "failed"]}, "failed",
				]})),
			)
			.await;

			// Silences: flaky at server scope, groupwide at group scope; a
			// silence on a canopy check (a reserved source, outside the
			// health/ namespace) must not leak into alertd's map.
			ServerSilencedRef::add(&mut conn, server_id, "alertd", "health/flaky", None)
				.await
				.expect("server silence");
			ServerGroupSilencedRef::add(
				&mut conn,
				group_id,
				"alertd",
				"health/groupwide",
				Some(&ApplicationType::TamanuFacility),
				None,
			)
			.await
			.expect("group silence");
			ServerSilencedRef::add(&mut conn, server_id, "canopy", "reachability", None)
				.await
				.expect("canopy silence");

			let response = public
				.get(&format!("/status/{server_id}/check-severities"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let map: serde_json::Value = response.json();
			assert_eq!(
				map,
				serde_json::json!({
					"disk_space": "fail",
					"cert_expiry": "warn",
					"chatty": "skip",
					"verbose": "skip",
					"flaky": "skip",
					"groupwide": "skip",
					"ruled": "warn",
				}),
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_works_for_ungrouped_server() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let server_id = insert_server(&mut conn, Some(device_id), None).await;
			seed_catalog(&mut conn, "disk_space", "failed", None).await;
			ServerSilencedRef::add(&mut conn, server_id, "alertd", "health/disk_space", None)
				.await
				.expect("silence");

			let response = public
				.get(&format!("/status/{server_id}/check-severities"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_ok();
			let map: serde_json::Value = response.json();
			assert_eq!(map, serde_json::json!({"disk_space": "skip"}));
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn status_response_carries_check_severities() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let group_id = insert_group(&mut conn).await;
			let server_id = insert_server(&mut conn, Some(device_id), Some(group_id)).await;

			seed_catalog(&mut conn, "disk_space", "failed", None).await;
			seed_catalog(&mut conn, "cert_expiry", "failed", None).await;
			ServerSilencedRef::add(&mut conn, server_id, "alertd", "health/cert_expiry", None)
				.await
				.expect("silence");

			let response = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"health": [
					{"check": "disk_space", "result": "passed"},
					{"check": "cert_expiry", "result": "failed"},
					{"check": "brand_new", "result": "passed"},
				]}))
				.await;
			response.assert_status_ok();
			let body: serde_json::Value = response.json();
			// The map reflects the catalog and silences, including the check
			// first seen on this very push (upserted at default Warning).
			assert_eq!(
				body.get("check_severities"),
				Some(&serde_json::json!({
					"disk_space": "fail",
					"cert_expiry": "skip",
					"brand_new": "warn",
				})),
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_rejects_device_not_bound_to_server() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, _device_id, public, _| {
			// Application bound to no device: the caller's device doesn't match.
			let server_id = insert_server(&mut conn, None, None).await;

			let response = public
				.get(&format!("/status/{server_id}/check-severities"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_forbidden();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn push_rejects_device_not_bound_to_server() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, _device_id, public, _| {
			let server_id = insert_server(&mut conn, None, None).await;

			let response = public
				.post(&format!("/status/{server_id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"health": []}))
				.await;
			response.assert_status_forbidden();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_404s_for_unknown_server() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |_conn, cert, _device_id, public, _| {
			let response = public
				.get(&format!("/status/{}/check-severities", Uuid::new_v4()))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			response.assert_status_not_found();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn endpoint_requires_device_auth() {
	commons_tests::server::run(async |mut conn, public, _| {
		let server_id = insert_server(&mut conn, None, None).await;
		let response = public
			.get(&format!("/status/{server_id}/check-severities"))
			.await;
		response.assert_status_not_ok();
	})
	.await
}
