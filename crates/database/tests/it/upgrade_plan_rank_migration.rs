//! The `2026-09-03-001252-0000_upgrade_plan_rank` migration places each open
//! plan on the environment its group's headline version comes from.
//!
//! Replays it for real: reverts the migration, seeds plans in the shape they
//! had before it, then re-applies it. `applications.rank` is unconstrained
//! text, and the rank the migration writes is not: a spelling the mapping does
//! not know must reach the column's CHECK as production rather than as itself,
//! which would abort the deploy.

use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str =
	include_str!("../../../../migrations/2026-09-03-001252-0000_upgrade_plan_rank/up.sql");
const DOWN: &str =
	include_str!("../../../../migrations/2026-09-03-001252-0000_upgrade_plan_rank/down.sql");

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct Rank {
	#[diesel(sql_type = sql_types::Text)]
	rank: String,
}

async fn group(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('site') RETURNING id")
		.get_result(conn)
		.await
		.expect("group");
	row.id
}

/// One application on a box of its own, carrying `rank` exactly as given so a
/// legacy or unrecognised spelling can be seeded.
async fn member(
	conn: &mut diesel_async::AsyncPgConnection,
	group: Uuid,
	rank: Option<&str>,
	host: &str,
) -> Uuid {
	let machine: RowId = sql_query("INSERT INTO machines (group_id) VALUES ($1) RETURNING id")
		.bind::<sql_types::Uuid, _>(group)
		.get_result(conn)
		.await
		.expect("machine");
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, group_id, rank, machine_id, is_monitored) \
		 VALUES ('tamanu-central', $1, $2, $3, $4, true) RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(Some(group))
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("application");
	row.id
}

/// A published version, for the plan to target.
async fn version(conn: &mut diesel_async::AsyncPgConnection, semver: &str) -> Uuid {
	let row: RowId = sql_query(
		"INSERT INTO versions (major, minor, patch, status) \
		 VALUES ($1, $2, $3, 'published') RETURNING id",
	)
	.bind::<sql_types::Integer, _>(semver.split('.').nth(0).unwrap().parse::<i32>().unwrap())
	.bind::<sql_types::Integer, _>(semver.split('.').nth(1).unwrap().parse::<i32>().unwrap())
	.bind::<sql_types::Integer, _>(semver.split('.').nth(2).unwrap().parse::<i32>().unwrap())
	.get_result(conn)
	.await
	.expect("version");
	row.id
}

async fn plan(conn: &mut diesel_async::AsyncPgConnection, group: Uuid, target: Uuid) {
	sql_query("INSERT INTO upgrade_plans (group_id, target_version_id) VALUES ($1, $2)")
		.bind::<sql_types::Uuid, _>(group)
		.bind::<sql_types::Uuid, _>(target)
		.execute(conn)
		.await
		.expect("plan");
}

async fn revert(conn: &mut diesel_async::AsyncPgConnection) {
	conn.batch_execute(DOWN).await.expect("revert");
}

async fn apply(conn: &mut diesel_async::AsyncPgConnection) {
	conn.batch_execute(UP).await.expect("apply");
}

async fn open_ranks(conn: &mut diesel_async::AsyncPgConnection, group: Uuid) -> Vec<String> {
	let rows: Vec<Rank> = sql_query(
		"SELECT rank FROM upgrade_plans WHERE group_id = $1 AND met_at IS NULL \
		 AND superseded_at IS NULL AND withdrawn_at IS NULL ORDER BY rank",
	)
	.bind::<sql_types::Uuid, _>(group)
	.load(conn)
	.await
	.expect("open plans");
	rows.into_iter().map(|r| r.rank).collect()
}

/// The mapped spellings land on the rank they name.
// spec: UPG#a-plan
#[tokio::test(flavor = "multi_thread")]
async fn a_legacy_spelling_maps_to_its_rank() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let target = version(&mut conn, "2.66.0").await;
		member(&mut conn, g, Some("clone"), "http://clone.invalid/").await;

		revert(&mut conn).await;
		plan(&mut conn, g, target).await;
		sql_query("UPDATE applications SET rank = 'staging' WHERE group_id = $1")
			.bind::<sql_types::Uuid, _>(g)
			.execute(&mut conn)
			.await
			.expect("store the older spelling of clone");

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec!["clone".to_string()],
			"a group stored as staging is a clone",
		);
	})
	.await
}

/// A spelling the mapping does not know must not fail the deploy. The plan
/// takes production, the rank a plan gets when its group offers none.
///
/// `ServerRank` refuses to read such a value, so the application layer cannot
/// record one either: the row is seeded canonical and rewritten once the
/// revert has taken the model layer out of the way.
// spec: UPG#a-plan
#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_spelling_falls_to_production() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let g = group(&mut conn).await;
		let target = version(&mut conn, "2.66.0").await;
		member(&mut conn, g, Some("production"), "http://odd.invalid/").await;

		revert(&mut conn).await;
		plan(&mut conn, g, target).await;
		sql_query("UPDATE applications SET rank = 'preprod' WHERE group_id = $1")
			.bind::<sql_types::Uuid, _>(g)
			.execute(&mut conn)
			.await
			.expect("store a spelling the mapping does not know");

		apply(&mut conn).await;
		assert_eq!(
			open_ranks(&mut conn, g).await,
			vec!["production".to_string()],
			"the migration ran and swept it to production",
		);
	})
	.await
}
