//! Recording a migration test and reading the verdict it produces.

use commons_tests::db::TestDb;
use commons_types::backup::{BackupType, RestoreIntent, RunOutcome};
use database::{
	migration_tests::{MigrationTest, NewMigrationTest, Verdict, verdict},
	pg_duration::PgDuration,
	restore::NewBackupRestoreCheck,
	version_known_issues::VersionKnownIssue,
	versions::{NewVersion, Version},
};
use diesel::{OptionalExtension, QueryableByName, SelectableHelper, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_group(conn: &mut AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO server_groups (name) VALUES ('kamaka') RETURNING id")
		.get_result(conn)
		.await
		.expect("group");
	row.id
}

/// A machine and the one application on it, as `(machine, application)`. A
/// report is about the machine's snapshot; the test is about the application's
/// candidate version.
async fn insert_server(conn: &mut AsyncPgConnection, group_id: Uuid) -> (Uuid, Uuid) {
	let machine: RowId =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Uuid, _>(group_id)
			.get_result(conn)
			.await
			.expect("machine");
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, rank, group_id, machine_id) VALUES ('tamanu-central', $1, 'production', $2, $3) RETURNING id",
	)
	.bind::<sql_types::Text, _>("https://central.kamaka.example")
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Uuid, _>(machine.id)
	.get_result(conn)
	.await
	.expect("server");
	(machine.id, row.id)
}

async fn insert_consumer(conn: &mut AsyncPgConnection) -> Uuid {
	let row: RowId = sql_query("INSERT INTO devices (role) VALUES ('backup-restore') RETURNING id")
		.get_result(conn)
		.await
		.expect("consumer");
	row.id
}

async fn insert_version(conn: &mut AsyncPgConnection, minor: i32) -> Version {
	diesel::insert_into(database::schema::versions::table)
		.values(NewVersion {
			major: 2,
			minor,
			patch: 0,
			status: commons_types::version::VersionStatus::Published,
			changelog: String::new(),
			device_id: None,
		})
		.returning(Version::as_returning())
		.get_result(conn)
		.await
		.expect("version")
}

fn report(
	consumer: Uuid,
	group: Uuid,
	machine: Uuid,
	outcome: RunOutcome,
) -> NewBackupRestoreCheck {
	NewBackupRestoreCheck {
		replica_id: None,
		replica_name: None,
		consumer_device_id: consumer,
		group_id: group,
		machine_id: Some(machine),
		r#type: BackupType::TamanuPostgres,
		intent: RestoreIntent::from("migration-test"),
		snapshot_id: Some("snap-x".into()),
		outcome,
		error: None,
		replica_healthy: true,
		postgres_version: Some("18".into()),
		observed_at: Timestamp::now(),
		s3_sent_raw_bytes: None,
		s3_sent_payload_bytes: None,
		s3_received_raw_bytes: None,
		s3_received_payload_bytes: None,
		health_details: None,
		run_id: None,
		redaction_outcome: None,
		redaction_manifest_version: None,
		redaction_columns_masked: None,
		redaction_columns_skipped: None,
		redaction_error: None,
	}
}

fn secs(n: i64) -> PgDuration {
	PgDuration(SignedDuration::from_secs(n))
}

