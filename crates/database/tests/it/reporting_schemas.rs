//! Pair derivation and settling for reporting schemas.
//!
//! spec: RPT

use commons_tests::db::TestDb;
use database::{
	diesel_async::AsyncPgConnection,
	reporting_schemas::{
		NewReportingSchemaBuild, PairState, ReportingSchemaBuild, ReportingSchemaRequest,
		pairs_for_group, versions_for_group,
	},
	restore::NewBackupRestoreCheck,
};
use diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

const GROUP: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
const MACHINE: &str = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
const CENTRAL: &str = "cccccccc-cccc-cccc-cccc-cccccccccccc";
const FACILITY: &str = "dddddddd-dddd-dddd-dddd-dddddddddddd";
const CONSUMER: &str = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";

/// A group with a central on 2.60.0 and a facility on 2.59.0, both published.
async fn seed(conn: &mut AsyncPgConnection) -> (Uuid, Uuid) {
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{CONSUMER}', 'backup-restore');

		 INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
		 ('11111111-1111-1111-1111-111111111111', 2, 59, 0, '', 'published'),
		 ('22222222-2222-2222-2222-222222222222', 2, 60, 0, '', 'published');

		 INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka');

		 INSERT INTO machines (id, name, group_id) VALUES ('{MACHINE}', 'box', '{GROUP}');

		 INSERT INTO applications (id, type, name, host, machine_id, group_id) VALUES
		 ('{CENTRAL}', 'tamanu-central', 'central', 'https://c', '{MACHINE}', '{GROUP}'),
		 ('{FACILITY}', 'tamanu-facility', 'facility', 'https://f', '{MACHINE}', '{GROUP}');

		 INSERT INTO application_reported_detail (application_id, source, reported_at, version) VALUES
		 ('{CENTRAL}', 'tamanu', NOW(), '2.60.0'),
		 ('{FACILITY}', 'tamanu', NOW(), '2.59.0')",
	))
	.await
	.expect("seed");

	(
		"11111111-1111-1111-1111-111111111111".parse().unwrap(),
		"22222222-2222-2222-2222-222222222222".parse().unwrap(),
	)
}

fn group() -> Uuid {
	GROUP.parse().unwrap()
}

