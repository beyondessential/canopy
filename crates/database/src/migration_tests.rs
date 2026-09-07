//! Which versions Canopy asks to be migration-tested against which applications,
//! and what came back.
//!
//! Candidacy here is the version axis only. Whether a server has a snapshot to
//! restore and migrate is settled when a consumer's worklist is built.

use std::collections::HashMap;

use commons_errors::Result;
use commons_types::backup::{BackupType, RestoreIntent, RunOutcome};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	applications::Application, backup::refs, pg_duration::PgDuration, restore::BackupRestoreCheck,
	restore::NewBackupRestoreCheck, version_known_issues::VersionKnownIssue, versions::Version,
};

/// A version a server could upgrade to, so one to test against that server's
/// data before it gets there.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
	pub server_id: Uuid,
	pub version_id: Uuid,
}

/// The version `server` should be tested against, if any.
///
/// Its own environment's open plan names it (see [`crate::upgrade_plans`]), and
/// an environment with no plan has no candidate: a restore costs hours, and it
/// is only worth spending on a version an environment has said it intends to
/// apply. An application with no rank follows its group's headline environment.
///
/// Tamanu applications only: the migrations under test are Tamanu's, so no other
/// product's server has an upgrade path through them.
// spec: RST#candidate-versions
pub async fn candidate_for(
	db: &mut AsyncPgConnection,
	server: &Application,
) -> Result<Option<Version>> {
	// The migrations under test are Tamanu's, so only Tamanu has candidates —
	// a central and a facility alike, both being upgraded along the same train.
	// spec: RST#candidate-versions
	if server.r#type.software() != "tamanu" {
		return Ok(None);
	}

	let Some(group_id) = server.group_id else {
		return Ok(None);
	};
	let Some(rank) = crate::server_groups::ServerGroup::environment_of(db, server).await? else {
		return Ok(None);
	};

	crate::upgrade_plans::planned_target(db, group_id, rank).await
}

/// Every candidate across the fleet, at most one per server.
// spec: RST#candidate-versions
pub async fn candidates(db: &mut AsyncPgConnection) -> Result<Vec<Candidate>> {
	let mut candidates = Vec::new();

	for server in Application::get_all(db, 0, None).await? {
		if let Some(version) = candidate_for(db, &server).await? {
			candidates.push(Candidate {
				server_id: server.id,
				version_id: version.id,
			});
		}
	}

	Ok(candidates)
}

/// How long one migration took, in the order it ran.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::migration_timings)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MigrationTiming {
	pub ordinal: i32,
	pub name: String,
	pub elapsed: PgDuration,
}

/// A recorded migration test, alongside the report carrying its common fields.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::migration_tests)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MigrationTest {
	pub check_id: i64,
	pub target_version_id: Uuid,
	pub total_elapsed: PgDuration,
	pub failed_migration: Option<String>,
	pub data_bytes_before: i64,
	pub data_bytes_after: i64,
}

/// What a consumer reports for one migration test, beyond the report's own
/// fields.
#[derive(Debug, Clone)]
pub struct NewMigrationTest {
	/// The application whose candidate version was tried. The report names the
	/// machine whose snapshot was restored; this names the workload the version
	/// belongs to, which a box running two of them could not otherwise recover.
	// spec: RST#candidate-versions
	pub application_id: Uuid,
	pub target_version_id: Uuid,
	pub total_elapsed: PgDuration,
	pub failed_migration: Option<String>,
	pub data_bytes_before: i64,
	pub data_bytes_after: i64,
	/// One entry per migration that ran, in the order they ran.
	pub timings: Vec<(String, PgDuration)>,
}

/// One joined row behind [`latest_test`].
#[derive(Queryable)]
struct LatestRow {
	outcome: RunOutcome,
	failed_migration: Option<String>,
	snapshot_id: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	reported_at: Timestamp,
	total_elapsed: PgDuration,
	data_bytes_before: i64,
	data_bytes_after: i64,
}