#[tokio::test(flavor = "multi_thread")]
async fn a_passing_test_records_timings_in_order() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		let check_id = MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Success),
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(900),
				failed_migration: None,
				error: None,
				data_bytes_before: 200_000_000_000,
				data_bytes_after: 260_000_000_000,
				timings: vec![
					("addIndexToFhirJobs".into(), secs(12)),
					("backfillNoteTypeIds".into(), secs(880)),
				],
			},
		)
		.await
		.expect("record");

		assert_eq!(
			verdict(&mut conn, machine, target.id)
				.await
				.expect("verdict"),
			Verdict::Passed
		);

		let timings = MigrationTest::timings(&mut conn, check_id)
			.await
			.expect("timings");
		let names: Vec<&str> = timings.iter().map(|t| t.name.as_str()).collect();
		assert_eq!(
			names,
			vec!["addIndexToFhirJobs", "backfillNoteTypeIds"],
			"kept in the order they ran"
		);
		assert_eq!(
			timings[1].elapsed,
			secs(880),
			"the slow one is attributable"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_named_failing_migration_is_a_failed_verdict() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Failure),
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(45),
				failed_migration: Some("backfillNoteTypeIds".into()),
				error: None,
				data_bytes_before: 200_000_000_000,
				data_bytes_after: 200_000_000_000,
				timings: vec![("backfillNoteTypeIds".into(), secs(45))],
			},
		)
		.await
		.expect("record");

		assert_eq!(
			verdict(&mut conn, machine, target.id)
				.await
				.expect("verdict"),
			Verdict::Failed
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untested_pair_and_an_untested_version() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let tested = insert_version(&mut conn, 63).await;
		let untested = insert_version(&mut conn, 64).await;

		assert_eq!(
			verdict(&mut conn, server, tested.id)
				.await
				.expect("verdict"),
			Verdict::NotTested,
			"nothing reported yet"
		);

		MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Success),
			NewMigrationTest {
				application_id: server,
				target_version_id: tested.id,
				total_elapsed: secs(10),
				failed_migration: None,
				error: None,
				data_bytes_before: 1,
				data_bytes_after: 1,
				timings: vec![],
			},
		)
		.await
		.expect("record");

		assert_eq!(
			verdict(&mut conn, server, untested.id)
				.await
				.expect("verdict"),
			Verdict::NotTested,
			"one version's pass says nothing about another's"
		);
	})
	.await
}

#[derive(QueryableByName)]
struct FiledCheck {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	observed: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	effective: Option<String>,
	#[diesel(sql_type = sql_types::Bool)]
	escalates: bool,
}

/// The server's migration-test check as it stands.
///
/// Sweeps first: `sweep_restore_checks` is the sole filer of the restore checks, so a
/// recorded verdict reaches the check on the next pass rather than at the
/// moment it is recorded (see `BackupRestoreCheck::record_report`).
async fn migration_check(conn: &mut AsyncPgConnection, server: Uuid) -> Option<FiledCheck> {
	database::restore::sweep_restore_checks(conn)
		.await
		.expect("sweep");
	sql_query(
		"SELECT i.observed_result AS observed, i.effective_result AS effective, i.escalates
		 FROM issues i
		 WHERE i.application_id = $1 AND i.ref = 'migration-test' AND i.active = true",
	)
	.bind::<sql_types::Uuid, _>(server)
	.get_result(conn)
	.await
	.optional()
	.expect("read filed check")
}

async fn unresolved_issues_for(conn: &mut AsyncPgConnection, server: Uuid) -> i64 {
	#[derive(QueryableByName)]
	struct Count {
		#[diesel(sql_type = sql_types::BigInt)]
		count: i64,
	}
	sql_query(
		"SELECT count(*) AS count FROM version_known_issues
		 WHERE application_id = $1 AND resolved_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(server)
	.get_result::<Count>(conn)
	.await
	.expect("count known issues")
	.count
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failure_warns_on_the_server_and_holds_the_version_back() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Success),
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(45),
				failed_migration: Some("backfillNoteTypeIds".into()),
				error: None,
				data_bytes_before: 10,
				data_bytes_after: 10,
				timings: vec![],
			},
		)
		.await
		.expect("record failure");

		let filed = migration_check(&mut conn, server)
			.await
			.expect("a check was filed");
		assert_eq!(
			filed.observed.as_deref(),
			Some("warning"),
			"a warning, not a failure"
		);
		assert_eq!(
			filed.effective.as_deref(),
			Some("warning"),
			"and the policy ceiling keeps it there"
		);
		assert!(!filed.escalates, "does not page against a healthy server");

		assert!(
			!VersionKnownIssue::version_is_ready(&mut conn, 2, 63, 0)
				.await
				.expect("readiness"),
			"the version is held back from rollout"
		);
		assert_eq!(unresolved_issues_for(&mut conn, server).await, 1);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn tomorrows_snapshot_does_not_file_the_issue_twice() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		for snapshot in ["snap-1", "snap-2"] {
			let mut failing = report(consumer, group, machine, RunOutcome::Success);
			failing.snapshot_id = Some(snapshot.into());
			MigrationTest::record(
				&mut conn,
				failing,
				NewMigrationTest {
					application_id: server,
					target_version_id: target.id,
					total_elapsed: secs(45),
					failed_migration: Some("backfillNoteTypeIds".into()),
					error: None,
					data_bytes_before: 10,
					data_bytes_after: 10,
					timings: vec![],
				},
			)
			.await
			.expect("record failure");
		}

		assert_eq!(
			unresolved_issues_for(&mut conn, server).await,
			1,
			"the same version failing again is the same finding"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_later_pass_recovers_the_check() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		let mut failing = report(consumer, group, machine, RunOutcome::Success);
		failing.snapshot_id = Some("snap-1".into());
		MigrationTest::record(
			&mut conn,
			failing,
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(45),
				failed_migration: Some("backfillNoteTypeIds".into()),
				error: None,
				data_bytes_before: 10,
				data_bytes_after: 10,
				timings: vec![],
			},
		)
		.await
		.expect("record failure");
		assert!(migration_check(&mut conn, server).await.is_some());

		let mut passing = report(consumer, group, machine, RunOutcome::Success);
		passing.snapshot_id = Some("snap-2".into());
		MigrationTest::record(
			&mut conn,
			passing,
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(50),
				failed_migration: None,
				error: None,
				data_bytes_before: 10,
				data_bytes_after: 12,
				timings: vec![],
			},
		)
		.await
		.expect("record pass");

		assert!(
			migration_check(&mut conn, server).await.is_none(),
			"the check recovers once the migrations apply"
		);
	})
	.await
}

