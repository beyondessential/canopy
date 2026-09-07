//! Each source's current server-wide detail: how a push replaces it, how the
//! sources resolve into one set of figures, and how long a value lasts.

use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde_json::json;
use uuid::Uuid;

use database::reported_detail::ReportedDetail;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut AsyncPgConnection) -> Uuid {
	let host = format!("http://detail.invalid/{}", Uuid::new_v4());
	let machine: RowId = sql_query("INSERT INTO machines DEFAULT VALUES RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	let server: RowId =
		sql_query("INSERT INTO applications (type, host, machine_id) VALUES ('tamanu-central', $1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(host)
			.bind::<sql_types::Uuid, _>(machine.id)
			.get_result(conn)
			.await
			.expect("insert server");
	server.id
}

async fn insert_production_server(conn: &mut AsyncPgConnection) -> Uuid {
	let host = format!("http://prod.invalid/{}", Uuid::new_v4());
	let machine: RowId = sql_query("INSERT INTO machines DEFAULT VALUES RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	let server: RowId = sql_query(
		"INSERT INTO applications (type, host, rank, machine_id) VALUES ('tamanu-central', $1, 'production', $2) RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("insert production server");
	server.id
}

async fn age_report(conn: &mut AsyncPgConnection, server: Uuid, interval: &str) {
	sql_query(format!(
		"UPDATE application_reported_detail SET reported_at = NOW() - INTERVAL '{interval}' \
		 WHERE application_id = $1"
	))
	.bind::<sql_types::Uuid, _>(server)
	.execute(conn)
	.await
	.expect("age report");
}

/// A report carrying no version keeps the version its source last reported —
/// the agent reporting while the application is down doesn't mean the server
/// stopped being on that version.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn a_version_less_report_keeps_the_last_version() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server = insert_server(&mut conn).await;
		let m_server = machine_of(&mut conn, server).await;

		ReportedDetail::record(
			&mut conn,
			Some(server),
			m_server,
			"alertd",
			&json!({}),
			Some(&"2.34.1".parse().unwrap()),
		)
		.await
		.unwrap();
		ReportedDetail::record(
			&mut conn,
			Some(server),
			m_server,
			"alertd",
			&json!({"uptimeSecs": 42}),
			None,
		)
		.await
		.unwrap();

		assert_eq!(
			ReportedDetail::last_version(&mut conn, server)
				.await
				.unwrap()
				.map(|v| v.to_string())
				.as_deref(),
			Some("2.34.1"),
		);

		// An explicit later version still supersedes it.
		ReportedDetail::record(
			&mut conn,
			Some(server),
			m_server,
			"alertd",
			&json!({}),
			Some(&"2.35.0".parse().unwrap()),
		)
		.await
		.unwrap();
		assert_eq!(
			ReportedDetail::last_version(&mut conn, server)
				.await
				.unwrap()
				.map(|v| v.to_string())
				.as_deref(),
			Some("2.35.0"),
		);
	})
	.await
}

/// The last version survives however long the server has been quiet: a group's
/// headline version shouldn't blank out because its canonical member is down.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn last_version_is_not_bounded_by_a_lookback() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server = insert_server(&mut conn).await;
		let m_server = machine_of(&mut conn, server).await;
		ReportedDetail::record(
			&mut conn,
			Some(server),
			m_server,
			"alertd",
			&json!({}),
			Some(&"2.34.1".parse().unwrap()),
		)
		.await
		.unwrap();
		// Well past the ninety-day cap the status-history read needed.
		age_report(&mut conn, server, "200 days").await;

		assert_eq!(
			ReportedDetail::last_version(&mut conn, server)
				.await
				.unwrap()
				.map(|v| v.to_string())
				.as_deref(),
			Some("2.34.1"),
		);
	})
	.await
}

