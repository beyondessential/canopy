//! The `server_groups` version cache: `recompute_version` picks the canonical
//! member and caches its last version-bearing status, and the `statuses` AFTER
//! INSERT trigger keeps `effective_version` fresh for the canonical member only.

use commons_tests::db::TestDb;
use database::server_groups::ServerGroup;
use diesel::{ExpressionMethods, QueryDsl, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

/// Insert a group, returning its id.
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

/// Insert an application of the given type and rank into a group, returning its id.
async fn insert_server(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Uuid,
	r#type: &str,
	rank: Option<&str>,
) -> Uuid {
	#[derive(diesel::QueryableByName)]
	struct RowId {
		#[diesel(sql_type = sql_types::Uuid)]
		id: Uuid,
	}
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	let row: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (group_id) VALUES ($4) RETURNING id) INSERT INTO applications (host, type, rank, group_id, machine_id) SELECT $1, $2, $3, $4, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Text, _>(r#type)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(group_id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

/// Insert a status row (fires the trigger). `created_at` is `now()` offset by
/// `offset_secs` so all rows land in a partition the fresh test DB has.
async fn insert_status(
	conn: &mut database::diesel_async::AsyncPgConnection,
	server_id: Uuid,
	version: Option<&str>,
	offset_secs: i64,
) {
	sql_query(
		"INSERT INTO statuses (server_id, created_at, version, healthy, health)
		 VALUES ($1, now() + ($2 || ' seconds')::interval, $3, true, '[]'::jsonb)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(offset_secs.to_string())
	.bind::<sql_types::Nullable<sql_types::Text>, _>(version)
	.execute(conn)
	.await
	.expect("insert status");

	// Mirror ingestion's current-detail upsert, which is what the cache
	// recompute reads — including its rule that a version-less push keeps the
	// version the source last reported.
	sql_query(
		"INSERT INTO application_reported_detail (application_id, source, version, reported_at)
		 VALUES ($1, 'alertd', $3, now() + ($2 || ' seconds')::interval)
		 ON CONFLICT (application_id, source) DO UPDATE
		 SET version = COALESCE(EXCLUDED.version, application_reported_detail.version),
		     reported_at = EXCLUDED.reported_at",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.bind::<sql_types::Text, _>(offset_secs.to_string())
	.bind::<sql_types::Nullable<sql_types::Text>, _>(version)
	.execute(conn)
	.await
	.expect("record reported detail");
}

async fn cache(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Uuid,
) -> (Option<Uuid>, Option<String>) {
	use database::schema::server_groups::dsl;
	dsl::server_groups
		.select((dsl::version_application_id, dsl::effective_version))
		.filter(dsl::id.eq(group_id))
		.first(conn)
		.await
		.expect("load cache")
}

#[tokio::test(flavor = "multi_thread")]
async fn recompute_picks_canonical_and_trigger_updates_only_it() {
	TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn, "Group").await;
		// A dev central is the only member at first. The headline is a
		// central's version, so the members that can carry it are centrals;
		// rank is what orders them.
		let dev = insert_server(&mut conn, group_id, "tamanu-central", Some("dev")).await;
		insert_status(&mut conn, dev, Some("1.0.0"), -60).await;

		// (a) recompute picks the (lone) canonical member.
		ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();
		let (vsid, ver) = cache(&mut conn, group_id).await;
		assert_eq!(vsid, Some(dev));
		assert_eq!(ver.as_deref(), Some("1.0.0"));

		// (b) a higher-ranked server added to the group flips the cache after
		// recompute.
		let prod = insert_server(&mut conn, group_id, "tamanu-central", Some("production")).await;
		insert_status(&mut conn, prod, Some("2.0.0"), -50).await;
		ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();
		let (vsid, ver) = cache(&mut conn, group_id).await;
		assert_eq!(vsid, Some(prod), "production-central is now canonical");
		assert_eq!(ver.as_deref(), Some("2.0.0"));

		// (c) the trigger updates effective_version only for the canonical
		// server, and only for version-bearing statuses.
		insert_status(&mut conn, prod, Some("2.1.0"), -10).await;
		let (_, ver) = cache(&mut conn, group_id).await;
		assert_eq!(
			ver.as_deref(),
			Some("2.1.0"),
			"canonical server's new version updates the cache"
		);

		// A later status from the non-canonical dev central is ignored.
		insert_status(&mut conn, dev, Some("9.9.9"), -5).await;
		let (_, ver) = cache(&mut conn, group_id).await;
		assert_eq!(
			ver.as_deref(),
			Some("2.1.0"),
			"non-canonical member's version does not touch the cache"
		);

		// A NULL-version (down/error) status from the canonical server is
		// ignored, so the cached version is never blanked.
		insert_status(&mut conn, prod, None, -1).await;
		let (_, ver) = cache(&mut conn, group_id).await;
		assert_eq!(
			ver.as_deref(),
			Some("2.1.0"),
			"a NULL-version status from the canonical server does not blank the cache"
		);

		// (d) deleting the canonical member then recomputing falls back to the
		// next-ranked member.
		sql_query("DELETE FROM applications WHERE id = $1")
			.bind::<sql_types::Uuid, _>(prod)
			.execute(&mut conn)
			.await
			.expect("delete canonical server");
		ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();
		let (vsid, ver) = cache(&mut conn, group_id).await;
		assert_eq!(vsid, Some(dev), "falls back to the remaining dev member");
		assert_eq!(
			ver.as_deref(),
			Some("9.9.9"),
			"dev member's last version-bearing status"
		);
	})
	.await
}

async fn updated_at(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Uuid,
) -> jiff::Timestamp {
	use database::schema::server_groups::dsl;
	let stamp: jiff_diesel::Timestamp = dsl::server_groups
		.select(dsl::updated_at)
		.filter(dsl::id.eq(group_id))
		.first(conn)
		.await
		.expect("load updated_at");
	stamp.into()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeated_version_leaves_the_group_row_alone() {
	TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn, "Group").await;
		let central =
			insert_server(&mut conn, group_id, "tamanu-central", Some("production")).await;
		insert_status(&mut conn, central, Some("1.0.0"), -60).await;
		ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();

		let before = updated_at(&mut conn, group_id).await;
		insert_status(&mut conn, central, Some("1.0.0"), -30).await;
		assert_eq!(
			updated_at(&mut conn, group_id).await,
			before,
			"a push carrying the cached version does not write the group row"
		);

		insert_status(&mut conn, central, Some("1.1.0"), -10).await;
		assert!(
			updated_at(&mut conn, group_id).await > before,
			"a push carrying a new version still writes it"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn recompute_clears_cache_when_no_members() {
	TestDb::run(async |mut conn, _| {
		let group_id = insert_group(&mut conn, "Empty").await;
		let server = insert_server(&mut conn, group_id, "tamanu-central", Some("production")).await;
		insert_status(&mut conn, server, Some("3.0.0"), -10).await;
		ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();
		let (vsid, ver) = cache(&mut conn, group_id).await;
		assert_eq!(vsid, Some(server));
		assert_eq!(ver.as_deref(), Some("3.0.0"));

		sql_query("DELETE FROM applications WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("delete server");
		ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();
		let (vsid, ver) = cache(&mut conn, group_id).await;
		assert_eq!(vsid, None, "no members → cache cleared");
		assert_eq!(ver, None);
	})
	.await
}
