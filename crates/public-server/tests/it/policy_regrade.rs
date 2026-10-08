//! A change to a reported check's policy re-grades its states at once, and the
//! incidents they are in follow the new grades.
//!
//! spec: CHK#policy, INC#membership, INC#notification

use commons_types::{namespace::Namespace, server::app_type::ApplicationType, status::CheckResult};
use database::check_policies::{CheckPolicy, IfLadder};
use database::issues::Incident;
use diesel::prelude::*;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde_json::{Value, json};
use uuid::Uuid;

const CHECK: &str = "disk_space";
const REF: &str = "health/disk_space";

fn ns() -> Namespace {
	Namespace::of("alertd", Some(&ApplicationType::TamanuCentral))
}

/// A production tamanu-central in a group, on its own machine, with the calling
/// device enrolled on that machine. Returns the application and group ids.
async fn central(conn: &mut AsyncPgConnection, device_id: Uuid) -> (Uuid, Uuid) {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'regrade-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	let id = Uuid::new_v4();
	sql_query(
		"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) \
		 VALUES ('box', $1, $3, $2) RETURNING id) \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) \
		 VALUES ($1, 'https://central.example.com', 'tamanu-central', $3, 'production', $1)",
	)
	.bind::<sql_types::Uuid, _>(id)
	.bind::<sql_types::Uuid, _>(device_id)
	.bind::<sql_types::Uuid, _>(group_id)
	.execute(conn)
	.await
	.expect("insert central");
	(id, group_id)
}

/// Save the check's policy as an operator does, which reviews it. The catalog
/// entry is made first if nothing has reported the check yet.
async fn save_policy(conn: &mut AsyncPgConnection, ceiling: CheckResult, escalates: bool) {
	CheckPolicy::upsert_default(conn, "alertd", &ns(), CHECK)
		.await
		.expect("ensure catalog row");
	CheckPolicy::update(
		conn,
		"alertd",
		&ns(),
		CHECK,
		ceiling,
		escalates,
		None,
		"ops",
	)
	.await
	.expect("save policy");
}

async fn push(
	public: &axum_test::TestServer,
	cert: &str,
	conn: &mut AsyncPgConnection,
	id: Uuid,
	body: Value,
) {
	public
		.post(&format!("/status/{id}"))
		.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
		.json(&body)
		.await
		.assert_status_ok();
	database::issues::process_incident_reeval_queue(conn, i64::MAX)
		.await
		.expect("drain incident reeval queue");
}

fn disk(result: &str) -> Value {
	json!({ "health": [{ "check": CHECK, "result": result, "free_gb": 2 }] })
}

#[derive(QueryableByName, Debug)]
struct State {
	#[diesel(sql_type = sql_types::Bool)]
	active: bool,
	#[diesel(sql_type = sql_types::Text)]
	message: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	description: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	effective_result: Option<String>,
}

async fn state(conn: &mut AsyncPgConnection, application_id: Uuid) -> State {
	sql_query(
		"SELECT active, message, description, effective_result FROM issues \
		 WHERE application_id = $1 AND source = 'alertd' AND ref = $2",
	)
	.bind::<sql_types::Uuid, _>(application_id)
	.bind::<sql_types::Text, _>(REF)
	.get_result(conn)
	.await
	.expect("check state")
}

async fn incidents(conn: &mut AsyncPgConnection, group_id: Uuid) -> Vec<Incident> {
	use database::schema::incidents::dsl;
	dsl::incidents
		.select(Incident::as_select())
		.filter(dsl::server_group_id.eq(group_id))
		.order(dsl::opened_at.asc())
		.load(conn)
		.await
		.expect("incidents")
}

async fn the_incident(conn: &mut AsyncPgConnection, group_id: Uuid) -> Incident {
	let mut all = incidents(conn, group_id).await;
	assert_eq!(all.len(), 1, "one incident on the environment");
	all.remove(0)
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

async fn opens_queued(conn: &mut AsyncPgConnection, incident_id: Uuid) -> i64 {
	sql_query(
		"SELECT count(*) AS n FROM slack_outbox WHERE incident_id = $1 AND kind = 'incident_open'",
	)
	.bind::<sql_types::Uuid, _>(incident_id)
	.get_result::<Count>(conn)
	.await
	.expect("count opens")
	.n
}

/// Lowering the ceiling of the only failure to a warning closes its incident at
/// once, the warning staying on the state, worded as a warning.
// spec: CHK#policy, INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn a_ceiling_lowered_to_warning_closes_the_incident() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (id, group) = central(&mut conn, device_id).await;
			save_policy(&mut conn, CheckResult::Failed, false).await;
			push(&public, &cert, &mut conn, id, disk("failed")).await;
			assert!(the_incident(&mut conn, group).await.closed_at.is_none());

			save_policy(&mut conn, CheckResult::Warning, false).await;

			let incident = the_incident(&mut conn, group).await;
			assert!(
				incident.closed_at.is_some(),
				"an operator's re-grade closes rather than lingering"
			);
			let state = state(&mut conn, id).await;
			assert_eq!(state.effective_result.as_deref(), Some("warning"));
			assert!(state.active, "a warning is still degraded");
			assert_eq!(
				state.description.as_deref(),
				Some("Health check 'disk_space' warned")
			);
		},
	)
	.await
}

