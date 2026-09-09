//! Reporting-schema builds: which pairs of group and Tamanu version have a
//! schema, which have been tried, and which an operator has asked for again.
//!
//! spec: RPT

use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	restore::{BackupRestoreCheck, NewBackupRestoreCheck},
	versions::Version,
};
use commons_types::backup::RunOutcome;

/// A build of one pair, hanging off the restore report that carries the
/// replica's own health.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::reporting_schema_builds)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ReportingSchemaBuild {
	/// The restore report this build was reported with.
	pub check_id: i64,
	/// The group the schema was built for.
	pub group_id: Uuid,
	/// The Tamanu version the schema was built for.
	pub version_id: Uuid,
	/// The central application whose snapshot the replica was restored from.
	pub application_id: Option<Uuid>,
	/// Whether a schema came out of it.
	pub built: bool,
	/// What went wrong, where it did not.
	pub error: Option<String>,
	/// The artifacts this build registered, of which the schema is one.
	pub artifact_ids: Vec<Option<Uuid>>,
	/// When the build was recorded, which is what a later artifact change is
	/// compared against.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub built_at: Timestamp,
}

#[derive(Debug, Clone)]
pub struct NewReportingSchemaBuild {
	pub group_id: Uuid,
	pub version_id: Uuid,
	pub application_id: Option<Uuid>,
	pub built: bool,
	pub error: Option<String>,
	pub artifact_ids: Vec<Uuid>,
}

impl ReportingSchemaBuild {
	/// Record a build: the replica's restore report first, then the build that
	/// rode on it.
	// spec: RPT#what-a-build-reports
	pub async fn record(
		db: &mut AsyncPgConnection,
		report: NewBackupRestoreCheck,
		build: NewReportingSchemaBuild,
	) -> Result<i64> {
		let restore_failed = report.outcome != RunOutcome::Success;

		let check_id = BackupRestoreCheck::record_report(db, report).await?;

		// A replica that failed to restore says nothing about whether the pair
		// can be built: the build never ran. Restore-health already raises on
		// that, and recording no build leaves the pair unsettled so it is
		// dispatched again, which is what an unhealthy restore should do.
		if restore_failed {
			return Ok(check_id);
		}

		diesel::insert_into(crate::schema::reporting_schema_builds::table)
			.values((
				crate::schema::reporting_schema_builds::check_id.eq(check_id),
				crate::schema::reporting_schema_builds::group_id.eq(build.group_id),
				crate::schema::reporting_schema_builds::version_id.eq(build.version_id),
				crate::schema::reporting_schema_builds::application_id.eq(build.application_id),
				crate::schema::reporting_schema_builds::built.eq(build.built),
				crate::schema::reporting_schema_builds::error.eq(build.error),
				crate::schema::reporting_schema_builds::artifact_ids.eq(build
					.artifact_ids
					.into_iter()
					.map(Some)
					.collect::<Vec<_>>()),
			))
			.execute(db)
			.await?;

		// An operator's ask is answered once the build it asked for lands,
		// whichever way it went.
		ReportingSchemaRequest::clear(db, build.group_id, build.version_id).await?;

		Ok(check_id)
	}