/// The active-version summary counts each still-reporting production server
/// once, at the version the most recent source to report one gave.
// spec: FIG#active-versions
#[tokio::test(flavor = "multi_thread")]
async fn production_versions_counts_reporting_servers_once() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server = insert_production_server(&mut conn).await;
		let m_server = machine_of(&mut conn, server).await;
		ReportedDetail::record(
			&mut conn,
			Some(server),
			m_server,
			"alertd",
			&json!({}),
			Some(&"2.34.1".parse().unwrap()),
		)
		.await
		.unwrap();
		// A later source reports no version at all: the server still runs
		// 2.34.1, and must not drop out of the summary.
		age_report(&mut conn, server, "2 hours").await;
		ReportedDetail::record(
			&mut conn,
			Some(server),
			m_server,
			"tamanu",
			&json!({"uptimeSecs": 42}),
			None,
		)
		.await
		.unwrap();

		let versions = ReportedDetail::production_versions(&mut conn)
			.await
			.expect("production versions");
		assert_eq!(
			versions.iter().map(ToString::to_string).collect::<Vec<_>>(),
			vec!["2.34.1"],
			"one entry per server, from the newest source that reported a version",
		);
	})
	.await
}

/// Only production applications that are still reporting count as actively
/// running something.
// spec: FIG#active-versions
#[tokio::test(flavor = "multi_thread")]
async fn production_versions_excludes_the_quiet_and_the_unranked() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let live = insert_production_server(&mut conn).await;
		let quiet = insert_production_server(&mut conn).await;
		let unranked = insert_server(&mut conn).await;

		for (server, version) in [(live, "2.34.1"), (quiet, "2.10.0"), (unranked, "2.20.0")] {
			let machine = machine_of(&mut conn, server).await;
			ReportedDetail::record(
				&mut conn,
				Some(server),
				machine,
				"alertd",
				&json!({}),
				Some(&version.parse().unwrap()),
			)
			.await
			.unwrap();
		}
		age_report(&mut conn, quiet, "8 days").await;

		let versions: Vec<String> = ReportedDetail::production_versions(&mut conn)
			.await
			.expect("production versions")
			.iter()
			.map(ToString::to_string)
			.collect();
		assert_eq!(
			versions,
			vec!["2.34.1"],
			"a server quiet beyond the week, and a server that isn't production, are not \
			 actively running anything",
		);
	})
	.await
}

/// A source's report replaces what it reported before, and leaves other
/// sources' reports alone.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn a_report_replaces_its_own_source_only() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let m_server_id = machine_of(&mut conn, server_id).await;

		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"bestoolVersion": "2.9.1", "pgVersion": "PostgreSQL 16.3 on x86_64"}),
			None,
		)
		.await
		.expect("record alertd");
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"tamanu",
			&json!({"uptimeSecs": 42}),
			None,
		)
		.await
		.expect("record tamanu");

		// alertd reports again, dropping pgVersion from its payload.
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"bestoolVersion": "2.10.5"}),
			None,
		)
		.await
		.expect("re-record alertd");

		let reports = ReportedDetail::for_server(&mut conn, server_id)
			.await
			.expect("read reports");
		// Two sources, and each contributes at both grains now that detail
		// splits: the box's fields on the machine, the rest on the
		// application. Still one row per (source, grain), not one per push.
		assert_eq!(
			reports.len(),
			4,
			"one row per source per grain, not one per push"
		);

		let figures = ReportedDetail::merge(&reports);
		assert_eq!(
			figures.bestool_version().as_deref(),
			Some("2.10.5"),
			"the newer report supersedes the source's previous one",
		);
		assert_eq!(
			figures.postgres_version(),
			None,
			"a push is the source's whole truth: a field it drops is no longer reported",
		);
		assert_eq!(
			figures.get("uptimeSecs"),
			Some(&json!(42)),
			"the other source's report is untouched",
		);
	})
	.await
}