/// A restore report for the pair, as a consumer would send one.
fn report_for(
	outcome: commons_types::backup::RunOutcome,
	healthy: bool,
	error: Option<String>,
) -> NewBackupRestoreCheck {
	NewBackupRestoreCheck {
		replica_id: None,
		replica_name: None,
		consumer_device_id: CONSUMER.parse().unwrap(),
		group_id: group(),
		machine_id: Some(MACHINE.parse().unwrap()),
		r#type: "tamanu-postgres".parse().unwrap(),
		intent: "reporting-schema".parse().unwrap(),
		snapshot_id: Some("snap-1".to_owned()),
		outcome,
		error,
		replica_healthy: healthy,
		postgres_version: None,
		observed_at: jiff::Timestamp::now(),
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

/// Record a build against a throwaway restore report for the pair.
async fn record_build(conn: &mut AsyncPgConnection, version: Uuid, built: bool) {
	let report = NewBackupRestoreCheck {
		replica_id: None,
		replica_name: None,
		consumer_device_id: CONSUMER.parse().unwrap(),
		group_id: group(),
		machine_id: Some(MACHINE.parse().unwrap()),
		r#type: "tamanu-postgres".parse().unwrap(),
		intent: "reporting-schema".parse().unwrap(),
		snapshot_id: Some("snap-1".to_owned()),
		outcome: commons_types::backup::RunOutcome::Success,
		error: None,
		replica_healthy: true,
		postgres_version: None,
		observed_at: jiff::Timestamp::now(),
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
	};

	ReportingSchemaBuild::record(
		conn,
		report,
		NewReportingSchemaBuild {
			group_id: group(),
			version_id: version,
			application_id: Some(CENTRAL.parse().unwrap()),
			built,
			error: (!built).then(|| "views did not compile".to_owned()),
			artifact_ids: vec![],
		},
	)
	.await
	.expect("record build");
}

/// The pairs are every version the group's Tamanu applications report running.
/// A facility mid-rollout is on a different version from its central, and both
/// are pairs, because a schema follows the version rather than the application.
#[tokio::test(flavor = "multi_thread")]
async fn a_facility_on_its_own_version_is_a_pair_of_its_own() {
	TestDb::run(|mut conn, _url| async move {
		let (older, newer) = seed(&mut conn).await;

		let versions = versions_for_group(&mut conn, group())
			.await
			.expect("derive versions");
		let ids: Vec<Uuid> = versions.iter().map(|v| v.id).collect();

		assert!(ids.contains(&older), "the facility's version is a pair");
		assert!(ids.contains(&newer), "the central's version is a pair");
	})
	.await;
}

/// A pair with no build is awaiting one; a built pair is settled; a failed
/// build settles it as firmly, since a build against a fixed version and
/// configuration fails the same way every time.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_build_settles_the_pair() {
	TestDb::run(|mut conn, _url| async move {
		let (_older, newer) = seed(&mut conn).await;
		declare_builder(&mut conn, true).await;

		assert!(
			!ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"an untried pair is on the worklist"
		);

		record_build(&mut conn, newer, false).await;

		assert!(
			ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"a failure settles it as firmly as a pass"
		);

		let pairs = pairs_for_group(&mut conn, group()).await.expect("pairs");
		let failed = pairs.iter().find(|p| p.version_id == newer).unwrap();
		assert_eq!(failed.state, PairState::Failed);
		assert_eq!(failed.error.as_deref(), Some("views did not compile"));
	})
	.await;
}

/// An operator asking for a build reinstates a settled pair, and the ask is
/// answered once the build lands.
#[tokio::test(flavor = "multi_thread")]
async fn an_operator_ask_reinstates_a_settled_pair() {
	TestDb::run(|mut conn, _url| async move {
		let (_older, newer) = seed(&mut conn).await;
		declare_builder(&mut conn, true).await;

		record_build(&mut conn, newer, true).await;
		assert!(
			ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap()
		);

		ReportingSchemaRequest::enqueue(&mut conn, group(), newer, Some("someone@bes.au"))
			.await
			.expect("enqueue");

		assert!(
			!ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"an ask puts the pair back on the worklist"
		);

		let pairs = pairs_for_group(&mut conn, group()).await.expect("pairs");
		assert!(
			pairs
				.iter()
				.find(|p| p.version_id == newer)
				.unwrap()
				.requested
		);

		// The build that answers the ask clears it.
		record_build(&mut conn, newer, true).await;
		assert!(
			ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"the ask is answered once the build it asked for lands"
		);
	})
	.await;
}

/// A replica that failed to restore says nothing about whether the pair can be
/// built, so it records no build and the pair stays on the worklist.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_restore_leaves_the_pair_unsettled() {
	TestDb::run(|mut conn, _url| async move {
		let (_older, newer) = seed(&mut conn).await;

		let report = NewBackupRestoreCheck {
			replica_id: None,
			replica_name: None,
			consumer_device_id: CONSUMER.parse().unwrap(),
			group_id: group(),
			machine_id: Some(MACHINE.parse().unwrap()),
			r#type: "tamanu-postgres".parse().unwrap(),
			intent: "reporting-schema".parse().unwrap(),
			snapshot_id: Some("snap-1".to_owned()),
			outcome: commons_types::backup::RunOutcome::Failure,
			error: Some("replica never came up".to_owned()),
			replica_healthy: false,
			postgres_version: None,
			observed_at: jiff::Timestamp::now(),
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
		};

		ReportingSchemaBuild::record(
			&mut conn,
			report,
			NewReportingSchemaBuild {
				group_id: group(),
				version_id: newer,
				application_id: Some(CENTRAL.parse().unwrap()),
				built: false,
				error: None,
				artifact_ids: vec![],
			},
		)
		.await
		.expect("record");

		assert!(
			!ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"an unhealthy restore is dispatched again rather than settling the pair"
		);
	})
	.await;
}

/// A schema built from a superseded release of a version is not the schema that
/// version describes, so registering an artifact against the version puts the
/// pair back on the worklist without an operator asking.
#[tokio::test(flavor = "multi_thread")]
async fn a_new_artifact_for_the_version_reinstates_the_pair() {
	TestDb::run(|mut conn, _url| async move {
		let (_older, newer) = seed(&mut conn).await;

		record_build(&mut conn, newer, true).await;
		assert!(
			ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"a built pair settles"
		);

		conn.batch_execute(&format!(
			"INSERT INTO artifacts (version_id, artifact_type, platform, download_url)
			 VALUES ('{}', 'migrations', 'any', 'https://example.com/m.tar')",
			newer
		))
		.await
		.expect("register an artifact for the version");

		assert!(
			!ReportingSchemaBuild::is_settled(&mut conn, group(), newer)
				.await
				.unwrap(),
			"the version's artifacts changed, so the pair is built again"
		);
	})
	.await;
}