	/// The most recent build of a pair, if it has been tried.
	pub async fn latest_for_pair(
		db: &mut AsyncPgConnection,
		group: Uuid,
		version: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::{backup_restore_checks, reporting_schema_builds};

		reporting_schema_builds::table
			.inner_join(
				backup_restore_checks::table
					.on(backup_restore_checks::id.eq(reporting_schema_builds::check_id)),
			)
			.filter(reporting_schema_builds::group_id.eq(group))
			.filter(reporting_schema_builds::version_id.eq(version))
			.order_by(backup_restore_checks::reported_at.desc())
			.select(Self::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The most recent build of each of a group's pairs, by version.
	///
	/// One query rather than one per version: this backs both the operator page
	/// and the sweep, which walk every version a group runs.
	pub async fn latest_by_version_for_group(
		db: &mut AsyncPgConnection,
		group: Uuid,
	) -> Result<std::collections::HashMap<Uuid, Self>> {
		use crate::schema::{backup_restore_checks, reporting_schema_builds};

		let builds: Vec<Self> = reporting_schema_builds::table
			.inner_join(
				backup_restore_checks::table
					.on(backup_restore_checks::id.eq(reporting_schema_builds::check_id)),
			)
			.filter(reporting_schema_builds::group_id.eq(group))
			.distinct_on(reporting_schema_builds::version_id)
			.order_by((
				reporting_schema_builds::version_id,
				backup_restore_checks::reported_at.desc(),
			))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(builds.into_iter().map(|b| (b.version_id, b)).collect())
	}

	/// Whether a pair is settled: it has been built or has failed, and either
	/// way is not dispatched again until the version's artifacts change or an
	/// operator asks.
	// spec: RPT#pairs
	pub async fn is_settled(
		db: &mut AsyncPgConnection,
		group: Uuid,
		version: Uuid,
	) -> Result<bool> {
		let row = Version::get_by_id(db, version).await?;
		let settlement = Settlement::for_group(db, group, std::slice::from_ref(&row)).await?;
		Ok(settlement.settled(version))
	}
}

/// Where every pair of a group stands, answered from memory.
///
/// The worklist asks this of each of a group's versions on every poll, and
/// every restore consumer polls on a schedule, so the three lookups it takes
/// are made once for the group rather than once per pair.
// spec: RPT#pairs
pub struct Settlement {
	requested: std::collections::HashSet<Uuid>,
	builds: std::collections::HashMap<Uuid, ReportingSchemaBuild>,
	changed: std::collections::HashMap<Uuid, Timestamp>,
}

impl Settlement {
	pub async fn for_group(
		db: &mut AsyncPgConnection,
		group: Uuid,
		versions: &[Version],
	) -> Result<Self> {
		Ok(Self {
			requested: ReportingSchemaRequest::pending_for_group(db, group).await?,
			builds: ReportingSchemaBuild::latest_by_version_for_group(db, group).await?,
			changed: crate::artifacts::Artifact::newest_change_for_versions(db, versions).await?,
		})
	}

	pub fn settled(&self, version: Uuid) -> bool {
		if self.requested.contains(&version) {
			return false;
		}

		let Some(build) = self.builds.get(&version) else {
			return false;
		};

		// A schema built from a superseded release of the version is not the
		// schema that version describes, so an artifact registered since the
		// build puts the pair back on the worklist.
		match self.changed.get(&version) {
			Some(at) => *at <= build.built_at,
			None => true,
		}
	}
}

/// An operator asking for a pair's build.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::reporting_schema_requests)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ReportingSchemaRequest {
	pub group_id: Uuid,
	pub version_id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub requested_at: Timestamp,
	pub requested_by: Option<String>,
}

impl ReportingSchemaRequest {
	/// Enqueue, or refresh, an ask for a pair.
	// spec: RPT#pairs
	pub async fn enqueue(
		db: &mut AsyncPgConnection,
		group: Uuid,
		version: Uuid,
		requested_by: Option<&str>,
	) -> Result<()> {
		use crate::schema::reporting_schema_requests::dsl;

		diesel::insert_into(dsl::reporting_schema_requests)
			.values((
				dsl::group_id.eq(group),
				dsl::version_id.eq(version),
				dsl::requested_by.eq(requested_by),
			))
			.on_conflict((dsl::group_id, dsl::version_id))
			.do_update()
			.set((
				dsl::requested_at.eq(diesel::dsl::now),
				dsl::requested_by.eq(requested_by),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;

		Ok(())
	}

	/// Which of a group's versions have an ask pending, in one query.
	pub async fn pending_for_group(
		db: &mut AsyncPgConnection,
		group: Uuid,
	) -> Result<std::collections::HashSet<Uuid>> {
		use crate::schema::reporting_schema_requests::dsl;

		let versions: Vec<Uuid> = dsl::reporting_schema_requests
			.filter(dsl::group_id.eq(group))
			.select(dsl::version_id)
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(versions.into_iter().collect())
	}

	async fn clear(db: &mut AsyncPgConnection, group: Uuid, version: Uuid) -> Result<()> {
		use crate::schema::reporting_schema_requests::dsl;

		diesel::delete(
			dsl::reporting_schema_requests
				.filter(dsl::group_id.eq(group))
				.filter(dsl::version_id.eq(version)),
		)
		.execute(db)
		.await
		.map_err(AppError::from)?;

		Ok(())
	}
}

/// Where a pair stands, for the operator view.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum PairState {
	/// No build has been recorded, so the pair is on the worklist.
	Awaiting,
	/// A build produced a schema.
	Built,
	/// A build ran and produced none.
	Failed,
}

/// One pair of group and Tamanu version, and where it stands.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Pair {
	/// The group this pair is for.
	pub group_id: Uuid,
	/// The Tamanu version this pair is for.
	pub version_id: Uuid,
	/// That version as semver, for display.
	pub version: String,
	/// Whether the pair has a schema, failed to build one, or is awaiting one.
	pub state: PairState,
	/// What went wrong, where a build failed.
	pub error: Option<String>,
	/// Whether an operator has asked for this pair to be built again.
	pub requested: bool,
	/// The group's Tamanu applications reporting this version, by name. Empty
	/// where the pair comes from the open plan rather than from something
	/// running it.
	pub applications: Vec<String>,
}

/// The pairs of a group: every published version its Tamanu applications report
/// running, plus the version its open plan moves it to.
///
/// A group no enabled declaration covers has no pairs. Canopy owes it no
/// schema, so listing versions against it would offer an operator a build
/// nothing will pick up.
// spec: RPT#pairs
pub async fn pairs_for_group(db: &mut AsyncPgConnection, group: Uuid) -> Result<Vec<Pair>> {
	if !groups_building_schemas(db).await?.contains(&group) {
		return Ok(Vec::new());
	}

	let members = crate::applications::Application::list_live_in_group(db, group).await?;
	pairs_of_members(db, group, &members).await
}

/// The pairs of a group already known to have a builder, from members already
/// in hand.
// spec: RPT#pairs
async fn pairs_of_members(
	db: &mut AsyncPgConnection,
	group: Uuid,
	members: &[crate::applications::Application],
) -> Result<Vec<Pair>> {
	let versions = versions_and_applications(db, group, members).await?;
	let builds = ReportingSchemaBuild::latest_by_version_for_group(db, group).await?;
	let requests = ReportingSchemaRequest::pending_for_group(db, group).await?;

	let mut pairs = Vec::with_capacity(versions.len());
	for (version, applications) in versions {
		let (state, error) = match builds.get(&version.id) {
			None => (PairState::Awaiting, None),
			Some(build) if build.built => (PairState::Built, None),
			Some(build) => (PairState::Failed, build.error.clone()),
		};

		pairs.push(Pair {
			group_id: group,
			version_id: version.id,
			applications,
			version: version.as_semver().to_string(),
			state,
			error,
			requested: requests.contains(&version.id),
		});
	}

	Ok(pairs)
}

/// Every published version a group's Tamanu applications report running, plus
/// the version its open plan moves it to.
///
/// A reported version Canopy holds no release row for is not a pair: a build
/// needs that version's migrations, which reach a builder as its published
/// artifacts.
// spec: RPT#pairs
pub async fn versions_for_group(db: &mut AsyncPgConnection, group: Uuid) -> Result<Vec<Version>> {
	let members = crate::applications::Application::list_live_in_group(db, group).await?;

	Ok(versions_and_applications(db, group, &members)
		.await?
		.into_iter()
		.map(|(version, _)| version)
		.collect())
}

/// A group's pairs, and which of its Tamanu applications report each one.
///
/// The applications are carried alongside the version rather than joined back
/// on a stringified semver: a `Version` row holds major, minor and patch alone,
/// so a reported `2.60.0-rc1` resolves to the 2.60.0 row and would never match
/// its own key, and the pair would read as an upgrade plan while a server runs
/// it.
// spec: RPT#pairs
async fn versions_and_applications(
	db: &mut AsyncPgConnection,
	group: Uuid,
	applications: &[crate::applications::Application],
) -> Result<Vec<(Version, Vec<String>)>> {
	use commons_types::version::VersionStatus;

	let tamanu: Vec<&crate::applications::Application> = applications
		.iter()
		.filter(|a| a.r#type.software() == "tamanu")
		.collect();

	let ids: Vec<Uuid> = tamanu.iter().map(|a| a.id).collect();
	let reported = crate::reported_detail::ReportedDetail::last_versions(db, &ids).await?;

	let mut wanted: Vec<commons_types::version::VersionStr> = reported.values().cloned().collect();
	wanted.sort_by(|a, b| a.0.cmp(&b.0));
	wanted.dedup_by(|a, b| a.0 == b.0);
	let released = Version::get_by_versions(db, &wanted).await?;

	let mut pairs: Vec<(Version, Vec<String>)> = Vec::new();
	for application in tamanu {
		let Some(shown) = reported.get(&application.id) else {
			continue;
		};
		// A version Canopy holds no release row for is not a pair: a build needs
		// that version's migrations, which reach a builder as published artifacts.
		let Some(version) = released.iter().find(|v| {
			(v.major, v.minor, v.patch)
				== (
					shown.0.major as i32,
					shown.0.minor as i32,
					shown.0.patch as i32,
				)
		}) else {
			continue;
		};
		if version.status != VersionStatus::Published {
			continue;
		}

		// A pair is unique per group and version, so two applications on one
		// version are one pair carrying both names.
		match pairs.iter_mut().find(|(v, _)| v.id == version.id) {
			Some((_, names)) => names.push(application.label()),
			None => pairs.push((version.clone(), vec![application.label()])),
		}
	}

	// A plan moving a group to a version something already runs adds no pair.
	// Dispatch counts a restore and a migrate per entry, so a duplicate here is
	// paid for rather than merely untidy.
	if let Some(target) = crate::upgrade_plans::planned_target(db, group).await?
		&& !pairs.iter().any(|(v, _)| v.id == target.id)
	{
		pairs.push((target, Vec::new()));
	}

	for (_, names) in &mut pairs {
		names.sort();
	}
	pairs.sort_by_key(|(v, _)| (v.major, v.minor, v.patch));

	Ok(pairs)
}

/// File the reporting-schema check for every group that has a builder.
///
/// One check per group, on its central application, with each of the group's
/// pairs as an instance. The version is in the instance detail rather than the
/// check name, so a release does not spawn a catalog entry of its own.
// spec: RPT#alerting
pub async fn sweep(db: &mut AsyncPgConnection) -> Result<()> {
	use crate::{
		applications::Application,
		backup::refs,
		issues::{CheckInstance, GradedInstance, Scope},
		restore::{RestoreCheck, file_restore_check},
		server_groups::ServerGroup,
	};
	use commons_types::status::CheckResult;

	// Which groups have a builder is one question of the whole fleet rather than
	// one per group: asking per group walked every group's declarations and
	// every declaration's consumer, once a minute, for groups that have none.
	let builders = groups_building_schemas(db).await?;

	for group in ServerGroup::list_all(db).await? {
		let members = Application::list_live_in_group(db, group.id).await?;
		let Some(central) = ServerGroup::canonical_central(&members).map(|a| a.id) else {
			continue;
		};

		// A group that has stopped building still has whatever this check filed
		// while it did, and nothing else recovers it. Filing no instances is
		// what says the finding is gone; where none was open this costs one
		// query and writes nothing.
		// spec: RPT#alerting
		let pairs = if builders.contains(&group.id) {
			pairs_of_members(db, group.id, &members).await?
		} else {
			Vec::new()
		};

		let instances: Vec<CheckInstance> = pairs
			.iter()
			.filter(|p| p.state != PairState::Awaiting)
			.map(|pair| {
				let mut detail = serde_json::json!({ "version": pair.version });
				if pair.state != PairState::Built {
					detail["why"] = pair
						.error
						.clone()
						.unwrap_or_else(|| format!("no schema could be built for {}", pair.version))
						.into();
				}

				CheckInstance {
					label: pair.version.clone(),
					observed: match pair.state {
						PairState::Built => CheckResult::Passed,
						_ => CheckResult::Warning,
					},
					detail: Some(detail),
				}
			})
			.collect();

		let name = group.name.clone();
		let total = pairs.len();
		file_restore_check(
			db,
			Scope::Application(central),
			RestoreCheck {
				r#ref: refs::REPORTING_SCHEMA,
				documentation: refs::REPORTING_SCHEMA_DOC,
				title: "reporting schema not built",
				gone: &format!("No reporting schema is owed for {}", group.name),
			},
			instances,
			&move |degraded: &[GradedInstance]| match degraded {
				[] => format!("Reporting schemas are built for every version {name} runs"),
				[one] => format!(
					"No reporting schema for {name} on {}: {}",
					one.label,
					one.detail
						.as_ref()
						.and_then(|d| d.get("why"))
						.and_then(|v| v.as_str())
						.unwrap_or("the build failed")
				),
				many => format!(
					"No reporting schema for {} of {total} versions {name} runs: {}",
					many.len(),
					many.iter()
						.map(|i| i.label.as_str())
						.collect::<Vec<_>>()
						.join(", ")
				),
			},
		)
		.await?;
	}

	Ok(())
}

/// The groups an enabled declaration builds schemas for.
///
/// The same conditions `RestoreReplica::authorizes_schema_artifacts` asks of one
/// consumer and one group, asked of the fleet at once: dispatching builds a
/// group would then refuse to accept is the divergence worth not having.
async fn groups_building_schemas(
	db: &mut AsyncPgConnection,
) -> Result<std::collections::HashSet<Uuid>> {
	use crate::schema::{restore_consumer_capabilities, restore_replicas};
	use diesel::dsl::sql;
	use diesel::sql_types::Bool;

	let groups: Vec<Uuid> = restore_replicas::table
		.inner_join(
			restore_consumer_capabilities::table.on(
				restore_consumer_capabilities::consumer_device_id
					.eq(restore_replicas::consumer_device_id)
					.and(restore_consumer_capabilities::intent.eq(restore_replicas::intent)),
			),
		)
		.filter(restore_replicas::enabled.eq(true))
		.filter(restore_replicas::publishes_schemas.eq(true))
		// Dispatch builds no schema from a redacting or machine-scoped
		// declaration, and one nothing is dispatched for publishes nothing.
		.filter(restore_replicas::redacts.eq(false))
		.filter(restore_replicas::machine_id.is_null())
		.filter(sql::<Bool>(
			"restore_consumer_capabilities.semantics @> '[\"reporting-schema\"]'::jsonb",
		))
		.select(restore_replicas::group_id)
		.distinct()
		.load(db)
		.await
		.map_err(AppError::from)?;

	Ok(groups.into_iter().collect())
}
