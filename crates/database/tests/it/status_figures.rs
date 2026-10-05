//! Resolving a server's reported figures across the sources reporting on it.
//!
//! Sources interleave and don't carry the same fields, so the figures are
//! resolved per key from the most recent source to report each one.

use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

use database::statuses::{MergedDetail, Status};

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

/// `extra` and `created_at` are test-controlled SQL literals, interpolated
/// directly; the server id and source are bound.
async fn insert_status(
	conn: &mut AsyncPgConnection,
	server_id: Uuid,
	source: &str,
	extra: &str,
	created_at: &str,
) {
	let query = format!(
		"INSERT INTO statuses (server_id, source, extra, created_at) \
		 VALUES ($1, $2, {extra}::jsonb, {created_at})"
	);
	sql_query(query)
		.bind::<sql_types::Uuid, _>(server_id)
		.bind::<sql_types::Text, _>(source)
		.execute(conn)
		.await
		.expect("insert status");
}

async fn insert_server(conn: &mut AsyncPgConnection) -> Uuid {
	let host = format!("http://figures.invalid/{}", Uuid::new_v4());
	let server: RowId = sql_query("WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) INSERT INTO applications (type, host, machine_id) SELECT 'tamanu-central', $1, m.id FROM m RETURNING id")
		.bind::<sql_types::Text, _>(host)
		.get_result(conn)
		.await
		.expect("insert server");
	server.id
}

async fn figures(conn: &mut AsyncPgConnection, server_id: Uuid) -> MergedDetail {
	let statuses = Status::latest_per_source_at(conn, server_id, None)
		.await
		.expect("load statuses per source");
	MergedDetail::from_statuses(&statuses)
}

/// A newer push from a source that carries none of the figures doesn't blank
/// them: each falls through to the most recent source that reports it.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn figures_fall_through_to_the_source_that_reports_them() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;

		insert_status(
			&mut conn,
			server_id,
			"alertd",
			r#"'{"pgVersion": "PostgreSQL 16.3 on x86_64-pc-linux-gnu, compiled by gcc", "nodeVersion": "20.11.0", "bestoolVersion": "2.10.5", "timezone": "Pacific/Auckland"}'"#,
			"NOW() - INTERVAL '2 hours'",
		)
		.await;
		// The legacy Tamanu source reports later, and carries none of them.
		insert_status(
			&mut conn,
			server_id,
			"tamanu",
			r#"'{"uptimeSecs": 6038594}'"#,
			"NOW() - INTERVAL '1 hour'",
		)
		.await;

		let figures = figures(&mut conn, server_id).await;
		assert_eq!(figures.postgres_version().as_deref(), Some("16.3"));
		assert_eq!(figures.node_version().as_deref(), Some("20.11.0"));
		assert_eq!(figures.bestool_version().as_deref(), Some("2.10.5"));
		assert_eq!(figures.timezone().as_deref(), Some("Pacific/Auckland"));
		assert_eq!(figures.platform().as_deref(), Some("Linux"));
	})
	.await
}

/// Where two sources report the same figure, the newest report wins — by time,
/// not by source name.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn newest_report_wins_per_figure() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;

		// Source names ordered against their report times, so a merge that
		// trusted the per-source query's (alphabetical) row order would take
		// the stale value.
		insert_status(
			&mut conn,
			server_id,
			"zeta",
			r#"'{"pgVersion": "PostgreSQL 14.1 on x86_64-pc-linux-gnu, compiled by gcc"}'"#,
			"NOW() - INTERVAL '3 hours'",
		)
		.await;
		insert_status(
			&mut conn,
			server_id,
			"alpha",
			r#"'{"pgVersion": "PostgreSQL 16.3 (Visual C++ build 1940), 64-bit"}'"#,
			"NOW() - INTERVAL '1 hour'",
		)
		.await;

		let figures = figures(&mut conn, server_id).await;
		assert_eq!(
			figures.postgres_version().as_deref(),
			Some("16.3"),
			"the later report supersedes the earlier one",
		);
		assert_eq!(
			figures.platform().as_deref(),
			Some("Windows"),
			"platform follows the same report the version came from",
		);
	})
	.await
}

/// A server no bestool reports on presents no bestool version.
// spec: FIG#figures
#[tokio::test(flavor = "multi_thread")]
async fn bestool_version_absent_when_no_source_reports_it() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		insert_status(
			&mut conn,
			server_id,
			"tamanu",
			r#"'{"uptimeSecs": 42}'"#,
			"NOW() - INTERVAL '5 minutes'",
		)
		.await;

		assert_eq!(figures(&mut conn, server_id).await.bestool_version(), None);
	})
	.await
}

/// The figures see only as far back as the per-source lookback: a source
/// silent beyond it contributes nothing, so its figures read as unreported
/// rather than costing an unbounded scan of the partitioned history.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn figures_from_a_long_silent_source_are_dropped() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		insert_status(
			&mut conn,
			server_id,
			"alertd",
			r#"'{"bestoolVersion": "2.10.5"}'"#,
			"NOW() - INTERVAL '29 days'",
		)
		.await;
		assert_eq!(
			figures(&mut conn, server_id)
				.await
				.bestool_version()
				.as_deref(),
			Some("2.10.5"),
			"a report inside the lookback still counts",
		);

		sql_query(
			"UPDATE statuses SET created_at = NOW() - INTERVAL '31 days' WHERE server_id = $1",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("age the status out of the window");

		assert_eq!(
			figures(&mut conn, server_id).await.bestool_version(),
			None,
			"a source silent beyond the lookback contributes no figures",
		);
	})
	.await
}