/// Figures resolve per field across sources: the most recent source to
/// report a field wins it, and a source that doesn't carry it doesn't erase
/// it.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn figures_resolve_per_field_newest_first() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let m_server_id = machine_of(&mut conn, server_id).await;

		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"pgVersion": "PostgreSQL 16.3 (Visual C++ build 1940), 64-bit", "nodeVersion": "20.11.0"}),
			None,
		)
		.await
		.expect("record alertd");
		// Reported later, and carries none of alertd's fields.
		sql_query(
			"UPDATE application_reported_detail SET reported_at = NOW() - INTERVAL '1 hour' \
			 WHERE application_id = $1",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("age alertd's report");
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"tamanu",
			&json!({"uptimeSecs": 42}),
			None,
		)
			.await
			.expect("record tamanu");

		let figures =
			ReportedDetail::merge(&ReportedDetail::for_server(&mut conn, server_id).await.unwrap());
		assert_eq!(figures.postgres_version().as_deref(), Some("16.3"));
		assert_eq!(figures.platform().as_deref(), Some("Windows"));
		assert_eq!(figures.node_version().as_deref(), Some("20.11.0"));
	})
	.await
}

/// The Munin flag is one of the reported figures, so it holds for as long as
/// the server exists rather than expiring with a lookback — and an explicit
/// later value overrides an earlier one.
// spec: SVC#munin-link
#[tokio::test(flavor = "multi_thread")]
async fn munin_flag_holds_indefinitely() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let m_server_id = machine_of(&mut conn, server_id).await;

		let munin = async |conn: &mut AsyncPgConnection| {
			ReportedDetail::merge(&ReportedDetail::for_server(conn, server_id).await.unwrap())
				.munin()
		};

		assert_eq!(munin(&mut conn).await, None, "never reported");

		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"munin": true}),
			None,
		)
		.await
		.expect("record munin");
		// Long past any status-history lookback.
		sql_query(
			"UPDATE application_reported_detail SET reported_at = NOW() - INTERVAL '400 days' \
			 WHERE application_id = $1",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("age the report");
		assert_eq!(
			munin(&mut conn).await,
			Some(true),
			"a reported flag doesn't expire, however long the server has been quiet",
		);

		// A source that reports nothing about munin leaves it alone...
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"tamanu",
			&json!({"uptimeSecs": 42}),
			None,
		)
		.await
		.expect("record tamanu");
		assert_eq!(munin(&mut conn).await, Some(true));

		// ...but an explicit later false overrides it.
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"munin": false}),
			None,
		)
		.await
		.expect("record munin=false");
		assert_eq!(munin(&mut conn).await, Some(false));
	})
	.await
}

/// Deleting a server takes its reported detail with it, so the fleet read
/// can't surface a server that no longer exists.
// spec: FIG#sourcing
#[tokio::test(flavor = "multi_thread")]
async fn reported_detail_is_deleted_with_its_server() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let m_server_id = machine_of(&mut conn, server_id).await;
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"munin": true}),
			None,
		)
		.await
		.expect("record");

		sql_query("DELETE FROM applications WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server_id)
			.execute(&mut conn)
			.await
			.expect("delete server");

		assert!(
			ReportedDetail::for_server(&mut conn, server_id)
				.await
				.expect("read reports")
				.is_empty()
		);
	})
	.await
}

/// The platform is what the server says it runs; only a server that reports
/// no operating system falls back to what the database engine gives away.
// spec: FIG#figures
#[tokio::test(flavor = "multi_thread")]
async fn platform_prefers_the_reported_operating_system() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let m_server_id = machine_of(&mut conn, server_id).await;
		let platform = async |conn: &mut AsyncPgConnection| {
			ReportedDetail::merge(&ReportedDetail::for_server(conn, server_id).await.unwrap())
				.platform()
		};

		// Only a PostgreSQL banner: the family is all it can give.
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"pgVersion": "PostgreSQL 16.3 (Visual C++ build 1940), 64-bit"}),
			None,
		)
		.await
		.unwrap();
		assert_eq!(platform(&mut conn).await.as_deref(), Some("Windows"));

		// A reported name supersedes the inference, and the version qualifies
		// it. Values are as the fleet actually reports them.
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"osName": "Windows", "osVersion": "10 (17763)"}),
			None,
		)
		.await
		.unwrap();
		assert_eq!(
			platform(&mut conn).await.as_deref(),
			Some("Windows 10 (17763)"),
			"the reported OS is finer than the family the banner implies",
		);

		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"osName": "Ubuntu", "osVersion": "24.04"}),
			None,
		)
		.await
		.unwrap();
		assert_eq!(platform(&mut conn).await.as_deref(), Some("Ubuntu 24.04"));

		// A name without a version stands alone.
		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			m_server_id,
			"alertd",
			&json!({"osName": "Debian GNU/Linux"}),
			None,
		)
		.await
		.unwrap();
		assert_eq!(
			platform(&mut conn).await.as_deref(),
			Some("Debian GNU/Linux"),
		);
	})
	.await
}

