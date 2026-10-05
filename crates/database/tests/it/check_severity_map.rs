//! Queries backing the device-facing effective check map:
//! `CheckPolicy::ceiling_map_for_source` (static policy ceilings,
//! ignoring conditional rules) and
//! `silenced_refs::silenced_health_checks_at` (server- plus
//! group-scope silences under one reporting source).

/// The type every application in this file has.
///
/// Load-bearing here, unlike in most catalog tests: the ceiling map and the
/// silence set are both resolved through the reporting application's type, so
/// the seeded rows and the lookups have to name the same one.
fn ty() -> ApplicationType {
	ApplicationType::TamanuFacility
}

fn ns() -> Namespace {
	Namespace::Application(ty())
}
use commons_types::{namespace::Namespace, server::app_type::ApplicationType, status::CheckResult};
use database::check_policies::{CheckPolicy, IfLadder};
use database::issues::Scope;
use database::silenced_refs::{
	MachineSilencedRef, ServerGroupSilencedRef, ServerSilencedRef, silenced_health_checks_at,
};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use serde_json::json;
use uuid::Uuid;

async fn insert_group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'severity-map-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	group_id
}

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection, group_id: Option<Uuid>) -> Uuid {
	let server_id = Uuid::new_v4();
	// The machine takes the application's own id, as the split's backfill did.
	sql_query("INSERT INTO machines (name, id, group_id) VALUES ('box', $1, $2)")
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
		.execute(conn)
		.await
		.expect("insert machine");
	sql_query(
		"INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ($1, 'https://severity-map.example.com', $4, $2, $3)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(ty().to_string())
	.execute(conn)
	.await
	.expect("insert server");
	server_id
}

#[tokio::test(flavor = "multi_thread")]
async fn ceiling_map_returns_static_ceilings_for_one_source() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for check in ["disk_space", "cert_expiry", "chatty"] {
			CheckPolicy::upsert_default(&mut conn, "alertd", &ns(), check)
				.await
				.expect("seed");
		}
		CheckPolicy::upsert_default(&mut conn, "seedling", &ns(), "other_source_check")
			.await
			.expect("seed other source");
		CheckPolicy::update(
			&mut conn,
			"alertd",
			&ns(),
			"disk_space",
			CheckResult::Failed,
			false,
			None,
			"alice",
		)
		.await
		.expect("update disk_space");
		CheckPolicy::update(
			&mut conn,
			"alertd",
			&ns(),
			"chatty",
			CheckResult::Passed,
			false,
			None,
			"alice",
		)
		.await
		.expect("update chatty");

		let map = CheckPolicy::ceiling_map_for_source(&mut conn, "alertd", &ns())
			.await
			.expect("map");
		assert_eq!(map.len(), 3, "only the requested source's checks");
		assert_eq!(map.get("disk_space"), Some(&CheckResult::Failed));
		assert_eq!(map.get("cert_expiry"), Some(&CheckResult::Warning));
		assert_eq!(map.get("chatty"), Some(&CheckResult::Passed));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ceiling_map_ignores_conditional_rules() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		CheckPolicy::upsert_default(&mut conn, "alertd", &ns(), "ruled")
			.await
			.expect("seed");
		let ladder: IfLadder = serde_json::from_value(json!({"if": [
			{"==": [{"var": "check.result"}, "failed"]}, "failed",
		]}))
		.expect("parse ladder");
		CheckPolicy::update_rules(&mut conn, "alertd", &ns(), "ruled", Some(&ladder), "alice")
			.await
			.expect("set rules");

		// The expression could grade a failure through at push time, but
		// the static map must only reflect the ceiling column.
		let map = CheckPolicy::ceiling_map_for_source(&mut conn, "alertd", &ns())
			.await
			.expect("map");
		assert_eq!(map.get("ruled"), Some(&CheckResult::Warning));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn silenced_checks_combine_scopes_and_stay_per_source() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		let server_id = insert_server(&mut conn, Some(group_id)).await;
		let other_server_id = insert_server(&mut conn, None).await;

		ServerSilencedRef::add(&mut conn, server_id, "alertd", "health/flaky", None, None)
			.await
			.expect("server silence");
		ServerGroupSilencedRef::add(
			&mut conn,
			group_id,
			"alertd",
			"health/groupwide",
			Some(&ty()),
			None,
			None,
		)
		.await
		.expect("group silence");
		// None of these may leak into alertd's set: a check's identity is
		// the (source, check) pair, so another source's silence never
		// applies; nor do canopy's own silences or other applications'.
		ServerSilencedRef::add(
			&mut conn,
			server_id,
			"seedling",
			"health/other-source",
			None,
			None,
		)
		.await
		.expect("other-source silence");
		ServerSilencedRef::add(&mut conn, server_id, "canopy", "reachability", None, None)
			.await
			.expect("canopy silence");
		ServerSilencedRef::add(
			&mut conn,
			other_server_id,
			"alertd",
			"health/other",
			None,
			None,
		)
		.await
		.expect("other-server silence");

		let checks = silenced_health_checks_at(
			&mut conn,
			Scope::Application(server_id),
			Some(group_id),
			"alertd",
		)
		.await
		.expect("checks");
		assert_eq!(
			checks.into_iter().collect::<Vec<_>>(),
			vec!["flaky", "groupwide"]
		);

		// Ungrouped lookup only sees the server-scope silences.
		let checks =
			silenced_health_checks_at(&mut conn, Scope::Application(server_id), None, "alertd")
				.await
				.expect("checks without group");
		assert_eq!(checks.into_iter().collect::<Vec<_>>(), vec!["flaky"]);
	})
	.await
}

