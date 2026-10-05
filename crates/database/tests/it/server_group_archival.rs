//! Group archival (soft-delete): an empty group, or one whose live members are
//! all "gone" (no status in 7 days), can be archived — the latter cascades,
//! archiving those members too; a group with a recently-reporting server is
//! refused. Restore cascades back. Archived groups drop out of live listings.
//! Also covers `Application::list_archived`.

use commons_tests::db::TestDb;
use database::applications::Application;
use database::server_groups::ServerGroup;
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

/// Insert a server in a group; `archived` sets `deleted_at`.
async fn insert_server(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Uuid,
	archived: bool,
) -> Uuid {
	#[derive(diesel::QueryableByName)]
	struct RowId {
		#[diesel(sql_type = sql_types::Uuid)]
		id: Uuid,
	}
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	let row: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $2) RETURNING id) INSERT INTO applications (host, type, group_id, deleted_at, machine_id) SELECT $1, 'tamanu-central', $2, CASE WHEN $3 THEN now() ELSE NULL END, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Bool, _>(archived)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

/// Give a server a recent status so it counts as live (not "gone").
async fn insert_recent_status(
	conn: &mut database::diesel_async::AsyncPgConnection,
	server_id: Uuid,
) {
	sql_query(
		"INSERT INTO statuses (server_id, created_at, version, healthy, health)
		 VALUES ($1, now(), '1.0.0', true, '[]'::jsonb)",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.execute(conn)
	.await
	.expect("insert status");
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_a_group_with_a_recent_server_is_refused() {
	TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn, "Group").await;
		let server = insert_server(&mut conn, group, false).await;
		insert_recent_status(&mut conn, server).await; // reported now → not gone

		assert!(
			ServerGroup::soft_delete(&mut conn, group).await.is_err(),
			"a group with a recently-reporting server can't be archived",
		);
		// Neither the group nor the server was touched.
		assert!(
			ServerGroup::get_by_id(&mut conn, group)
				.await
				.unwrap()
				.deleted_at
				.is_none(),
		);
		assert!(
			Application::get_by_id(&mut conn, server)
				.await
				.unwrap()
				.deleted_at
				.is_none(),
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn archiving_an_all_gone_group_cascades_and_restore_reverses() {
	TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn, "Group").await;
		// Two live members with no status → both "gone".
		let a = insert_server(&mut conn, group, false).await;
		let b = insert_server(&mut conn, group, false).await;

		// Archive cascades to the gone members.
		ServerGroup::soft_delete(&mut conn, group).await.unwrap();
		assert!(
			ServerGroup::get_by_id(&mut conn, group)
				.await
				.unwrap()
				.deleted_at
				.is_some(),
			"group archived",
		);
		assert!(
			Application::get_by_id(&mut conn, a)
				.await
				.unwrap()
				.deleted_at
				.is_some(),
			"member a cascade-archived",
		);
		assert!(
			Application::get_by_id(&mut conn, b)
				.await
				.unwrap()
				.deleted_at
				.is_some(),
			"member b cascade-archived",
		);
		assert!(
			!ServerGroup::list_all(&mut conn)
				.await
				.unwrap()
				.iter()
				.any(|g| g.id == group)
		);

		// Restore cascades back.
		ServerGroup::restore(&mut conn, group).await.unwrap();
		assert!(
			ServerGroup::get_by_id(&mut conn, group)
				.await
				.unwrap()
				.deleted_at
				.is_none(),
			"group restored",
		);
		assert!(
			Application::get_by_id(&mut conn, a)
				.await
				.unwrap()
				.deleted_at
				.is_none(),
			"member a restored",
		);
		assert!(
			Application::get_by_id(&mut conn, b)
				.await
				.unwrap()
				.deleted_at
				.is_none(),
			"member b restored",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn server_list_archived_only_returns_archived() {
	TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn, "G").await;
		let live = insert_server(&mut conn, group, false).await;
		let archived = insert_server(&mut conn, group, true).await;

		let archived_ids: Vec<Uuid> = Application::list_archived(&mut conn)
			.await
			.unwrap()
			.into_iter()
			.map(|s| s.id)
			.collect();
		assert!(archived_ids.contains(&archived), "archived server listed");
		assert!(
			!archived_ids.contains(&live),
			"live server not in archived list"
		);

		let live_ids: Vec<Uuid> = Application::get_all(&mut conn, 0, None)
			.await
			.unwrap()
			.into_iter()
			.map(|s| s.id)
			.collect();
		assert!(live_ids.contains(&live), "live server in get_all");
		assert!(
			!live_ids.contains(&archived),
			"archived server excluded from get_all"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn live_server_counts_excludes_archived() {
	TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn, "G").await;
		insert_server(&mut conn, group, false).await; // live
		insert_server(&mut conn, group, false).await; // live
		insert_server(&mut conn, group, true).await; // archived — excluded

		let counts = ServerGroup::live_server_counts(&mut conn).await.unwrap();
		assert_eq!(
			counts.get(&group).copied(),
			Some(2),
			"counts only live (non-archived) members",
		);
	})
	.await
}

/// Restore is the inverse of the archival *cascade*, not a blanket
/// un-archive of everything in the group. A server an operator archived
/// deliberately before the group was archived must stay archived: the group's
/// `is_monitored` survives archival, so resurrecting a decommissioned box
/// puts it straight back into monitoring and it starts filing "never
/// reported" alerts.
#[tokio::test(flavor = "multi_thread")]
async fn restore_leaves_individually_archived_members_archived() {
	TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn, "Group").await;
		// Archived on its own, well before the group.
		let retired = insert_server(&mut conn, group, true).await;
		// Live, so the group's archival cascades over it.
		let cascaded = insert_server(&mut conn, group, false).await;

		ServerGroup::soft_delete(&mut conn, group).await.unwrap();
		assert!(
			Application::get_by_id(&mut conn, cascaded)
				.await
				.unwrap()
				.deleted_at
				.is_some(),
			"the live member is cascade-archived",
		);

		ServerGroup::restore(&mut conn, group).await.unwrap();

		assert!(
			Application::get_by_id(&mut conn, cascaded)
				.await
				.unwrap()
				.deleted_at
				.is_none(),
			"the cascade-archived member comes back",
		);
		assert!(
			Application::get_by_id(&mut conn, retired)
				.await
				.unwrap()
				.deleted_at
				.is_some(),
			"the individually-archived member stays archived",
		);
	})
	.await
}

/// The same resurrection by another route: restoring a group that was never
/// archived must not un-archive its members either.
#[tokio::test(flavor = "multi_thread")]
async fn restoring_a_live_group_does_not_resurrect_its_archived_members() {
	TestDb::run(async |mut conn, _| {
		let group = insert_group(&mut conn, "Group").await;
		let retired = insert_server(&mut conn, group, true).await;

		ServerGroup::restore(&mut conn, group).await.unwrap();

		assert!(
			Application::get_by_id(&mut conn, retired)
				.await
				.unwrap()
				.deleted_at
				.is_some(),
			"nothing was cascaded, so nothing comes back",
		);
	})
	.await
}