/// The fleet read resolves each server's figures independently.
// spec: FIG#fleet-spread
#[tokio::test(flavor = "multi_thread")]
async fn merge_by_server_keeps_servers_apart() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let one = insert_server(&mut conn).await;
		let m_one = machine_of(&mut conn, one).await;
		let two = insert_server(&mut conn).await;
		let m_two = machine_of(&mut conn, two).await;

		ReportedDetail::record(
			&mut conn,
			Some(one),
			m_one,
			"alertd",
			&json!({"bestoolVersion": "2.10.5"}),
			None,
		)
		.await
		.unwrap();
		ReportedDetail::record(
			&mut conn,
			Some(two),
			m_two,
			"alertd",
			&json!({"bestoolVersion": "2.4.7"}),
			None,
		)
		.await
		.unwrap();

		let merged = ReportedDetail::merge_by_server(ReportedDetail::all(&mut conn).await.unwrap());
		assert_eq!(
			merged.get(&one).unwrap().0.bestool_version().as_deref(),
			Some("2.10.5")
		);
		assert_eq!(
			merged.get(&two).unwrap().0.bestool_version().as_deref(),
			Some("2.4.7")
		);
	})
	.await
}

/// The machine an application sits on. Reported detail splits by grain, so a
/// push names both: the box's fields go to the machine, the rest here.
async fn machine_of(conn: &mut database::diesel_async::AsyncPgConnection, app: Uuid) -> Uuid {
	#[derive(diesel::QueryableByName)]
	struct M {
		#[diesel(sql_type = diesel::sql_types::Uuid)]
		machine_id: Uuid,
	}
	diesel::sql_query("SELECT machine_id FROM applications WHERE id = $1")
		.bind::<diesel::sql_types::Uuid, _>(app)
		.get_result::<M>(conn)
		.await
		.expect("machine of application")
		.machine_id
}

/// Detail splits by grain on the way in: the box's facts to the machine, the
/// workload's to the application. A host running two workloads records its
/// platform once rather than once per workload.
// spec: FIG
#[tokio::test(flavor = "multi_thread")]
async fn detail_is_stored_at_the_grain_it_describes() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let machine_id = machine_of(&mut conn, server_id).await;

		ReportedDetail::record(
			&mut conn,
			Some(server_id),
			machine_id,
			"alertd",
			&json!({
				"osName": "Debian",
				"bestoolVersion": "2.10.5",
				"pgVersion": "PostgreSQL 16.3",
				"tamanuVersion": "2.34.1",
			}),
			None,
		)
		.await
		.expect("record");

		#[derive(diesel::QueryableByName)]
		struct Extra {
			#[diesel(sql_type = sql_types::Jsonb)]
			extra: serde_json::Value,
		}
		let on_machine: Extra =
			sql_query("SELECT extra FROM machine_reported_detail WHERE machine_id = $1")
				.bind::<sql_types::Uuid, _>(machine_id)
				.get_result(&mut conn)
				.await
				.expect("machine detail");
		assert_eq!(on_machine.extra["osName"], "Debian");
		assert_eq!(on_machine.extra["bestoolVersion"], "2.10.5");
		assert!(
			on_machine.extra.get("pgVersion").is_none(),
			"the database engine is the workload's, not the box's"
		);

		let on_application: Extra =
			sql_query("SELECT extra FROM application_reported_detail WHERE application_id = $1")
				.bind::<sql_types::Uuid, _>(server_id)
				.get_result(&mut conn)
				.await
				.expect("application detail");
		assert_eq!(on_application.extra["pgVersion"], "PostgreSQL 16.3");
		assert!(
			on_application.extra.get("osName").is_none(),
			"the platform is the box's, and is stored once"
		);

		// A reader is not meant to care which grain holds what.
		let figures = ReportedDetail::merge(
			&ReportedDetail::for_server(&mut conn, server_id)
				.await
				.expect("read"),
		);
		assert_eq!(figures.platform().as_deref(), Some("Debian"));
		assert_eq!(figures.bestool_version().as_deref(), Some("2.10.5"));
		assert_eq!(figures.postgres_version().as_deref(), Some("16.3"));
	})
	.await
}