/// Lowering the ceiling to a pass recovers the state at once and closes the
/// incident without waiting out the linger window.
// spec: CHK#policy, INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn a_ceiling_lowered_to_passed_closes_without_lingering() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (id, group) = central(&mut conn, device_id).await;
			save_policy(&mut conn, CheckResult::Failed, false).await;
			push(&public, &cert, &mut conn, id, disk("failed")).await;

			save_policy(&mut conn, CheckResult::Passed, false).await;

			let incident = the_incident(&mut conn, group).await;
			assert!(incident.closed_at.is_some());
			let state = state(&mut conn, id).await;
			assert_eq!(state.effective_result.as_deref(), Some("passed"));
			assert!(!state.active);
			assert!(state.description.is_none());
			assert_eq!(state.message, "Health check 'disk_space' recovered");
		},
	)
	.await
}

/// A rule reading the report's own fields is re-graded from the report the
/// state was last filed with, so a plain check's rule sees what it saw then.
// spec: CHK#policy
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_change_regrades_from_the_last_report() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (id, group) = central(&mut conn, device_id).await;
			save_policy(&mut conn, CheckResult::Failed, false).await;
			let mut body = disk("failed");
			body["region"] = json!("north");
			push(&public, &cert, &mut conn, id, body).await;
			assert!(the_incident(&mut conn, group).await.closed_at.is_none());

			let ladder: IfLadder = serde_json::from_value(json!({ "if": [
				{ "==": [{ "var": "status.region" }, "north"] }, "passed",
			] }))
			.expect("ladder");
			CheckPolicy::update_rules(&mut conn, "alertd", &ns(), CHECK, Some(&ladder), "ops")
				.await
				.expect("save rules");

			assert_eq!(
				state(&mut conn, id).await.effective_result.as_deref(),
				Some("passed"),
				"status.region reaches the rule on a re-grade",
			);
			assert!(the_incident(&mut conn, group).await.closed_at.is_some());
		},
	)
	.await
}

/// Reviewing a pending check lifts its warning cap, so a failure it was holding
/// down opens an incident when the review is saved.
// spec: CHK#policy, INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn reviewing_a_pending_check_opens_its_incident() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (id, group) = central(&mut conn, device_id).await;
			push(&public, &cert, &mut conn, id, disk("failed")).await;
			assert_eq!(
				state(&mut conn, id).await.effective_result.as_deref(),
				Some("warning"),
				"a pending check is capped at warning",
			);
			assert!(incidents(&mut conn, group).await.is_empty());

			save_policy(&mut conn, CheckResult::Failed, false).await;

			let state = state(&mut conn, id).await;
			assert_eq!(state.effective_result.as_deref(), Some("failed"));
			assert_eq!(
				state.description.as_deref(),
				Some("Health check 'disk_space' failed")
			);
			assert!(the_incident(&mut conn, group).await.closed_at.is_none());
		},
	)
	.await
}

/// A report lessening the last failure to a warning ends it as a recovery
/// does: the incident lingers, the warning staying in it.
// spec: INC#membership
#[tokio::test(flavor = "multi_thread")]
async fn a_report_lessening_the_last_failure_lingers() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (id, group) = central(&mut conn, device_id).await;
			save_policy(&mut conn, CheckResult::Failed, false).await;
			push(&public, &cert, &mut conn, id, disk("failed")).await;

			push(&public, &cert, &mut conn, id, disk("warning")).await;

			let incident = the_incident(&mut conn, group).await;
			assert!(incident.closed_at.is_none(), "still open while lingering");
			assert!(incident.closing_at.is_some(), "lingering");

			push(&public, &cert, &mut conn, id, disk("failed")).await;
			let incident = the_incident(&mut conn, group).await;
			assert!(
				incident.closing_at.is_none(),
				"the failure returning ends the lingering"
			);
		},
	)
	.await
}

/// Making a live failure escalating escalates its notified incident once, as an
/// escalating failure joining it would.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn escalating_a_live_failure_escalates_once() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let (id, group) = central(&mut conn, device_id).await;
			save_policy(&mut conn, CheckResult::Failed, false).await;
			push(&public, &cert, &mut conn, id, disk("failed")).await;
			let incident = the_incident(&mut conn, group).await;
			sql_query(
				"UPDATE slack_outbox SET delivered_at = NOW() \
				 WHERE incident_id = $1 AND kind = 'incident_open'",
			)
			.bind::<sql_types::Uuid, _>(incident.id)
			.execute(&mut conn)
			.await
			.expect("deliver the open");
			assert_eq!(opens_queued(&mut conn, incident.id).await, 1);

			save_policy(&mut conn, CheckResult::Failed, true).await;
			let incident = the_incident(&mut conn, group).await;
			assert!(incident.escalated_at.is_some());
			assert_eq!(opens_queued(&mut conn, incident.id).await, 2);

			save_policy(&mut conn, CheckResult::Failed, true).await;
			assert_eq!(
				opens_queued(&mut conn, incident.id).await,
				2,
				"saving the same policy again changes no grade"
			);
		},
	)
	.await
}