/// A `migrate` declaration plus the capability that advertises it, so the
/// overdue sweep has something to walk.
async fn declare_migrate(
	conn: &mut AsyncPgConnection,
	consumer: Uuid,
	group: Uuid,
	overdue_seconds: i64,
) {
	sql_query(
		"INSERT INTO restore_consumer_capabilities (consumer_device_id, intent, description, semantics, params)
		 VALUES ($1, 'migrate', '', '[\"check\",\"once\",\"migrate\"]'::jsonb, '[]'::jsonb)",
	)
	.bind::<sql_types::Uuid, _>(consumer)
	.execute(conn)
	.await
	.expect("register capability");

	sql_query(
		"INSERT INTO restore_replicas
		 (consumer_device_id, group_id, type, intent, name, overdue_after, params)
		 VALUES ($1, $2, 'tamanu-postgres', 'migrate', 'kamaka-migrate', make_interval(secs => $3), '{}'::jsonb)",
	)
	.bind::<sql_types::Uuid, _>(consumer)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Double, _>(overdue_seconds as f64)
	.execute(conn)
	.await
	.expect("declare replica");
}

/// A successful backup run, which is the snapshot a migration test would use.
///
/// Takes the *machine*: a run captures a box's data.
// spec: BAK
async fn record_snapshot(
	conn: &mut AsyncPgConnection,
	consumer: Uuid,
	group: Uuid,
	machine: Uuid,
	snapshot: &str,
	age_seconds: i64,
) {
	sql_query(
		"INSERT INTO backup_runs
		 (id, device_id, group_id, machine_id, type, purpose, outcome, snapshot_id, reported_at)
		 VALUES (gen_random_uuid(), $1, $2, $3, 'tamanu-postgres', 'backup', 'success', $4,
		         NOW() - make_interval(secs => $5))",
	)
	.bind::<sql_types::Uuid, _>(consumer)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Uuid, _>(machine)
	.bind::<sql_types::Text, _>(snapshot)
	.bind::<sql_types::Double, _>(age_seconds as f64)
	.execute(conn)
	.await
	.expect("record backup run");
}