/// Two applications on one box both present its platform, and it is stored
/// once — the failure mode the whole split exists to prevent.
// spec: FIG
#[tokio::test(flavor = "multi_thread")]
async fn one_box_reports_its_platform_once_for_both_workloads() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let first = insert_server(&mut conn).await;
		let machine_id = machine_of(&mut conn, first).await;
		let host = format!("http://second.invalid/{}", Uuid::new_v4());
		let second: Uuid = sql_query(
			"INSERT INTO applications (host, type, machine_id) \
			 VALUES ($1, 'tamanu-central', $2) RETURNING id",
		)
		.bind::<sql_types::Text, _>(host)
		.bind::<sql_types::Uuid, _>(machine_id)
		.get_result::<RowId>(&mut conn)
		.await
		.expect("second application on the same box")
		.id;

		for app in [first, second] {
			ReportedDetail::record(
				&mut conn,
				Some(app),
				machine_id,
				"alertd",
				&json!({"osName": "Debian"}),
				None,
			)
			.await
			.expect("record");
		}

		#[derive(diesel::QueryableByName)]
		struct Count {
			#[diesel(sql_type = sql_types::BigInt)]
			count: i64,
		}
		let stored: i64 = sql_query("SELECT count(*) AS count FROM machine_reported_detail")
			.get_result::<Count>(&mut conn)
			.await
			.expect("count")
			.count;
		assert_eq!(
			stored, 1,
			"the box's platform is stored once, not per workload"
		);

		for app in [first, second] {
			let figures = ReportedDetail::merge(
				&ReportedDetail::for_server(&mut conn, app)
					.await
					.expect("read"),
			);
			assert_eq!(
				figures.platform().as_deref(),
				Some("Debian"),
				"both workloads present their box's platform"
			);
		}
	})
	.await
}

