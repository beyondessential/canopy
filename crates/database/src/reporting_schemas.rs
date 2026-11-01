//! Reporting-schema builds: which pairs of a group's environment and a Tamanu
//! version have a schema, which have been tried, and which an operator has
//! asked for again.
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
use commons_types::{backup::RunOutcome, server::rank::ServerRank};

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
	/// The environment within the group the schema was built for. `null` on a
	/// build recorded before a schema named one, which no pair matches.
	pub rank: Option<ServerRank>,
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
	pub rank: ServerRank,
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
		let began_at = match report.run_id {
			Some(run) => run_started_at(db, run).await?.unwrap_or(report.observed_at),
			None => report.observed_at,
		};

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
				crate::schema::reporting_schema_builds::rank.eq(build.rank),
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
		// whichever way it went. A build takes half an hour, and an ask entered
		// while it ran is for whatever changed after it began, so what answers
		// that one is the next build rather than this.
		ReportingSchemaRequest::clear(db, build.group_id, build.rank, build.version_id, began_at)
			.await?;

		Ok(check_id)
	}

	/// The most recent build of a pair, if it has been tried.
	pub async fn latest_for_pair(
		db: &mut AsyncPgConnection,
		group: Uuid,
		rank: ServerRank,
		version: Uuid,
	) -> Result<Option<Self>> {
		use crate::schema::{backup_restore_checks, reporting_schema_builds};

		reporting_schema_builds::table
			.inner_join(
				backup_restore_checks::table
					.on(backup_restore_checks::id.eq(reporting_schema_builds::check_id)),
			)
			.filter(reporting_schema_builds::group_id.eq(group))
			.filter(reporting_schema_builds::rank.eq(rank))
			.filter(reporting_schema_builds::version_id.eq(version))
			.order_by(backup_restore_checks::reported_at.desc())
			.select(Self::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The most recent build of each of a group's pairs, by environment and
	/// version.
	///
	/// One query rather than one per pair: this backs both the operator page
	/// and the sweep, which walk every pair a group has. A build recorded
	/// before a schema named an environment is left out, since there is no
	/// environment it can be said to be the build of.
	pub async fn latest_by_pair_for_group(
		db: &mut AsyncPgConnection,
		group: Uuid,
	) -> Result<std::collections::HashMap<(ServerRank, Uuid), Self>> {
		use crate::schema::{backup_restore_checks, reporting_schema_builds};

		let builds: Vec<Self> = reporting_schema_builds::table
			.inner_join(
				backup_restore_checks::table
					.on(backup_restore_checks::id.eq(reporting_schema_builds::check_id)),
			)
			.filter(reporting_schema_builds::group_id.eq(group))
			.filter(reporting_schema_builds::rank.is_not_null())
			.distinct_on((
				reporting_schema_builds::rank,
				reporting_schema_builds::version_id,
			))
			.order_by((
				reporting_schema_builds::rank,
				reporting_schema_builds::version_id,
				backup_restore_checks::reported_at.desc(),
			))
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(builds
			.into_iter()
			.filter_map(|b| b.rank.map(|rank| ((rank, b.version_id), b)))
			.collect())
	}

	/// Whether a pair is settled: it has been built or has failed, and either
	/// way is not dispatched again until the version's artifacts change or an
	/// operator asks.
	// spec: RPT#pairs
	pub async fn is_settled(
		db: &mut AsyncPgConnection,
		group: Uuid,
		rank: ServerRank,
		version: Uuid,
	) -> Result<bool> {
		let row = Version::get_by_id(db, version).await?;
		let ranges = crate::artifacts::RangeChanges::load(db).await?;
		let settlement =
			Settlement::for_group(db, group, std::slice::from_ref(&row), &ranges).await?;
		Ok(settlement.settled(rank, version))
	}
}

/// Where every pair of a group stands, answered from memory.
///
/// The worklist asks this of each of a group's versions on every poll, and
/// every restore consumer polls on a schedule, so the three lookups it takes
/// are made once for the group rather than once per pair.
// spec: RPT#pairs
pub struct Settlement {
	requested: std::collections::HashSet<(ServerRank, Uuid)>,
	builds: std::collections::HashMap<(ServerRank, Uuid), ReportingSchemaBuild>,
	changed: std::collections::HashMap<Uuid, Timestamp>,
}

impl Settlement {
	pub async fn for_group(
		db: &mut AsyncPgConnection,
		group: Uuid,
		versions: &[Version],
		ranges: &crate::artifacts::RangeChanges,
	) -> Result<Self> {
		Ok(Self {
			requested: ReportingSchemaRequest::pending_for_group(db, group).await?,
			builds: ReportingSchemaBuild::latest_by_pair_for_group(db, group).await?,
			changed: crate::artifacts::Artifact::newest_change_for_versions(db, versions, ranges)
				.await?,
		})
	}

	pub fn settled(&self, rank: ServerRank, version: Uuid) -> bool {
		if self.requested.contains(&(rank, version)) {
			return false;
		}

		let Some(build) = self.builds.get(&(rank, version)) else {
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

/// When the run behind a report began, read from the first credential it was
/// issued.
///
/// A run reports once it is over, so its own timestamp is the far end of a
/// window half an hour wide, and what it started before is the question an ask
/// made inside that window turns on.
async fn run_started_at(db: &mut AsyncPgConnection, run: Uuid) -> Result<Option<Timestamp>> {
	use crate::schema::backup_credential_issuances::dsl;

	let issued: Option<jiff_diesel::Timestamp> = dsl::backup_credential_issuances
		.filter(dsl::run_id.eq(Some(run)))
		.select(diesel::dsl::min(dsl::issued_at))
		.first(db)
		.await
		.map_err(AppError::from)?;

	Ok(issued.map(Into::into))
}

/// An operator asking for a pair's build.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::reporting_schema_requests)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ReportingSchemaRequest {
	pub group_id: Uuid,
	pub rank: ServerRank,
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
		rank: ServerRank,
		version: Uuid,
		requested_by: Option<&str>,
	) -> Result<()> {
		use crate::schema::reporting_schema_requests::dsl;

		diesel::insert_into(dsl::reporting_schema_requests)
			.values((
				dsl::group_id.eq(group),
				dsl::rank.eq(rank),
				dsl::version_id.eq(version),
				dsl::requested_by.eq(requested_by),
			))
			.on_conflict((dsl::group_id, dsl::rank, dsl::version_id))
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

	/// Which of a group's pairs have an ask pending, in one query.
	pub async fn pending_for_group(
		db: &mut AsyncPgConnection,
		group: Uuid,
	) -> Result<std::collections::HashSet<(ServerRank, Uuid)>> {
		use crate::schema::reporting_schema_requests::dsl;

		let pairs: Vec<(ServerRank, Uuid)> = dsl::reporting_schema_requests
			.filter(dsl::group_id.eq(group))
			.select((dsl::rank, dsl::version_id))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(pairs.into_iter().collect())
	}

	/// Clear a pair's ask, where it was made before `answered_at`.
	async fn clear(
		db: &mut AsyncPgConnection,
		group: Uuid,
		rank: ServerRank,
		version: Uuid,
		answered_at: Timestamp,
	) -> Result<()> {
		use crate::schema::reporting_schema_requests::dsl;

		diesel::delete(
			dsl::reporting_schema_requests
				.filter(dsl::group_id.eq(group))
				.filter(dsl::rank.eq(rank))
				.filter(dsl::version_id.eq(version))
				.filter(dsl::requested_at.lt(jiff_diesel::Timestamp::from(answered_at))),
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

/// One pair of a group's environment and a Tamanu version, and where it
/// stands.
#[derive(Debug, Clone, Serialize, utoipa::ToSchema)]
pub struct Pair {
	/// The group this pair is for.
	pub group_id: Uuid,
	/// The environment within that group this pair is for.
	pub rank: ServerRank,
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
	/// The environment's Tamanu applications reporting this version, by name.
	/// Empty where the pair comes from the open plan rather than from
	/// something running it.
	pub applications: Vec<String>,
}

/// The pairs of a group: for each of its environments, every published version
/// that environment's Tamanu applications report running, plus the version its
/// open plan moves it to.
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
	let builds = ReportingSchemaBuild::latest_by_pair_for_group(db, group).await?;
	let requests = ReportingSchemaRequest::pending_for_group(db, group).await?;

	let mut pairs = Vec::with_capacity(versions.len());
	for (rank, version, applications) in versions {
		let (state, error) = match builds.get(&(rank, version.id)) {
			None => (PairState::Awaiting, None),
			Some(build) if build.built => (PairState::Built, None),
			Some(build) => (PairState::Failed, build.error.clone()),
		};

		pairs.push(Pair {
			group_id: group,
			rank,
			version_id: version.id,
			applications,
			version: version.as_semver().to_string(),
			state,
			error,
			requested: requests.contains(&(rank, version.id)),
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
	versions_of_members(db, group, &members).await
}

/// A group's pairs as environment and version, from members already in hand.
///
/// The dispatch needs the environment each version belongs to, since the
/// central it restores is that environment's.
// spec: RPT#the-build-contract
pub async fn pairs_of_live_members(
	db: &mut AsyncPgConnection,
	group: Uuid,
	members: &[crate::applications::Application],
) -> Result<Vec<(ServerRank, Version)>> {
	Ok(versions_and_applications(db, group, members)
		.await?
		.into_iter()
		.map(|(rank, version, _)| (rank, version))
		.collect())
}

/// The versions of a group's pairs, from members already in hand.
pub async fn versions_of_members(
	db: &mut AsyncPgConnection,
	group: Uuid,
	members: &[crate::applications::Application],
) -> Result<Vec<Version>> {
	let mut versions: Vec<Version> = Vec::new();
	for (_, version, _) in versions_and_applications(db, group, members).await? {
		if !versions.iter().any(|held| held.id == version.id) {
			versions.push(version);
		}
	}
	Ok(versions)
}

/// A group's pairs, and which of its Tamanu applications report each one.
///
/// The applications are carried alongside the version rather than joined back
/// on a stringified semver: a `Version` row holds major, minor and patch alone,
/// so a reported `2.60.0-rc1` resolves to the 2.60.0 row and would never match
/// its own key, and the pair would read as an upgrade plan while a server runs
/// it.
///
/// A pair is one environment's, so two environments of a group on the same
/// version are two pairs rather than one. Their configuration is what a schema
/// follows from and it is not the same configuration, so one schema between
/// them would be the wrong one for whichever it was not built against.
// spec: RPT#pairs
async fn versions_and_applications(
	db: &mut AsyncPgConnection,
	group: Uuid,
	applications: &[crate::applications::Application],
) -> Result<Vec<(ServerRank, Version, Vec<String>)>> {
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

	let mut pairs: Vec<(ServerRank, Version, Vec<String>)> = Vec::new();
	for application in &tamanu {
		// A pending application is in none of the group's environments, so
		// there is no environment to build a schema for it against.
		// spec: GRP#environments
		let Some(rank) = application.rank else {
			continue;
		};
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

		// A pair is unique per environment and version, so two applications of
		// one environment on one version are one pair carrying both names.
		match pairs
			.iter_mut()
			.find(|(r, v, _)| *r == rank && v.id == version.id)
		{
			Some((_, _, names)) => names.push(application.display_name()),
			None => pairs.push((rank, version.clone(), vec![application.display_name()])),
		}
	}

	// A plan is opened against an environment, and each environment of the
	// group holds its own, so the target is asked for per environment rather
	// than once for the group's central.
	let mut environments: Vec<ServerRank> = tamanu.iter().filter_map(|a| a.rank).collect();
	environments.sort();
	environments.dedup();

	for rank in environments {
		let Some(target) = crate::upgrade_plans::planned_target(db, group, rank).await? else {
			continue;
		};
		// A plan moving an environment to a version something in it already
		// runs adds no pair. Dispatch counts a restore and a migrate per entry,
		// so a duplicate here is paid for rather than merely untidy.
		if !pairs
			.iter()
			.any(|(r, v, _)| *r == rank && v.id == target.id)
		{
			pairs.push((rank, target, Vec::new()));
		}
	}

	for (_, _, names) in &mut pairs {
		names.sort();
	}
	pairs.sort_by_key(|(r, v, _)| (*r, v.major, v.minor, v.patch));

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
		backup::{refs, staleness::applications_with_open_issue},
		issues::{CheckInstance, GradedCheck, Scope},
		restore::{RestoreCheck, file_restore_check},
		server_groups::ServerGroup,
	};
	use commons_types::status::CheckResult;

	// Which groups have a builder is one question of the whole fleet rather than
	// one per group: asking per group walked every group's declarations and
	// every declaration's consumer, once a minute, for groups that have none.
	let builders = groups_building_schemas(db).await?;

	// A group that has stopped building still has whatever this check filed
	// while it did, and filing no instances is what says the finding is gone.
	// Asked of the fleet at once, it is also what keeps this to the groups the
	// sweep has something to say about: every other group is walked, its
	// members loaded and its issues probed, once a minute, to file nothing.
	// spec: RPT#alerting
	let open = applications_with_open_issue(db, refs::REPORTING_SCHEMA).await?;
	let walk: std::collections::HashSet<Uuid> = builders
		.iter()
		.copied()
		.chain(open.iter().filter_map(|(_, group)| *group))
		.collect();

	for group in ServerGroup::list_all(db).await? {
		if !walk.contains(&group.id) {
			continue;
		}

		let members = Application::list_live_in_group(db, group.id).await?;
		// The check files on the group's central. A group that has lost it
		// keeps the finding open against whichever application it was filed on,
		// which is the only scope a recovery reaches it through.
		let Some(central) = ServerGroup::canonical_central(&members)
			.map(|a| a.id)
			.or_else(|| {
				open.iter()
					.find(|(_, g)| *g == Some(group.id))
					.map(|(application, _)| *application)
			})
		else {
			continue;
		};

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
					key: pair.version.clone(),
					label: None,
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
			&move |graded: &GradedCheck| match graded.degraded().as_slice() {
				[] => format!("Reporting schemas are built for every version {name} runs"),
				[one] => format!(
					"No reporting schema for {name} on {}: {}",
					one.name(),
					one.detail
						.as_ref()
						.and_then(|d| d.get("why"))
						.and_then(|v| v.as_str())
						.unwrap_or("the build failed")
				),
				many => format!(
					"No reporting schema for {} of {total} versions {name} runs: {}",
					many.len(),
					many.iter().map(|i| i.name()).collect::<Vec<_>>().join(", ")
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
