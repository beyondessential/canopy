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

	/// Whether a pair is settled: it has been built or has failed, and either
	/// way is not dispatched again until the version's artifacts change or an
	/// operator asks.
	// spec: RPT#pairs
	pub async fn is_settled(
		db: &mut AsyncPgConnection,
		group: Uuid,
		version: Uuid,
	) -> Result<bool> {
		if ReportingSchemaRequest::pending(db, group, version).await? {
			return Ok(false);
		}

		let Some(build) = Self::latest_for_pair(db, group, version).await? else {
			return Ok(false);
		};

		// A schema built from a superseded release of the version is not the
		// schema that version describes, so an artifact registered since the
		// build puts the pair back on the worklist.
		let changed = crate::artifacts::Artifact::newest_change_for_version(db, version).await?;
		Ok(match changed {
			Some(at) => at <= build.built_at,
			None => true,
		})
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

	pub async fn pending(db: &mut AsyncPgConnection, group: Uuid, version: Uuid) -> Result<bool> {
		use crate::schema::reporting_schema_requests::dsl;

		Ok(dsl::reporting_schema_requests
			.filter(dsl::group_id.eq(group))
			.filter(dsl::version_id.eq(version))
			.select(dsl::group_id)
			.first::<Uuid>(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.is_some())
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
}

/// The pairs of a group: every published version its Tamanu applications report
/// running, plus the version its open plan moves it to.
///
/// A group no enabled declaration covers has no pairs. Canopy owes it no
/// schema, so listing versions against it would offer an operator a build
/// nothing will pick up.
// spec: RPT#pairs
pub async fn pairs_for_group(db: &mut AsyncPgConnection, group: Uuid) -> Result<Vec<Pair>> {
	if !group_builds_schemas(db, group).await? {
		return Ok(Vec::new());
	}

	let versions = versions_for_group(db, group).await?;

	let mut pairs = Vec::with_capacity(versions.len());
	for version in versions {
		let latest = ReportingSchemaBuild::latest_for_pair(db, group, version.id).await?;
		let requested = ReportingSchemaRequest::pending(db, group, version.id).await?;

		let (state, error) = match &latest {
			None => (PairState::Awaiting, None),
			Some(build) if build.built => (PairState::Built, None),
			Some(build) => (PairState::Failed, build.error.clone()),
		};

		pairs.push(Pair {
			group_id: group,
			version_id: version.id,
			version: version.as_semver().to_string(),
			state,
			error,
			requested,
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
	use commons_types::version::VersionStatus;

	let applications = crate::applications::Application::list_live_in_group(db, group).await?;
	let tamanu: Vec<Uuid> = applications
		.iter()
		.filter(|a| a.r#type.software() == "tamanu")
		.map(|a| a.id)
		.collect();

	let mut versions = Vec::new();

	let reported = crate::reported_detail::ReportedDetail::last_versions(db, &tamanu).await?;
	for shown in reported.into_values() {
		// A version Canopy holds no release row for is not a pair: a build needs
		// that version's migrations, which reach a builder as published artifacts.
		if let Ok(version) = Version::get_by_version(db, shown).await
			&& version.status == VersionStatus::Published
		{
			versions.push(version);
		}
	}

	if let Some(target) = crate::upgrade_plans::planned_target(db, group).await? {
		versions.push(target);
	}

	// A pair is unique per group and version, so two applications on one
	// version are one pair, and a plan moving a group to a version something
	// already runs adds none. Dispatch counts a restore and a migrate per
	// entry, so a duplicate here is paid for rather than merely untidy.
	versions.sort_by_key(|v| (v.major, v.minor, v.patch));
	versions.dedup_by_key(|v| v.id);

	Ok(versions)
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
		issues::{
			CheckInstance, GradedInstance, InstancedCheckFiling, Scope, file_check_instances,
		},
		server_groups::ServerGroup,
	};
	use commons_types::status::CheckResult;

	for group in ServerGroup::list_all(db).await? {
		if !group_builds_schemas(db, group.id).await? {
			continue;
		}

		let members = Application::list_live_in_group(db, group.id).await?;
		let Some(central) = ServerGroup::canonical_central(&members).map(|a| a.id) else {
			continue;
		};

		let pairs = pairs_for_group(db, group.id).await?;
		let instances: Vec<CheckInstance> = pairs
			.iter()
			.filter(|p| p.state != PairState::Awaiting)
			.map(|pair| CheckInstance {
				label: pair.version.clone(),
				observed: match pair.state {
					PairState::Built => CheckResult::Passed,
					_ => CheckResult::Warning,
				},
				detail: Some(serde_json::json!({
					"version": pair.version,
					"why": pair.error.clone().unwrap_or_else(|| {
						format!("no schema could be built for {}", pair.version)
					}),
				})),
			})
			.collect();

		// An empty set is not nothing to do: a check already open has to be
		// closed, or it stays open forever once its last pair goes away.
		if instances.is_empty() {
			let open = crate::backup::staleness::open_server_issue_active(
				db,
				central,
				refs::REPORTING_SCHEMA,
			)
			.await?;
			if open {
				crate::issues::file_check(
					db,
					crate::issues::CheckFiling {
						source: crate::statuses::CANOPY_SOURCE,
						scope: Scope::Application(central),
						device_id: None,
						check: refs::REPORTING_SCHEMA,
						observed: CheckResult::Passed,
						detail: None,
						message: &format!("No reporting schema is owed for {}", group.name),
						title: Some("reporting schema not built"),
						default_ceiling: CheckResult::Warning,
						default_escalates: false,
						documentation: Some(refs::REPORTING_SCHEMA_DOC),
					},
				)
				.await?;
			}
			continue;
		}

		let name = group.name.clone();
		let total = instances.len();
		file_check_instances(
			db,
			InstancedCheckFiling {
				source: crate::statuses::CANOPY_SOURCE,
				scope: Scope::Application(central),
				device_id: None,
				check: refs::REPORTING_SCHEMA,
				title: Some("reporting schema not built"),
				instances,
				default_ceiling: CheckResult::Warning,
				default_escalates: false,
				documentation: Some(refs::REPORTING_SCHEMA_DOC),
			},
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

/// Whether a group has an enabled declaration whose intent builds schemas.
async fn group_builds_schemas(db: &mut AsyncPgConnection, group: Uuid) -> Result<bool> {
	use crate::restore::{RestoreConsumerCapability, RestoreReplica};
	use commons_types::backup::semantics;

	for declaration in RestoreReplica::list_for_group(db, group).await? {
		if !declaration.enabled {
			continue;
		}
		let advertises =
			RestoreConsumerCapability::list_for_consumer(db, declaration.consumer_device_id)
				.await?
				.into_iter()
				.any(|d| {
					d.intent == declaration.intent && d.has_semantic(semantics::REPORTING_SCHEMA)
				});
		if advertises {
			return Ok(true);
		}
	}

	Ok(false)
}