/// A body that is not an object is stored as the empty object.
///
/// `extra` is `JSONB NOT NULL`, which admits JSON `null`, and a stored `null`
/// breaks every bulk read of the column: `jsonb_each` refuses a non-object and
/// so does a `-` key delete. That surfaces far from the push that caused it —
/// once, as a migration failing against production data. A non-object body
/// carries no fields, so the empty object says the same thing safely.
#[tokio::test(flavor = "multi_thread")]
async fn a_non_object_body_is_stored_as_an_object() {
	#[derive(QueryableByName)]
	struct Typeof {
		#[diesel(sql_type = sql_types::Text)]
		type_of: String,
	}

	async fn typeof_application(conn: &mut AsyncPgConnection, app: Uuid) -> String {
		sql_query(
			"SELECT jsonb_typeof(extra) AS type_of FROM application_reported_detail \
			 WHERE application_id = $1",
		)
		.bind::<sql_types::Uuid, _>(app)
		.get_result::<Typeof>(conn)
		.await
		.expect("read application body")
		.type_of
	}

	async fn typeof_machine(conn: &mut AsyncPgConnection, machine: Uuid) -> String {
		sql_query(
			"SELECT jsonb_typeof(extra) AS type_of FROM machine_reported_detail \
			 WHERE machine_id = $1",
		)
		.bind::<sql_types::Uuid, _>(machine)
		.get_result::<Typeof>(conn)
		.await
		.expect("read machine body")
		.type_of
	}

	commons_tests::db::TestDb::run(async |mut conn, _| {
		// Every shape `NOT NULL` lets past: JSON null, and the other scalars.
		for body in [
			json!(null),
			json!("a string"),
			json!(42),
			json!(true),
			json!(["an", "array"]),
		] {
			let app = insert_server(&mut conn).await;
			let machine = machine_of(&mut conn, app).await;

			ReportedDetail::record(&mut conn, Some(app), machine, "alertd", &body, None)
				.await
				.expect("record a non-object body");

			assert_eq!(
				typeof_application(&mut conn, app).await,
				"object",
				"{body} is stored on the application as an object"
			);
			assert_eq!(
				typeof_machine(&mut conn, machine).await,
				"object",
				"{body} is stored on the machine as an object"
			);
		}

		// The machine-only path: no application to split into, so the whole body
		// goes to the box and skips `split_by_grain` entirely.
		let lone = insert_server(&mut conn).await;
		let lone_machine = machine_of(&mut conn, lone).await;
		ReportedDetail::record(&mut conn, None, lone_machine, "alertd", &json!(null), None)
			.await
			.expect("record a machine-only non-object body");
		assert_eq!(
			typeof_machine(&mut conn, lone_machine).await,
			"object",
			"a machine-only push stores an object too"
		);

		// The operation that failed in production, over every row written above.
		let walked: i64 = sql_query(
			"SELECT count(*) AS count FROM ( \
			   SELECT jsonb_each(extra) FROM application_reported_detail \
			   UNION ALL SELECT jsonb_each(extra) FROM machine_reported_detail \
			 ) AS walked",
		)
		.get_result::<Count>(&mut conn)
		.await
		.expect("jsonb_each walks every stored body")
		.count;
		assert_eq!(walked, 0, "an empty body contributes no fields");
	})
	.await
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

/// A source that pushes without a version keeps the one it last carried, and
/// that push must not make its older version look newer than another source's.
#[tokio::test(flavor = "multi_thread")]
async fn a_version_less_push_does_not_outrank_a_newer_version_from_another_source() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server = insert_server(&mut conn).await;
		let machine = machine_of(&mut conn, server).await;
		let old: commons_types::version::VersionStr = "2.60.0".parse().expect("parse");
		let new: commons_types::version::VersionStr = "2.62.0".parse().expect("parse");
		ReportedDetail::record(
			&mut conn,
			Some(server),
			machine,
			"alertd",
			&json!({}),
			Some(&old),
		)
		.await
		.expect("alertd");
		sql_query(
			"UPDATE application_reported_detail \
			 SET reported_at = reported_at - interval '2 hours', \
			     version_reported_at = version_reported_at - interval '2 hours' \
			 WHERE application_id = $1",
		)
		.bind::<sql_types::Uuid, _>(server)
		.execute(&mut conn)
		.await
		.expect("age");
		ReportedDetail::record(
			&mut conn,
			Some(server),
			machine,
			"tamanu",
			&json!({}),
			Some(&new),
		)
		.await
		.expect("tamanu");
		sql_query(
			"UPDATE application_reported_detail \
			 SET reported_at = reported_at - interval '1 hour', \
			     version_reported_at = version_reported_at - interval '1 hour' \
			 WHERE application_id = $1 AND source = 'tamanu'",
		)
		.bind::<sql_types::Uuid, _>(server)
		.execute(&mut conn)
		.await
		.expect("age");

		// alertd speaks again, saying nothing about the version.
		ReportedDetail::record(&mut conn, Some(server), machine, "alertd", &json!({}), None)
			.await
			.expect("alertd again");

		assert_eq!(
			ReportedDetail::last_version(&mut conn, server)
				.await
				.expect("last")
				.map(|v| v.to_string()),
			Some("2.62.0".to_string()),
			"the newest version carried wins, not the newest source to speak"
		);
	})
	.await
}