/// A build records the artifacts it registered, so an operator can see what came
/// out of it rather than only that something did.
#[tokio::test(flavor = "multi_thread")]
async fn a_build_records_what_it_registered() {
	TestDb::run(|mut conn, _url| async move {
		let (_older, newer) = seed(&mut conn).await;

		let artifact: Uuid = "77777777-7777-7777-7777-777777777777".parse().unwrap();
		conn.batch_execute(&format!(
			"INSERT INTO artifacts (id, version_id, artifact_type, platform, download_url)
			 VALUES ('{artifact}', '{newer}', 'reporting-schema', 'any', 'https://example.com/s.sql')"
		))
		.await
		.expect("seed artifact");

		let report = report_for(commons_types::backup::RunOutcome::Success, true, None);
		ReportingSchemaBuild::record(
			&mut conn,
			report,
			NewReportingSchemaBuild {
				group_id: group(),
				version_id: newer,
				application_id: Some(CENTRAL.parse().unwrap()),
				built: true,
				error: None,
				artifact_ids: vec![artifact],
			},
		)
		.await
		.expect("record");

		let build = ReportingSchemaBuild::latest_for_pair(&mut conn, group(), newer)
			.await
			.unwrap()
			.expect("a build");
		assert_eq!(build.artifact_ids, vec![Some(artifact)]);
	})
	.await;
}

/// A declaration whose consumer advertises a schema-building intent, which is
/// what brings a group into the sweep at all.
async fn declare_builder(conn: &mut AsyncPgConnection, enabled: bool) {
	conn.batch_execute(&format!(
		"INSERT INTO restore_consumer_capabilities
		 (consumer_device_id, intent, description, semantics, params)
		 VALUES ('{CONSUMER}', 'reporting-schema', '',
		         '[\"check\",\"once\",\"migrate\",\"reporting-schema\"]'::jsonb, '[]'::jsonb);

		 INSERT INTO restore_replicas
		 (consumer_device_id, group_id, type, intent, name, enabled, params)
		 VALUES ('{CONSUMER}', '{GROUP}', 'tamanu-postgres', 'reporting-schema',
		         'kamaka-schemas', {enabled}, '{{}}'::jsonb)",
	))
	.await
	.expect("declare builder");
}

/// The reporting-schema issues standing against the group's central.
async fn schema_issues(conn: &mut AsyncPgConnection) -> Vec<database::issues::Issue> {
	database::issues::Issue::list_by_source_ref(
		conn,
		database::statuses::CANOPY_SOURCE,
		database::backup::refs::REPORTING_SCHEMA,
		&[CENTRAL.parse().unwrap()],
	)
	.await
	.expect("list issues")
}

/// A failed build files against the group's central application, carrying the
/// builder's own description, and grades a warning rather than a failure.
#[tokio::test(flavor = "multi_thread")]
async fn a_failed_build_warns_on_the_group_central() {
	TestDb::run(|mut conn, _url| async move {
		let (older, _newer) = seed(&mut conn).await;
		declare_builder(&mut conn, true).await;
		record_build(&mut conn, older, false).await;

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep");

		let issues = schema_issues(&mut conn).await;
		assert_eq!(issues.len(), 1, "one check per group, on its central");
		let issue = &issues[0];
		assert_eq!(
			issue.effective_result,
			Some(commons_types::status::CheckResult::Warning),
			"a failed build is a warning, not a failure"
		);
		assert!(issue.active);
		assert!(
			issue.message.contains("2.59.0") && issue.message.contains("views did not compile"),
			"the builder's own description reaches the operator: {}",
			issue.message
		);
	})
	.await;
}

/// The check does not escalate. A warning ceiling is what holds that: an
/// escalating flag is normalised away for anything below a failure, so pinning
/// the ceiling is what stops a schema nobody can build waking whoever is on
/// call for an application that is up and answering.
#[tokio::test(flavor = "multi_thread")]
async fn the_reporting_schema_check_cannot_escalate() {
	TestDb::run(|mut conn, _url| async move {
		let (older, _newer) = seed(&mut conn).await;
		declare_builder(&mut conn, true).await;
		record_build(&mut conn, older, false).await;

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep");

		let policies = database::check_policies::CheckPolicy::get_across_namespaces(
			&mut conn,
			database::statuses::CANOPY_SOURCE,
			database::backup::refs::REPORTING_SCHEMA,
		)
		.await
		.expect("read the policy");

		assert!(!policies.is_empty(), "the filing seeds a policy");
		for policy in &policies {
			assert_eq!(
				policy.ceiling,
				commons_types::status::CheckResult::Warning,
				"a failed build tops out at a warning"
			);
			assert!(!policy.escalates, "and so cannot escalate");
		}

		assert!(
			!schema_issues(&mut conn).await[0].escalates,
			"which the issue carries through"
		);
	})
	.await;
}

