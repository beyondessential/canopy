//! `issues::check_detail_by_server` — every server's current check state as a
//! per-check bag of fields, which is what a `check.field` fleet lookup reads.

use commons_types::status::CheckResult;
use database::check_policies::ScopedCheckPolicy;
use database::issues::{CheckFiling, Scope, check_detail_by_server, file_check};

use crate::helpers::app_ns;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use serde_json::json;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection, host: &str) -> Uuid {
	let machine: RowId = sql_query("INSERT INTO machines DEFAULT VALUES RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	let row: RowId =
		sql_query("INSERT INTO applications (type, host, machine_id) VALUES ('tamanu-central', $1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(host)
			.bind::<sql_types::Uuid, _>(machine.id)
			.get_result(conn)
			.await
			.expect("insert server");
	row.id
}

fn filing<'a>(
	server_id: Uuid,
	source: &'a str,
	check: &'a str,
	observed: CheckResult,
	detail: serde_json::Value,
) -> CheckFiling<'a> {
	CheckFiling {
		source,
		scope: Scope::Application(server_id),
		device_id: None,
		check,
		observed,
		title: None,
		message: "fleet check detail test",
		detail: Some(detail),
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

// spec: FIG#fleet-spread
#[tokio::test(flavor = "multi_thread")]
async fn detail_is_keyed_by_check_with_the_graded_result() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let one = insert_server(&mut conn, "http://fleet-one.invalid/").await;
		let two = insert_server(&mut conn, "http://fleet-two.invalid/").await;

		file_check(
			&mut conn,
			filing(
				one,
				"alertd",
				"diskspace",
				CheckResult::Warning,
				json!({"check": "diskspace", "result": "warning", "percent": 91, "path": "/"}),
			),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(
				two,
				"alertd",
				"diskspace",
				CheckResult::Passed,
				json!({"check": "diskspace", "result": "passed", "percent": 12, "path": "/"}),
			),
		)
		.await
		.expect("file");

		let detail = check_detail_by_server(&mut conn, &[(one, None), (two, None)])
			.await
			.expect("check detail");

		assert_eq!(detail[&one]["diskspace"]["percent"], json!(91));
		assert_eq!(detail[&two]["diskspace"]["percent"], json!(12));
		// The graded state wins over the `result` the source put in its own
		// detail, and the observation behind it is available beside it.
		assert_eq!(detail[&one]["diskspace"]["result"], json!("warning"));
		assert_eq!(detail[&one]["diskspace"]["observed"], json!("warning"));
	})
	.await
}

/// A server with no check state at all is simply absent — the fleet view
/// counts it in the unreported group rather than inventing empty checks.
// spec: FIG#fleet-spread
#[tokio::test(flavor = "multi_thread")]
async fn server_without_check_state_is_absent() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let quiet = insert_server(&mut conn, "http://fleet-quiet.invalid/").await;

		let detail = check_detail_by_server(&mut conn, &[(quiet, None)])
			.await
			.expect("check detail");

		assert!(!detail.contains_key(&quiet));
	})
	.await
}

/// The fleet reads a check exactly as the server's own check list does: a
/// silenced check reads as skipped, a decommissioned one doesn't present.
// spec: FIG#fleet-spread
#[tokio::test(flavor = "multi_thread")]
async fn silenced_reads_skipped_and_decommissioned_is_absent() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://fleet-hushed.invalid/").await;

		file_check(
			&mut conn,
			filing(
				server_id,
				"alertd",
				"hushed",
				CheckResult::Failed,
				json!({"percent": 99}),
			),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(
				server_id,
				"alertd",
				"gone",
				CheckResult::Warning,
				json!({"percent": 50}),
			),
		)
		.await
		.expect("file");

		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Application(server_id),
			"alertd",
			&app_ns(),
			"hushed",
			None,
			Some("op"),
		)
		.await
		.expect("silence");
		sql_query(
			"UPDATE check_policies SET decommissioned_at = now() \
			 WHERE source = 'alertd' AND check_name = 'gone'",
		)
		.execute(&mut conn)
		.await
		.expect("decommission");

		let detail = check_detail_by_server(&mut conn, &[(server_id, None)])
			.await
			.expect("check detail");

		let checks = &detail[&server_id];
		assert_eq!(checks["hushed"]["result"], json!("skipped"));
		// The silence caps the grading, not the report: what was observed
		// stays readable, and so do the check's own fields.
		assert_eq!(checks["hushed"]["observed"], json!("failed"));
		assert_eq!(checks["hushed"]["percent"], json!(99));
		assert!(
			!checks.contains_key("gone"),
			"a decommissioned check doesn't present in the fleet view either",
		);
	})
	.await
}

/// Two sources reporting the same check name present as one check, the more
/// recently reported field winning — the same rule the server-wide figures
/// resolve by.
// spec: FIG#fleet-spread
#[tokio::test(flavor = "multi_thread")]
async fn same_check_name_from_two_sources_merges_newest_first() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://fleet-merge.invalid/").await;

		file_check(
			&mut conn,
			filing(
				server_id,
				"alertd",
				"sync",
				CheckResult::Passed,
				json!({"lagSecs": 30, "onlyAlertd": true}),
			),
		)
		.await
		.expect("file");
		file_check(
			&mut conn,
			filing(
				server_id,
				"tamanu",
				"sync",
				CheckResult::Warning,
				json!({"lagSecs": 900}),
			),
		)
		.await
		.expect("file");
		// alertd reported first; make that explicit rather than relying on
		// the filings landing in distinguishable instants.
		sql_query(
			"UPDATE issues SET updated_at = now() - INTERVAL '1 hour' \
			 WHERE application_id = $1 AND source = 'alertd'",
		)
		.bind::<sql_types::Uuid, _>(server_id)
		.execute(&mut conn)
		.await
		.expect("age alertd's report");

		let detail = check_detail_by_server(&mut conn, &[(server_id, None)])
			.await
			.expect("check detail");

		let sync = &detail[&server_id]["sync"];
		assert_eq!(sync["lagSecs"], json!(900), "the newer report wins");
		assert_eq!(sync["result"], json!("warning"));
		assert_eq!(
			sync["onlyAlertd"],
			json!(true),
			"a field only the older source reports is still readable",
		);
	})
	.await
}