/// The production environment's open plan, which is what names the version to
/// migrate to.
async fn plan_upgrade(conn: &mut AsyncPgConnection, group: Uuid, target: &Version) {
	sql_query(
		"INSERT INTO upgrade_plans (group_id, rank, target_version_id, created_by)
		 VALUES ($1, 'production', $2, 'test@example.com')",
	)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Uuid, _>(target.id)
	.execute(conn)
	.await
	.expect("plan upgrade");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_untried_candidate_goes_overdue_and_a_tested_one_does_not() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		declare_migrate(&mut conn, consumer, group, 3600).await;
		record_snapshot(&mut conn, consumer, group, machine, "snap-old", 7200).await;

		let filed = database::restore::sweep_restore_checks(&mut conn)
			.await
			.expect("sweep");
		assert_eq!(filed, 1, "the candidate has gone untried past the bound");
		let check = migration_check(&mut conn, server)
			.await
			.expect("a check was filed");
		assert_eq!(check.observed.as_deref(), Some("warning"));
		assert!(!check.escalates);

		// Once it has a verdict for that snapshot, it is no longer overdue.
		let mut passing = report(consumer, group, machine, RunOutcome::Success);
		passing.snapshot_id = Some("snap-old".into());
		MigrationTest::record(
			&mut conn,
			passing,
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(10),
				failed_migration: None,
				error: None,
				data_bytes_before: 1,
				data_bytes_after: 1,
				timings: vec![],
			},
		)
		.await
		.expect("record pass");

		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			0,
			"a tried pair is not overdue"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_group_shows_where_each_server_stands() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (tested, tested_app) = insert_server(&mut conn, group).await;
		let (_untested, untested_app) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;

		let mut failing = report(consumer, group, tested, RunOutcome::Success);
		failing.snapshot_id = Some("snap-1".into());
		MigrationTest::record(
			&mut conn,
			failing,
			NewMigrationTest {
				application_id: tested_app,
				target_version_id: target.id,
				total_elapsed: secs(3600),
				failed_migration: Some("backfillNoteTypeIds".into()),
				error: None,
				data_bytes_before: 200,
				data_bytes_after: 260,
				timings: vec![
					("addIndexToFhirJobs".into(), secs(12)),
					("backfillNoteTypeIds".into(), secs(3588)),
				],
			},
		)
		.await
		.expect("record failure");

		let verdicts = database::migration_tests::verdicts_for_group(&mut conn, group)
			.await
			.expect("verdicts");

		assert_eq!(verdicts.len(), 2, "one row per server the plan covers");
		let by_server: std::collections::HashMap<Uuid, _> =
			verdicts.into_iter().map(|v| (v.server_id, v)).collect();

		let failed = &by_server[&tested_app];
		assert_eq!(failed.verdict, database::migration_tests::Verdict::Failed);
		assert_eq!(failed.target_version, "2.63.0");
		let latest = failed.latest.as_ref().expect("a test was reported");
		assert_eq!(latest.snapshot_id.as_deref(), Some("snap-1"));
		assert_eq!(
			latest.failed_migration.as_deref(),
			Some("backfillNoteTypeIds")
		);
		assert_eq!(
			latest.data_bytes_after - latest.data_bytes_before,
			60,
			"growth is readable from the verdict"
		);
		let names: Vec<&str> = latest.timings.iter().map(|t| t.name.as_str()).collect();
		assert_eq!(
			names,
			vec!["addIndexToFhirJobs", "backfillNoteTypeIds"],
			"the breakdown comes back in the order the migrations ran"
		);
		assert_eq!(latest.timings[1].elapsed, secs(3588));

		let pending = &by_server[&untested_app];
		assert_eq!(
			pending.verdict,
			database::migration_tests::Verdict::NotTested
		);
		assert!(pending.latest.is_none());
	})
	.await
}

async fn restore_check(conn: &mut AsyncPgConnection, server: Uuid) -> Option<FiledCheck> {
	sql_query(
		"SELECT i.observed_result AS observed, i.effective_result AS effective, i.escalates
		 FROM issues i
		 WHERE i.application_id = $1 AND i.ref = 'restore-verification' AND i.active = true",
	)
	.bind::<sql_types::Uuid, _>(server)
	.get_result(conn)
	.await
	.optional()
	.expect("read restore check")
}

/// One restore, two answers. A `migrate` semantic riding on a verifying intent
/// means a single report says both "the backup restores" and "this version's
/// migrations do not survive the data", and those must not contaminate each
/// other: the backup is fine, the version is not.
#[tokio::test(flavor = "multi_thread")]
async fn one_report_keeps_backup_health_and_version_readiness_apart() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		// The restore succeeded into a healthy replica; the migrations then failed.
		MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Success),
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(45),
				failed_migration: Some("backfillNoteTypeIds".into()),
				error: None,
				data_bytes_before: 10,
				data_bytes_after: 10,
				timings: vec![],
			},
		)
		.await
		.expect("record");

		assert!(
			restore_check(&mut conn, server).await.is_none(),
			"the backup restored, so restore-health raises nothing"
		);

		let migration = migration_check(&mut conn, server)
			.await
			.expect("the migration finding stands on its own");
		assert_eq!(migration.observed.as_deref(), Some("warning"));

		assert!(
			!VersionKnownIssue::version_is_ready(&mut conn, 2, 63, 0)
				.await
				.expect("readiness"),
			"and it is the version that is held back, not the server"
		);
	})
	.await
}

