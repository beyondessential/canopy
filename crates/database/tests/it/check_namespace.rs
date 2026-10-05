//! The check namespace: a check's identity is its source, its namespace and
//! its name, so two application types reporting one name are two entries.
//!
//! Covers the three things that only hold across namespaces — a silence
//! reaching one and not the other, the migration's fan-out carrying the
//! review while re-deriving liveness, and a type reporting a name for the
//! first time landing unreviewed however well reviewed its siblings are.

use commons_types::{namespace::Namespace, server::app_type::ApplicationType, status::CheckResult};
use database::check_policies::{CheckPolicy, ScopedCheckPolicy};
use database::issues::{CheckFiling, Scope, consolidated_checks_latest, file_check};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str =
	include_str!("../../../../migrations/2026-09-02-080628-0000_check_namespace/up.sql");
const DOWN: &str =
	include_str!("../../../../migrations/2026-09-02-080628-0000_check_namespace/down.sql");

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn machine(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	row.id
}

async fn application(
	conn: &mut diesel_async::AsyncPgConnection,
	machine_id: Uuid,
	ty: &str,
	group_id: Option<Uuid>,
) -> Uuid {
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, machine_id, group_id) \
		 VALUES ($1, $2, $3, $4) RETURNING id",
	)
	.bind::<sql_types::Text, _>(ty)
	.bind::<sql_types::Text, _>(format!("http://{ty}.namespace.invalid/"))
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
	.get_result(conn)
	.await
	.expect("insert application");
	row.id
}

fn filing(application_id: Uuid, check: &str, observed: CheckResult) -> CheckFiling<'_> {
	CheckFiling {
		source: "alertd",
		scope: Scope::Application(application_id),
		device_id: None,
		check,
		observed,
		title: None,
		message: "namespace test",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

/// `postgres` is application-subject, so the type that reported it says which
/// entry it is.
// spec: CHK#names
#[tokio::test(flavor = "multi_thread")]
async fn a_group_silence_on_one_type_leaves_the_others_check_alerting() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('ns') RETURNING id")
			.get_result(&mut conn)
			.await
			.expect("group");
		let box_id = machine(&mut conn).await;
		let central = application(&mut conn, box_id, "tamanu-central", Some(group.id)).await;
		let facility = application(&mut conn, box_id, "tamanu-facility", Some(group.id)).await;

		for id in [central, facility] {
			file_check(&mut conn, filing(id, "postgres", CheckResult::Failed))
				.await
				.expect("file");
		}

		ScopedCheckPolicy::silence(
			&mut conn,
			Scope::Group(group.id),
			"alertd",
			&Namespace::Application(ApplicationType::TamanuCentral),
			"postgres",
			None,
			Some("op"),
		)
		.await
		.expect("silence the central's postgres for the group");

		let quiet = consolidated_checks_latest(&mut conn, central, Some(group.id))
			.await
			.expect("central");
		let quiet = quiet
			.checks
			.iter()
			.find(|c| c.check == "postgres")
			.expect("the central presents postgres");
		assert_eq!(quiet.effective, CheckResult::Skipped);
		assert!(quiet.silenced);

		let loud = consolidated_checks_latest(&mut conn, facility, Some(group.id))
			.await
			.expect("facility");
		let loud = loud
			.checks
			.iter()
			.find(|c| c.check == "postgres")
			.expect("the facility presents postgres");
		assert_eq!(
			loud.effective,
			CheckResult::Failed,
			"the silence names one type's check, so the other type's is untouched"
		);
		assert!(!loud.silenced);
	})
	.await
}

/// A type reporting a name nothing of its type has reported before mints its
/// own entry, and it is pending review whatever its siblings are graded at:
/// the operator vetted the check for the type they saw it on.
///
/// `upsert_default` is the device-push path (see `statuses::ingest`), which is
/// the only one that registers unreviewed; Canopy's own filings register
/// already vetted.
// spec: CHK#policy
#[tokio::test(flavor = "multi_thread")]
async fn a_new_namespace_registers_pending_review() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let central = Namespace::Application(ApplicationType::TamanuCentral);
		let facility = Namespace::Application(ApplicationType::TamanuFacility);

		CheckPolicy::upsert_default(&mut conn, "alertd", &central, "postgres")
			.await
			.expect("a central reports postgres");
		CheckPolicy::update(
			&mut conn,
			"alertd",
			&central,
			"postgres",
			CheckResult::Failed,
			false,
			None,
			"op",
		)
		.await
		.expect("review the central's postgres");

		CheckPolicy::upsert_default(&mut conn, "alertd", &facility, "postgres")
			.await
			.expect("a facility reports postgres for the first time");

		let reviewed = CheckPolicy::get(&mut conn, "alertd", &central, "postgres")
			.await
			.expect("get")
			.expect("the central's entry");
		assert!(reviewed.reviewed_at.is_some());
		assert_eq!(reviewed.ceiling, CheckResult::Failed);

		let fresh = CheckPolicy::get(&mut conn, "alertd", &facility, "postgres")
			.await
			.expect("get")
			.expect("the facility's entry");
		assert!(
			fresh.reviewed_at.is_none(),
			"reviewing one type's check does not vet another type's check of the same name"
		);
		assert_eq!(
			fresh.ceiling,
			CheckResult::Warning,
			"so it lands at the default ceiling rather than inheriting the graded one"
		);
	})
	.await
}