/// The most recent test of one (server, version) pair.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct LatestTest {
	/// Whether the migrations applied.
	pub verdict: Verdict,
	/// The migration that failed, when one did.
	pub failed_migration: Option<String>,
	/// The snapshot the verdict was reached against.
	pub snapshot_id: Option<String>,
	/// When the consumer reported it.
	#[schema(value_type = String)]
	pub reported_at: Timestamp,
	/// Whole seconds the migration run took.
	#[schema(value_type = i64)]
	pub total_elapsed: PgDuration,
	/// Size of the data the migrations ran against.
	pub data_bytes_before: i64,
	/// Size of it afterwards; the growth is what a heavy backfill shows up as.
	pub data_bytes_after: i64,
}

/// Where one of a group's applications stands against the version it would take
/// next.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct GroupVerdict {
	/// The server the verdict is about.
	pub server_id: Uuid,
	/// The version it would take next.
	pub target_version_id: Uuid,
	/// That version, as semver.
	pub target_version: String,
	/// Where it stands against that version.
	pub verdict: Verdict,
	/// The test the verdict came from, absent when there has not been one.
	pub latest: Option<LatestTest>,
}

/// Where a (server, version) pair stands.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum Verdict {
	NotTested,
	Passed,
	Failed,
}

impl MigrationTest {
	/// Record a migration test: the report that carries the common fields,
	/// then the result and its per-migration timings.
	// spec: RST#what-a-migration-test-reports
	pub async fn record(
		db: &mut AsyncPgConnection,
		report: NewBackupRestoreCheck,
		test: NewMigrationTest,
	) -> Result<i64> {
		let application_id = test.application_id;
		let target_version_id = test.target_version_id;
		let failed_migration = test.failed_migration.clone();

		let restore_failed = report.outcome != RunOutcome::Success;

		let check_id = BackupRestoreCheck::record_report(db, report).await?;

		// A failed restore with no named migration says nothing about the
		// version: the migrations never ran. Restore-health already raises on
		// the failure, and leaving no result keeps the pair retryable.
		if restore_failed && failed_migration.is_none() {
			return Ok(check_id);
		}

		diesel::insert_into(crate::schema::migration_tests::table)
			.values((
				crate::schema::migration_tests::check_id.eq(check_id),
				crate::schema::migration_tests::application_id.eq(Some(test.application_id)),
				crate::schema::migration_tests::target_version_id.eq(test.target_version_id),
				crate::schema::migration_tests::total_elapsed.eq(test.total_elapsed),
				crate::schema::migration_tests::failed_migration.eq(test.failed_migration),
				crate::schema::migration_tests::data_bytes_before.eq(test.data_bytes_before),
				crate::schema::migration_tests::data_bytes_after.eq(test.data_bytes_after),
			))
			.execute(db)
			.await?;

		let timings: Vec<_> = test
			.timings
			.into_iter()
			.enumerate()
			.map(|(index, (name, elapsed))| {
				(
					crate::schema::migration_timings::check_id.eq(check_id),
					crate::schema::migration_timings::ordinal.eq(index as i32),
					crate::schema::migration_timings::name.eq(name),
					crate::schema::migration_timings::elapsed.eq(elapsed),
				)
			})
			.collect();
		if !timings.is_empty() {
			diesel::insert_into(crate::schema::migration_timings::table)
				.values(timings)
				.execute(db)
				.await?;
		}

		// The finding is held against the application whose candidate this was:
		// a version is an application's, so a box hosting two workloads carries
		// it against whichever of them the version was tested for.
		file_outcome(
			db,
			application_id,
			target_version_id,
			failed_migration.as_deref(),
		)
		.await?;

		Ok(check_id)
	}

	/// The per-migration timings of one test, in the order they ran.
	pub async fn timings(
		db: &mut AsyncPgConnection,
		check_id: i64,
	) -> Result<Vec<MigrationTiming>> {
		use crate::schema::migration_timings::dsl;

		dsl::migration_timings
			.select(MigrationTiming::as_select())
			.filter(dsl::check_id.eq(check_id))
			.order(dsl::ordinal.asc())
			.load(db)
			.await
			.map_err(Into::into)
	}
}