/// A restore that failed before migrating says nothing about the version: no
/// verdict lands, the pair stays retryable, and no check files either way.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_restore_leaves_the_version_unjudged() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Failure),
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(0),
				failed_migration: None,
				error: None,
				data_bytes_before: 0,
				data_bytes_after: 0,
				timings: vec![],
			},
		)
		.await
		.expect("record");

		assert_eq!(
			database::migration_tests::verdict(&mut conn, machine, target.id)
				.await
				.expect("verdict"),
			database::migration_tests::Verdict::NotTested,
			"retryable: the migrations never ran"
		);
		assert!(
			migration_check(&mut conn, server).await.is_none(),
			"neither a pass nor a warning is filed"
		);
	})
	.await
}

/// A verdict is a fact about a candidate version measured against a group's
/// data, so it surfaces on its own: the report carries no replica reference, no
/// declaration asks for the replica it came from, and there is no overdue bound
/// anywhere. Deriving the check from declarations alone lost this outright.
#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_with_no_declaration_still_surfaces() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;

		let unlinked = report(consumer, group, machine, RunOutcome::Success);
		assert!(
			unlinked.replica_id.is_none(),
			"nothing links it to a replica"
		);
		MigrationTest::record(
			&mut conn,
			unlinked,
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(45),
				failed_migration: Some("backfillNoteTypeIds".into()),
				error: None,
				data_bytes_before: 10,
				data_bytes_after: 10,
				timings: vec![],
			},
		)
		.await
		.expect("record failure");

		#[derive(QueryableByName)]
		struct Count {
			#[diesel(sql_type = sql_types::BigInt)]
			count: i64,
		}
		let declarations: Count = sql_query("SELECT count(*) AS count FROM restore_replicas")
			.get_result(&mut conn)
			.await
			.expect("count declarations");
		assert_eq!(declarations.count, 0, "and no declaration asks for one");

		let filed = migration_check(&mut conn, server)
			.await
			.expect("the verdict alone raises the check");
		assert_eq!(filed.observed.as_deref(), Some("warning"));
	})
	.await
}