/// The check recovers when the pair is built.
#[tokio::test(flavor = "multi_thread")]
async fn a_built_pair_grades_the_check_passed() {
	TestDb::run(|mut conn, _url| async move {
		let (older, _newer) = seed(&mut conn).await;
		declare_builder(&mut conn, true).await;
		record_build(&mut conn, older, true).await;

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep");

		let issues = schema_issues(&mut conn).await;
		assert_eq!(issues.len(), 1);
		assert_eq!(
			issues[0].effective_result,
			Some(commons_types::status::CheckResult::Passed),
			"a built pair is not a finding"
		);
	})
	.await;
}

/// A pair still awaiting its first build is not a failure: nothing has gone
/// wrong yet, and the worklist is what moves it along.
#[tokio::test(flavor = "multi_thread")]
async fn a_pair_awaiting_its_first_build_files_nothing() {
	TestDb::run(|mut conn, _url| async move {
		seed(&mut conn).await;
		declare_builder(&mut conn, true).await;

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep");

		assert!(
			schema_issues(&mut conn).await.is_empty(),
			"an unbuilt pair is not yet a finding"
		);
	})
	.await;
}

/// A group nothing builds schemas for owes none, so a disabled declaration
/// files nothing even where a build once failed.
#[tokio::test(flavor = "multi_thread")]
async fn a_disabled_declaration_takes_the_group_out_of_the_sweep() {
	TestDb::run(|mut conn, _url| async move {
		let (older, _newer) = seed(&mut conn).await;
		declare_builder(&mut conn, false).await;
		record_build(&mut conn, older, false).await;

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep");

		assert!(
			schema_issues(&mut conn).await.is_empty(),
			"a group with no enabled builder is not owed a schema"
		);
	})
	.await;
}

/// An open check has to be closed when the group's last non-awaiting pair goes
/// away, or it stands forever against a group that owes nothing.
#[tokio::test(flavor = "multi_thread")]
async fn the_check_closes_once_the_group_owes_no_schema() {
	TestDb::run(|mut conn, _url| async move {
		let (older, _newer) = seed(&mut conn).await;
		declare_builder(&mut conn, true).await;
		record_build(&mut conn, older, false).await;

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep");
		assert_eq!(
			schema_issues(&mut conn).await[0].effective_result,
			Some(commons_types::status::CheckResult::Warning),
			"the warning stands while the pair is failed"
		);

		conn.batch_execute("DELETE FROM reporting_schema_builds")
			.await
			.expect("drop the build");

		database::reporting_schemas::sweep(&mut conn)
			.await
			.expect("sweep again");

		let issues = schema_issues(&mut conn).await;
		assert_eq!(issues.len(), 1, "the same check, regraded");
		assert_eq!(
			issues[0].effective_result,
			Some(commons_types::status::CheckResult::Passed),
			"a group owed no schema is not a finding"
		);
		assert!(
			issues[0].message.contains("No reporting schema is owed"),
			"the closing message says why: {}",
			issues[0].message
		);
	})
	.await;
}

/// A group nothing builds schemas for is owed none, so it presents no pairs
/// even where its applications report published versions. Listing them would
/// offer an operator a build nothing will pick up.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_with_no_builder_has_no_pairs() {
	TestDb::run(|mut conn, _url| async move {
		seed(&mut conn).await;

		assert!(
			pairs_for_group(&mut conn, group())
				.await
				.expect("pairs")
				.is_empty(),
			"no declaration covers the group, so it is owed no schema"
		);

		declare_builder(&mut conn, true).await;

		assert_eq!(
			pairs_for_group(&mut conn, group())
				.await
				.expect("pairs")
				.len(),
			2,
			"declaring a builder is what brings the pairs into being"
		);
	})
	.await;
}

/// A pair is unique per group and version. Two applications reporting one
/// version are one pair, so the worklist dispatches one restore rather than
/// one per application.
#[tokio::test(flavor = "multi_thread")]
async fn two_applications_on_one_version_are_one_pair() {
	TestDb::run(|mut conn, _url| async move {
		let (_older, newer) = seed(&mut conn).await;

		conn.batch_execute(&format!(
			"UPDATE application_reported_detail SET version = '2.60.0'
			 WHERE application_id = '{FACILITY}'"
		))
		.await
		.expect("put the facility on the central's version");

		let versions = versions_for_group(&mut conn, group())
			.await
			.expect("derive versions");

		assert_eq!(
			versions.iter().filter(|v| v.id == newer).count(),
			1,
			"one pair, not one per reporting application: {versions:?}"
		);
	})
	.await;
}