/// The most recent test of `machine`'s data against `version`, with the report
/// context that says when it was and what it ran against.
///
/// The machine, because the data under test is the snapshot a machine backed
/// up; the version carries which application's candidate it was.
// spec: RST#verdicts
pub async fn latest_test(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	target_version_id: Uuid,
) -> Result<Option<LatestTest>> {
	use crate::schema::{backup_restore_checks, migration_tests};

	let row: Option<LatestRow> = migration_tests::table
		.inner_join(backup_restore_checks::table)
		.select((
			backup_restore_checks::outcome,
			migration_tests::failed_migration,
			backup_restore_checks::snapshot_id,
			backup_restore_checks::reported_at,
			migration_tests::total_elapsed,
			migration_tests::data_bytes_before,
			migration_tests::data_bytes_after,
		))
		.filter(migration_tests::target_version_id.eq(target_version_id))
		.filter(backup_restore_checks::machine_id.eq(machine_id))
		.order(backup_restore_checks::reported_at.desc())
		.first(db)
		.await
		.optional()?;

	Ok(row.map(|row| LatestTest {
		verdict: match (row.outcome, &row.failed_migration) {
			(RunOutcome::Success, None) => Verdict::Passed,
			_ => Verdict::Failed,
		},
		failed_migration: row.failed_migration,
		snapshot_id: row.snapshot_id,
		reported_at: row.reported_at,
		total_elapsed: row.total_elapsed,
		data_bytes_before: row.data_bytes_before,
		data_bytes_after: row.data_bytes_after,
	}))
}

/// Where `machine`'s data stands against `version`, from its most recent test.
///
/// A pass means every migration applied. Anything else is a failure, including
/// a report whose restore never got as far as migrating.
// spec: RST#verdicts
pub async fn verdict(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	target_version_id: Uuid,
) -> Result<Verdict> {
	Ok(latest_test(db, machine_id, target_version_id)
		.await?
		.map_or(Verdict::NotTested, |test| test.verdict))
}

/// Whether `server` already has a verdict for this snapshot and target version.
///
/// A failure counts. A migration failing against a fixed snapshot fails the
/// same way every time, so re-dispatching it would spend a full restore on an
/// answer already held.
// spec: RST#dispatching-a-migration-test
pub async fn has_verdict(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	snapshot_id: &str,
	target_version_id: Uuid,
) -> Result<bool> {
	use crate::schema::{backup_restore_checks, migration_tests};

	let existing: Option<i64> = migration_tests::table
		.inner_join(backup_restore_checks::table)
		.select(migration_tests::check_id)
		.filter(migration_tests::target_version_id.eq(target_version_id))
		.filter(backup_restore_checks::machine_id.eq(machine_id))
		.filter(backup_restore_checks::snapshot_id.eq(snapshot_id))
		.first(db)
		.await
		.optional()?;

	Ok(existing.is_some())
}

/// The latest recorded verdict for one replica key.
#[derive(Debug, Clone)]
pub struct KeyVerdict {
	/// The application whose candidate was tried. The `migration-test` check is
	/// filed against it, because the version under test is that application's.
	/// `None` for a test recorded before tests carried the application.
	// spec: RST#alerting
	pub application_id: Option<Uuid>,
	/// The version whose migrations were tried, as semver.
	pub target_version: String,
	/// The migration that failed, when one did.
	pub failed_migration: Option<String>,
	/// The snapshot they were tried against.
	pub snapshot_id: Option<String>,
	/// Whether they all applied. Read the same way [`latest_test`] reads it: a
	/// report whose restore never got as far as migrating is a failure too.
	pub verdict: Verdict,
}