/// A check reported under an application is that application's, whatever it
/// is called, so the box's `memory` and an application's own `memory` are two
/// entries: each target's reporter is told its own ceiling and its own
/// silences, never the other's.
// spec: CHK#names
#[tokio::test(flavor = "multi_thread")]
async fn a_box_check_and_an_applications_namesake_are_told_apart() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn).await;
		// The machine takes the application's id (see `insert_server`).
		let server_id = insert_server(&mut conn, Some(group_id)).await;
		let machine_id = server_id;

		for (namespace, ceiling) in [
			(Namespace::Machine, CheckResult::Failed),
			(ns(), CheckResult::Passed),
		] {
			CheckPolicy::upsert_default(&mut conn, "alertd", &namespace, "memory")
				.await
				.expect("seed");
			CheckPolicy::update(
				&mut conn, "alertd", &namespace, "memory", ceiling, false, None, "alice",
			)
			.await
			.expect("set ceiling");
		}

		let machine_map =
			CheckPolicy::ceiling_map_for_source(&mut conn, "alertd", &Namespace::Machine)
				.await
				.expect("machine map");
		let own_map = CheckPolicy::ceiling_map_for_source(&mut conn, "alertd", &ns())
			.await
			.expect("application map");
		assert_eq!(machine_map.get("memory"), Some(&CheckResult::Failed));
		assert_eq!(own_map.get("memory"), Some(&CheckResult::Passed));

		MachineSilencedRef::add(&mut conn, machine_id, "alertd", "health/memory", None, None)
			.await
			.expect("machine silence");
		let at_machine = silenced_health_checks_at(
			&mut conn,
			Scope::Machine(machine_id),
			Some(group_id),
			"alertd",
		)
		.await
		.expect("machine silences");
		let at_application = silenced_health_checks_at(
			&mut conn,
			Scope::Application(server_id),
			Some(group_id),
			"alertd",
		)
		.await
		.expect("application silences");
		assert!(at_machine.contains("memory"));
		assert!(
			!at_application.contains("memory"),
			"the box's silence is not the application's: {at_application:?}"
		);
	})
	.await
}