/// Retiring a check retires the entry, not the name: another type's check of
/// the same name keeps its catalog row and keeps presenting.
// spec: CHK#liveness-and-decommissioning
#[tokio::test(flavor = "multi_thread")]
async fn decommissioning_one_namespace_leaves_the_same_name_elsewhere_live() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let box_id = machine(&mut conn).await;
		let central = application(&mut conn, box_id, "tamanu-central", None).await;
		let facility = application(&mut conn, box_id, "tamanu-facility", None).await;

		for id in [central, facility] {
			file_check(&mut conn, filing(id, "postgres", CheckResult::Failed))
				.await
				.expect("file");
		}

		CheckPolicy::decommission(
			&mut conn,
			"alertd",
			&Namespace::Application(ApplicationType::TamanuCentral),
			"postgres",
			"op",
		)
		.await
		.expect("retire the central's postgres");

		let retired = consolidated_checks_latest(&mut conn, central, None)
			.await
			.expect("central");
		assert!(
			!retired.checks.iter().any(|c| c.check == "postgres"),
			"the retired entry stops presenting, and its state is resolved with it"
		);

		let live = consolidated_checks_latest(&mut conn, facility, None)
			.await
			.expect("facility");
		let live = live
			.checks
			.iter()
			.find(|c| c.check == "postgres")
			.expect("the facility's postgres is a different entry and still live");
		assert_eq!(live.effective, CheckResult::Failed);
	})
	.await
}

#[derive(QueryableByName)]
struct Entry {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	application_type: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>)]
	reviewed_at: Option<jiff_diesel::Timestamp>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>)]
	last_seen: Option<jiff_diesel::Timestamp>,
	#[diesel(sql_type = sql_types::Text)]
	ceiling: String,
}

/// Replays the fan-out for real: reverts the migration, seeds the catalog and
/// the states in the shape they had before it, then re-applies it.
// spec: CHK#names
#[tokio::test(flavor = "multi_thread")]
async fn the_fanout_carries_the_review_and_re_derives_liveness() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let box_id = machine(&mut conn).await;
		let central = application(&mut conn, box_id, "tamanu-central", None).await;
		let facility = application(&mut conn, box_id, "tamanu-facility", None).await;

		conn.batch_execute(DOWN).await.expect("revert");

		// One graded entry, as the pre-namespace catalog held it.
		conn.batch_execute(
			"INSERT INTO check_policies (source, check_name, ceiling, escalates, notes, \
			 reviewed_at, reviewed_by, first_seen, last_seen) \
			 VALUES ('alertd', 'postgres', 'failed', TRUE, 'graded before the split', \
			 NOW() - INTERVAL '30 days', 'op', NOW() - INTERVAL '90 days', NOW())",
		)
		.await
		.expect("seed the catalog");

		// The central reported it today; the facility last reported it in
		// March. Only the states say so, which is the point.
		for (id, days) in [(central, 0), (facility, 120)] {
			conn.batch_execute(&format!(
				"INSERT INTO issues (source, ref, message, active, check_name, \
				 application_id, first_seen, last_seen) \
				 VALUES ('alertd', 'postgres', 'seed', TRUE, 'postgres', '{id}', \
				 NOW() - INTERVAL '200 days', NOW() - INTERVAL '{days} days')"
			))
			.await
			.expect("seed a state");
		}

		conn.batch_execute(UP).await.expect("re-apply");

		let entries: Vec<Entry> = sql_query(
			"SELECT application_type, reviewed_at, last_seen, ceiling FROM check_policies \
			 WHERE source = 'alertd' AND check_name = 'postgres' ORDER BY application_type",
		)
		.load(&mut conn)
		.await
		.expect("the fanned-out entries");

		assert_eq!(
			entries
				.iter()
				.map(|e| e.application_type.as_deref())
				.collect::<Vec<_>>(),
			vec![Some("tamanu-central"), Some("tamanu-facility")],
			"one entry per type that reported the name"
		);
		for entry in &entries {
			assert_eq!(
				entry.ceiling, "failed",
				"the grading carries: a pending review would cap every vetted check at warning"
			);
			assert!(
				entry.reviewed_at.is_some(),
				"the review carries, having already applied to this fleet"
			);
		}

		// Liveness is re-derived per namespace rather than inherited, so the
		// facility's entry is a candidate for decommissioning and the
		// central's is not.
		let seen: Vec<jiff::Timestamp> = entries
			.iter()
			.map(|e| jiff::Timestamp::from(e.last_seen.expect("last_seen")))
			.collect();
		let day = 24 * 60 * 60;
		let ago = |t: jiff::Timestamp| jiff::Timestamp::now().as_second() - t.as_second();
		assert!(
			ago(seen[0]) < day,
			"the central's liveness is its own report, today"
		);
		assert!(
			ago(seen[1]) > 100 * day,
			"the facility's liveness is its own report, months ago, not the entry's"
		);
	})
	.await
}