/// The latest verdict per `(machine, type, intent, name)` across the fleet.
///
/// The `migration-test` check is derived from these as well as from
/// declarations: a failed migration is a fact about a candidate version
/// measured against a group's data, so it stands whether or not a
/// declaration still asks for the test, and what supersedes it is a later
/// verdict (see [`crate::restore::sweep_restore_checks`]).
pub async fn latest_verdict_by_key(
	db: &mut AsyncPgConnection,
) -> Result<HashMap<crate::restore::ReplicaKey, KeyVerdict>> {
	use crate::schema::{backup_restore_checks as checks, migration_tests as tests, versions};

	type Row = (
		Option<Uuid>,
		BackupType,
		RestoreIntent,
		Option<String>,
		(i32, i32, i32),
		Option<String>,
		Option<String>,
		RunOutcome,
		Option<Uuid>,
	);

	let rows: Vec<Row> = tests::table
		.inner_join(checks::table)
		.inner_join(versions::table)
		.select((
			checks::machine_id,
			checks::type_,
			checks::intent,
			checks::replica_name,
			(versions::major, versions::minor, versions::patch),
			tests::failed_migration,
			checks::snapshot_id,
			checks::outcome,
			tests::application_id,
		))
		.filter(checks::machine_id.is_not_null())
		.distinct_on((
			checks::machine_id,
			checks::type_,
			checks::intent,
			checks::replica_name,
		))
		// The recency tiebreak is one raw fragment because diesel only proves
		// `DISTINCT ON`/`ORDER BY` agreement for tuples up to five elements, and
		// the four key columns plus these two would be six.
		.order_by((
			checks::machine_id,
			checks::type_,
			checks::intent,
			checks::replica_name,
			diesel::dsl::sql::<diesel::sql_types::Untyped>(
				"backup_restore_checks.observed_at DESC, backup_restore_checks.id DESC",
			),
		))
		.load(db)
		.await?;

	Ok(rows
		.into_iter()
		.filter_map(
			|(
				machine_id,
				r#type,
				intent,
				replica_name,
				version,
				failed_migration,
				snapshot_id,
				outcome,
				application_id,
			)| {
				let verdict = match (outcome, &failed_migration) {
					(RunOutcome::Success, None) => Verdict::Passed,
					_ => Verdict::Failed,
				};
				let (major, minor, patch) = version;
				machine_id.map(|mid| {
					(
						(mid, r#type, intent, replica_name),
						KeyVerdict {
							application_id,
							target_version: format!("{major}.{minor}.{patch}"),
							failed_migration,
							snapshot_id,
							verdict,
						},
					)
				})
			},
		)
		.collect())
}

/// Record the consequence of a migration test: a failed one raises a known
/// issue against the candidate version, which is what holds it back from
/// rollout.
///
/// The `migration-test` check itself is filed by
/// [`crate::restore::sweep_restore_checks`], the sole filer of the restore checks: the
/// verdict recorded here and the overdue bound are two ways the same check can
/// be degraded, and only the sweep sees all of a server's replicas at once.
/// This path reads the verdict back from storage on the next pass (see
/// [`latest_verdict_by_key`]).
async fn file_outcome(
	db: &mut AsyncPgConnection,
	application_id: Uuid,
	target_version_id: Uuid,
	failed_migration: Option<&str>,
) -> Result<()> {
	let Some(migration) = failed_migration else {
		return Ok(());
	};
	let version = Version::get_by_id(db, target_version_id).await?;
	let affected = (version.major, version.minor, version.patch);
	if !VersionKnownIssue::unresolved_for_server(db, affected, application_id).await? {
		VersionKnownIssue::add(
			db,
			affected,
			refs::MIGRATION_TEST,
			&format!("Migration {migration} failed against {application_id}'s data."),
			Some(application_id),
		)
		.await?;
	}

	Ok(())
}

/// Where every server in `group` stands against the version it would take next.
///
/// One row per server that has a candidate at all: with no open plan, or
/// running another product, there is nothing to be tested against and so
/// nothing to show.
// spec: RST#verdicts
pub async fn verdicts_for_group(
	db: &mut AsyncPgConnection,
	group_id: Uuid,
) -> Result<Vec<GroupVerdict>> {
	let applications = Application::list_live_in_group(db, group_id).await?;
	verdicts(db, applications).await
}

/// Where each of `applications` stands against the version it would take next,
/// for a caller that has already picked out an environment's applications.
// spec: RST#verdicts
pub async fn verdicts(
	db: &mut AsyncPgConnection,
	applications: Vec<Application>,
) -> Result<Vec<GroupVerdict>> {
	let mut out = Vec::new();

	for server in applications {
		let Some(version) = candidate_for(db, &server).await? else {
			continue;
		};
		let latest = latest_test(db, server.machine_id, version.id).await?;

		out.push(GroupVerdict {
			server_id: server.id,
			target_version_id: version.id,
			target_version: version.as_semver().to_string(),
			verdict: latest
				.as_ref()
				.map_or(Verdict::NotTested, |test| test.verdict),
			latest,
		});
	}

	Ok(out)
}
