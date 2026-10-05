//! `issues::health_from_check_state` — the server health rollup over
//! current check state. Only *contributing* states count: an active,
//! unresolved effective failure is Unhealthy, but a resolved one, or one
//! whose check has been decommissioned, does not drag the server down.

use commons_types::issue::ResolvedReason;
use commons_types::status::{CheckResult, HealthState};
use database::issues::{CheckFiling, Issue, Scope, file_check, health_from_check_state};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId =
		sql_query("WITH m AS (INSERT INTO machines (name) VALUES ('box') RETURNING id) INSERT INTO applications (type, host, machine_id) SELECT 'tamanu-central', 'http://rollup.invalid/', m.id FROM m RETURNING id")
			.get_result(conn)
			.await
			.expect("insert server");
	row.id
}

fn failed_filing(server_id: Uuid, check: &str) -> CheckFiling<'_> {
	CheckFiling {
		source: "alertd",
		scope: Scope::Application(server_id),
		device_id: None,
		check,
		observed: CheckResult::Failed,
		title: Some("rollup test"),
		message: "rollup test filing",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

async fn health_of(conn: &mut diesel_async::AsyncPgConnection, server_id: Uuid) -> HealthState {
	health_from_check_state(conn, &[(server_id, None)])
		.await
		.expect("rollup")
		.get(&server_id)
		.copied()
		.unwrap_or(HealthState::Healthy)
}

#[tokio::test(flavor = "multi_thread")]
async fn active_failure_is_unhealthy() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(&mut conn, failed_filing(server_id, "boom"))
			.await
			.expect("file");
		assert_eq!(
			health_of(&mut conn, server_id).await,
			HealthState::Unhealthy
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn resolved_failure_does_not_count() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		let issue = file_check(&mut conn, failed_filing(server_id, "boom"))
			.await
			.expect("file");
		Issue::resolve(&mut conn, issue.id, "operator", ResolvedReason::Fixed)
			.await
			.expect("resolve");
		assert_eq!(
			health_of(&mut conn, server_id).await,
			HealthState::Healthy,
			"a resolved failure must not keep the server Unhealthy",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn decommissioned_check_does_not_count() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn).await;
		file_check(&mut conn, failed_filing(server_id, "boom"))
			.await
			.expect("file");
		sql_query(
			"UPDATE check_policies SET decommissioned_at = now() \
			 WHERE source = 'alertd' AND check_name = 'boom'",
		)
		.execute(&mut conn)
		.await
		.expect("decommission");
		assert_eq!(
			health_of(&mut conn, server_id).await,
			HealthState::Healthy,
			"a decommissioned check must not count toward health",
		);
	})
	.await
}