/// A second application at `rank` on its own box in the group, as `(machine,
/// application)`.
async fn insert_server_at(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	rank: &str,
	r#type: &str,
	machine: Option<Uuid>,
) -> (Uuid, Uuid) {
	let machine = match machine {
		Some(machine) => machine,
		None => {
			let row: RowId =
				sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
					.bind::<sql_types::Uuid, _>(group_id)
					.get_result(conn)
					.await
					.expect("machine");
			row.id
		}
	};
	let row: RowId = sql_query(
		"INSERT INTO applications (type, host, rank, group_id, machine_id) VALUES ($1, $2, $3, $4, $5) RETURNING id",
	)
	.bind::<sql_types::Text, _>(r#type)
	.bind::<sql_types::Text, _>(format!("https://{}.kamaka.example", Uuid::new_v4()))
	.bind::<sql_types::Text, _>(rank)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result(conn)
	.await
	.expect("server");
	(machine, row.id)
}

async fn plan_upgrade_at(conn: &mut AsyncPgConnection, group: Uuid, rank: &str, target: &Version) {
	sql_query(
		"INSERT INTO upgrade_plans (group_id, rank, target_version_id, created_by)
		 VALUES ($1, $2, $3, 'test@example.com')",
	)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Text, _>(rank)
	.bind::<sql_types::Uuid, _>(target.id)
	.execute(conn)
	.await
	.expect("plan upgrade");
}

#[allow(clippy::too_many_arguments)]
async fn record_test_at(
	conn: &mut AsyncPgConnection,
	consumer: Uuid,
	group: Uuid,
	machine: Uuid,
	application: Uuid,
	target: &Version,
	snapshot: &str,
	outcome: RunOutcome,
	failed_migration: Option<&str>,
	hours_ago: i64,
) {
	let mut check = report(consumer, group, machine, outcome);
	check.snapshot_id = Some(snapshot.into());
	check.observed_at = Timestamp::now() - SignedDuration::from_hours(hours_ago);
	MigrationTest::record(
		conn,
		check,
		NewMigrationTest {
			application_id: application,
			target_version_id: target.id,
			total_elapsed: secs(10),
			failed_migration: failed_migration.map(Into::into),
			error: None,
			data_bytes_before: 1,
			data_bytes_after: 1,
			timings: vec![],
		},
	)
	.await
	.expect("record test");
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

/// Whether an ask against an open plan for `target` is held for `machine`.
async fn pending(conn: &mut AsyncPgConnection, machine: Uuid, target: &Version) -> bool {
	let row: Count = sql_query(
		"SELECT COUNT(*) AS n FROM migration_test_requests r
		 JOIN upgrade_plans p ON p.id = r.plan_id
		 WHERE r.machine_id = $1 AND p.target_version_id = $2
		   AND p.met_at IS NULL AND p.superseded_at IS NULL AND p.withdrawn_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(machine)
	.bind::<sql_types::Uuid, _>(target.id)
	.get_result(conn)
	.await
	.expect("pending");
	row.n > 0
}

async fn ask(conn: &mut AsyncPgConnection, group: Uuid, rank: ServerRank) -> usize {
	database::migration_tests::MigrationTestRequest::request_environment(conn, group, rank, None)
		.await
		.expect("ask")
		.len()
}

async fn on_request(conn: &mut AsyncPgConnection) {
	sql_query("UPDATE restore_replicas SET migrates_on_request = TRUE")
		.execute(conn)
		.await
		.expect("on request");
}

use commons_types::server::rank::ServerRank;

async fn open_production(
	conn: &mut AsyncPgConnection,
	group: Uuid,
) -> database::upgrade_plans::UpgradePlan {
	database::upgrade_plans::UpgradePlan::open_for_environment(conn, group, ServerRank::Production)
		.await
		.expect("plan")
		.expect("open")
}

#[tokio::test(flavor = "multi_thread")]
async fn a_plan_starts_at_its_window_or_at_midnight_utc() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn).await;
		insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;

		sql_query("UPDATE upgrade_plans SET planned_for = '2026-11-02'")
			.execute(&mut conn)
			.await
			.expect("date");
		let plan = open_production(&mut conn, group).await;
		assert_eq!(
			database::upgrade_plans::planned_start(&plan),
			Some("2026-11-02T00:00:00Z".parse().unwrap()),
			"a date alone starts at midnight UTC"
		);

		sql_query(
			"UPDATE upgrade_plans SET planned_time = '22:30', planned_zone = 'Pacific/Auckland'",
		)
		.execute(&mut conn)
		.await
		.expect("time");
		let plan = open_production(&mut conn, group).await;
		assert_eq!(
			database::upgrade_plans::planned_start(&plan),
			Some("2026-11-02T09:30:00Z".parse().unwrap()),
			"a recorded hour is the window's opening, in its zone"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn on_the_schedule_overdue_runs_from_when_the_pair_fell_due() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		declare_migrate(&mut conn, consumer, group, 3600).await;
		record_snapshot(&mut conn, consumer, group, machine, "snap-old", 30 * 86400).await;
		record_test_at(
			&mut conn,
			consumer,
			group,
			machine,
			server,
			&target,
			"snap-old",
			RunOutcome::Success,
			None,
			3 * 24,
		)
		.await;
		record_snapshot(&mut conn, consumer, group, machine, "snap-new", 7200).await;

		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			0,
			"tested three days ago, so the newer snapshot is not due"
		);

		sql_query("UPDATE backup_restore_checks SET reported_at = NOW() - INTERVAL '8 days'")
			.execute(&mut conn)
			.await
			.expect("age the test");
		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			1,
			"due a day ago, past the hour's bound"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn asking_does_not_quiet_a_test_already_overdue() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, _) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		declare_migrate(&mut conn, consumer, group, 3600).await;
		record_snapshot(&mut conn, consumer, group, machine, "snap-old", 7200).await;
		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			1,
			"untried two hours, past the hour's bound"
		);

		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 1);
		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			1,
			"still owed from before the ask"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn on_request_is_overdue_only_for_an_ask() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, _) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		declare_migrate(&mut conn, consumer, group, 3600).await;
		on_request(&mut conn).await;
		record_snapshot(&mut conn, consumer, group, machine, "snap-old", 7200).await;

		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			0,
			"nobody asked"
		);

		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 1);
		sql_query("UPDATE migration_test_requests SET requested_at = NOW() - INTERVAL '2 hours'")
			.execute(&mut conn)
			.await
			.expect("age the ask");
		assert_eq!(
			database::restore::sweep_restore_checks(&mut conn)
				.await
				.expect("sweep"),
			1,
			"an ask unanswered past the bound"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_waits_for_a_test_begun_after_it() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 1);

		record_test_at(
			&mut conn,
			consumer,
			group,
			machine,
			server,
			&target,
			"snap-1",
			RunOutcome::Success,
			None,
			1,
		)
		.await;
		assert!(
			pending(&mut conn, machine, &target).await,
			"a test begun an hour before the ask does not answer it"
		);

		record_test_at(
			&mut conn,
			consumer,
			group,
			machine,
			server,
			&target,
			"snap-1",
			RunOutcome::Failure,
			None,
			0,
		)
		.await;
		assert!(
			pending(&mut conn, machine, &target).await,
			"a restore that failed before migrating leaves it standing"
		);

		sql_query(
			"UPDATE migration_test_requests SET requested_at = NOW() - INTERVAL '10 minutes'",
		)
		.execute(&mut conn)
		.await
		.expect("age the ask");
		record_test_at(
			&mut conn,
			consumer,
			group,
			machine,
			server,
			&target,
			"snap-1",
			RunOutcome::Success,
			None,
			0,
		)
		.await;
		assert!(!pending(&mut conn, machine, &target).await, "answered");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_run_already_under_way_does_not_answer_an_ask() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		let run = Uuid::new_v4();
		sql_query(
			"INSERT INTO backup_credential_issuances
				(device_id, group_id, type, purpose, issued_at, expires_at,
				 sts_assumed_role, bucket, prefix, run_id)
			 VALUES ($1, $2, 'tamanu-postgres', 'restore', NOW() - INTERVAL '3 hours',
				NOW() + INTERVAL '1 hour', 'arn:aws:iam::1:role/r', 'b', '', $3)",
		)
		.bind::<sql_types::Uuid, _>(consumer)
		.bind::<sql_types::Uuid, _>(group)
		.bind::<sql_types::Uuid, _>(run)
		.execute(&mut conn)
		.await
		.expect("issue credentials");
		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 1);

		let mut check = report(consumer, group, machine, RunOutcome::Success);
		check.snapshot_id = Some("snap-1".into());
		check.run_id = Some(run);
		MigrationTest::record(
			&mut conn,
			check,
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(10),
				failed_migration: None,
				error: None,
				data_bytes_before: 1,
				data_bytes_after: 1,
				timings: vec![],
			},
		)
		.await
		.expect("record test");

		assert!(
			pending(&mut conn, machine, &target).await,
			"the run began three hours before the ask, though it reported after"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn only_the_reporting_consumer_s_run_says_when_it_began() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let other = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		let run = Uuid::new_v4();
		sql_query(
			"INSERT INTO backup_credential_issuances
				(device_id, group_id, type, purpose, issued_at, expires_at,
				 sts_assumed_role, bucket, prefix, run_id)
			 VALUES ($1, $2, 'tamanu-postgres', 'restore', NOW() - INTERVAL '3 hours',
				NOW() + INTERVAL '1 hour', 'arn:aws:iam::1:role/r', 'b', '', $3)",
		)
		.bind::<sql_types::Uuid, _>(other)
		.bind::<sql_types::Uuid, _>(group)
		.bind::<sql_types::Uuid, _>(run)
		.execute(&mut conn)
		.await
		.expect("issue credentials to another consumer");
		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 1);
		sql_query("UPDATE migration_test_requests SET requested_at = NOW() - INTERVAL '1 hour'")
			.execute(&mut conn)
			.await
			.expect("age the ask");

		let mut check = report(consumer, group, machine, RunOutcome::Success);
		check.run_id = Some(run);
		MigrationTest::record(
			&mut conn,
			check,
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(10),
				failed_migration: None,
				error: None,
				data_bytes_before: 1,
				data_bytes_after: 1,
				timings: vec![],
			},
		)
		.await
		.expect("record test");
		assert!(
			!pending(&mut conn, machine, &target).await,
			"another consumer's issuance is not this run's start"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_report_with_no_run_began_its_elapsed_time_before() {
	TestDb::run(|mut conn, _url| async move {
		let consumer = insert_consumer(&mut conn).await;
		let group = insert_group(&mut conn).await;
		let (machine, server) = insert_server(&mut conn, group).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 1);
		sql_query(
			"UPDATE migration_test_requests SET requested_at = NOW() - INTERVAL '10 minutes'",
		)
		.execute(&mut conn)
		.await
		.expect("age the ask");

		MigrationTest::record(
			&mut conn,
			report(consumer, group, machine, RunOutcome::Success),
			NewMigrationTest {
				application_id: server,
				target_version_id: target.id,
				total_elapsed: secs(3600),
				failed_migration: None,
				error: None,
				data_bytes_before: 1,
				data_bytes_after: 1,
				timings: vec![],
			},
		)
		.await
		.expect("record test");
		assert!(
			pending(&mut conn, machine, &target).await,
			"an hour's run reported now began before the ask"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ask_covers_the_environment_s_tamanu_boxes() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn).await;
		assert_eq!(
			ask(&mut conn, group, ServerRank::Production).await,
			0,
			"no plan, nothing to ask for"
		);

		let (central, _) = insert_server(&mut conn, group).await;
		let (facility, _) =
			insert_server_at(&mut conn, group, "production", "tamanu-facility", None).await;
		let (other_product, _) =
			insert_server_at(&mut conn, group, "production", "senaite", None).await;
		let (clone, _) = insert_server_at(&mut conn, group, "clone", "tamanu-central", None).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		plan_upgrade_at(&mut conn, group, "clone", &target).await;

		assert_eq!(ask(&mut conn, group, ServerRank::Production).await, 2);
		assert!(pending(&mut conn, central, &target).await);
		assert!(pending(&mut conn, facility, &target).await);
		assert!(!pending(&mut conn, other_product, &target).await);
		assert!(
			!pending(&mut conn, clone, &target).await,
			"the clone is another environment"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_plan_s_asks_match_nothing() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn).await;
		let (machine, _) = insert_server(&mut conn, group).await;
		let (clone, _) = insert_server_at(&mut conn, group, "clone", "tamanu-central", None).await;
		let target = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &target).await;
		plan_upgrade_at(&mut conn, group, "clone", &target).await;
		ask(&mut conn, group, ServerRank::Production).await;
		ask(&mut conn, group, ServerRank::Clone).await;

		let plan = database::upgrade_plans::UpgradePlan::open_for_environment(
			&mut conn,
			group,
			ServerRank::Production,
		)
		.await
		.expect("plan")
		.expect("open");
		database::upgrade_plans::UpgradePlan::withdraw(&mut conn, plan.id, "ops@example.com")
			.await
			.expect("withdraw");

		assert!(!pending(&mut conn, machine, &target).await);
		assert!(
			pending(&mut conn, clone, &target).await,
			"the clone's plan still stands"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_box_is_tested_for_the_workload_an_ask_names() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn).await;
		let (machine, app) = insert_server(&mut conn, group).await;
		let production = insert_version(&mut conn, 63).await;
		plan_upgrade(&mut conn, group, &production).await;
		ask(&mut conn, group, ServerRank::Production).await;

		let applications =
			database::applications::Application::list_live_in_group(&mut conn, group)
				.await
				.expect("applications");
		let chosen = database::migration_tests::candidate_on_box(&mut conn, machine, &applications)
			.await
			.expect("candidate")
			.expect("one");
		assert_eq!(chosen.application.id, app);
		assert_eq!(chosen.version.id, production.id);
		assert!(chosen.request.is_some());
	})
	.await
}
