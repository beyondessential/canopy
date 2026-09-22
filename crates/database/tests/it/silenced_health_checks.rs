//! `silenced_refs::silenced_health_checks_for_server`: resolving the set
//! of healthcheck names silenced for a server under one reporting
//! source, at server and group scope. This set feeds the consolidated
//! check readers so silenced checks don't count toward the health
//! rollup — scoped to the status row's own source, since a check's
//! identity is the (source, check) pair.

use std::collections::BTreeSet;

use commons_tests::db::TestDb;
use commons_types::server::app_type::ApplicationType;
use database::silenced_refs::{
	ServerGroupSilencedRef, ServerSilencedRef, silenced_health_checks_for_server,
};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

async fn insert_group(conn: &mut database::diesel_async::AsyncPgConnection, name: &str) -> Uuid {
	#[derive(diesel::QueryableByName)]
	struct RowId {
		#[diesel(sql_type = sql_types::Uuid)]
		id: Uuid,
	}
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.get_result(conn)
		.await
		.expect("insert group");
	row.id
}

async fn insert_server(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Option<Uuid>,
) -> Uuid {
	#[derive(diesel::QueryableByName)]
	struct RowId {
		#[diesel(sql_type = sql_types::Uuid)]
		id: Uuid,
	}
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	let row: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (group_id) VALUES ($2) RETURNING id) INSERT INTO applications (host, type, group_id, machine_id) SELECT $1, 'tamanu-central', $2, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

fn checks(names: &[&str]) -> BTreeSet<String> {
	names.iter().map(|s| s.to_string()).collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn combines_server_and_group_scopes() {
	TestDb::run(async |mut conn, _url| {
		let group = insert_group(&mut conn, "g").await;
		let grouped = insert_server(&mut conn, Some(group)).await;
		let m_grouped = machine_of(&mut conn, grouped).await;
		let ungrouped = insert_server(&mut conn, None).await;
		let m_ungrouped = machine_of(&mut conn, ungrouped).await;
		let unsilenced = insert_server(&mut conn, Some(group)).await;
		let m_unsilenced = machine_of(&mut conn, unsilenced).await;

		ServerSilencedRef::add(&mut conn, grouped, "alertd", "health/postgres", None)
			.await
			.unwrap();
		ServerSilencedRef::add(&mut conn, ungrouped, "alertd", "health/disk", None)
			.await
			.unwrap();
		ServerGroupSilencedRef::add(
			&mut conn,
			group,
			"alertd",
			"health/uploads",
			Some(&ApplicationType::TamanuCentral),
			None,
		)
		.await
		.unwrap();

		// Application scope and group scope combine for the grouped server.
		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(grouped),
				Some(m_grouped),
				Some(group),
				"alertd"
			)
			.await
			.unwrap(),
			checks(&["postgres", "uploads"]),
		);
		// The ungrouped server only sees its own silences.
		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(ungrouped),
				Some(m_ungrouped),
				None,
				"alertd"
			)
			.await
			.unwrap(),
			checks(&["disk"]),
		);
		// A group member with no server-scope silence still inherits the
		// group's.
		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(unsilenced),
				Some(m_unsilenced),
				Some(group),
				"alertd"
			)
			.await
			.unwrap(),
			checks(&["uploads"]),
		);
	})
	.await
}

/// A check's identity is the (source, check) pair: another source's
/// silence on a same-named check never applies, and neither do canopy's
/// own or manual silences.
#[tokio::test(flavor = "multi_thread")]
async fn scoped_to_the_reporting_source() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, None).await;
		let m_server = machine_of(&mut conn, server).await;

		ServerSilencedRef::add(&mut conn, server, "canopy", "reachability", None)
			.await
			.unwrap();
		ServerSilencedRef::add(&mut conn, server, "seedling", "health/postgres", None)
			.await
			.unwrap();
		ServerSilencedRef::add(&mut conn, server, "alertd", "health/disk", None)
			.await
			.unwrap();

		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(server),
				Some(m_server),
				None,
				"alertd"
			)
			.await
			.unwrap(),
			checks(&["disk"]),
			"only alertd's own silence applies to alertd's checks",
		);
		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(server),
				Some(m_server),
				None,
				"seedling"
			)
			.await
			.unwrap(),
			checks(&["postgres"]),
		);
	})
	.await
}

/// Removing a silence takes the check back out of the set.
#[tokio::test(flavor = "multi_thread")]
async fn unsilencing_removes_the_check() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, None).await;
		let m_server = machine_of(&mut conn, server).await;

		ServerSilencedRef::add(&mut conn, server, "alertd", "health/postgres", None)
			.await
			.unwrap();
		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(server),
				Some(m_server),
				None,
				"alertd"
			)
			.await
			.unwrap(),
			checks(&["postgres"]),
		);

		ServerSilencedRef::remove(&mut conn, server, "alertd", "health/postgres")
			.await
			.unwrap();
		assert_eq!(
			silenced_health_checks_for_server(
				&mut conn,
				Some(server),
				Some(m_server),
				None,
				"alertd"
			)
			.await
			.unwrap(),
			BTreeSet::new(),
		);
	})
	.await
}

/// The machine an application sits on. These tests exercise application- and
/// group-scoped silences; the machine is passed because the lookup now covers
/// that grain too, and it carries no silences of its own here.
async fn machine_of(conn: &mut database::diesel_async::AsyncPgConnection, app: Uuid) -> Uuid {
	#[derive(diesel::QueryableByName)]
	struct M {
		#[diesel(sql_type = sql_types::Uuid)]
		machine_id: Uuid,
	}
	sql_query("SELECT machine_id FROM applications WHERE id = $1")
		.bind::<sql_types::Uuid, _>(app)
		.get_result::<M>(conn)
		.await
		.expect("machine of application")
		.machine_id
}
