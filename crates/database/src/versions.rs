use commons_errors::{AppError, Result};
use commons_types::version::{VersionStatus, VersionStr};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[macro_export]
macro_rules! predicate_version {
	($version:expr) => {{
		use ::diesel::BoolExpressionMethods as _;
		use $crate::schema::versions::dsl::*;
		let ::node_semver::Version {
			major: target_major,
			minor: target_minor,
			patch: target_patch,
			..
		} = $version;

		major
			.eq(target_major as i32)
			.and(minor.eq(target_minor as i32))
			.and(patch.eq(target_patch as i32))
	}};
}
pub use predicate_version;

/// SQL predicate for "this row is at or above `floor`", comparing
/// `(major, minor, patch)` **lexicographically**, the way semver orders
/// versions — not component-wise. `2.6.0` is above `2.5.1` despite its lower
/// patch, and `3.0.0` is above `2.5.1` despite its lower minor.
///
/// Only ever a prefilter ahead of the real [`node_semver::Range::satisfies`]
/// check, so what matters is that it never excludes a row the range accepts.
/// Prerelease ordering is left to that check: a row whose triple ties with
/// `floor` is kept regardless of prerelease.
pub fn at_or_above_version(
	floor: &node_semver::Version,
) -> Box<
	dyn BoxableExpression<
			crate::schema::versions::table,
			diesel::pg::Pg,
			SqlType = diesel::sql_types::Bool,
		>,
> {
	use crate::schema::versions::dsl::{major, minor, patch};

	let (floor_major, floor_minor, floor_patch) =
		(floor.major as i32, floor.minor as i32, floor.patch as i32);

	Box::new(
		major.gt(floor_major).or(major.eq(floor_major).and(
			minor
				.gt(floor_minor)
				.or(minor.eq(floor_minor).and(patch.ge(floor_patch))),
		)),
	)
}

/// A release version of the monitored software, with its publication
/// status and changelog.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, QueryableByName, utoipa::ToSchema,
)]
#[diesel(table_name = crate::schema::versions)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Version {
	/// Unique identifier of the version.
	pub id: Uuid,
	/// When the version record was created.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When the version record was last changed.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// Major version number.
	pub major: i32,
	/// Minor version number.
	pub minor: i32,
	/// Patch version number.
	pub patch: i32,
	/// Publication status: `draft`, `published`, or `yanked`.
	#[diesel(deserialize_as = String, serialize_as = String)]
	pub status: VersionStatus,
	/// Changelog text for this version, as Markdown.
	pub changelog: String,
	/// The releaser device that published this version, if it was published
	/// by a device rather than created by an operator.
	pub device_id: Option<Uuid>,
}

/// A release version as returned by the version-listing endpoints: the
/// version numbers, publication status, and changelog.
#[derive(
	Debug, Clone, Serialize, Deserialize, Queryable, Selectable, QueryableByName, utoipa::ToSchema,
)]
#[diesel(table_name = crate::views::version_updates)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ViewVersion {
	/// Unique identifier of the version.
	pub id: Uuid,
	/// Major version number.
	pub major: i32,
	/// Minor version number.
	pub minor: i32,
	/// Patch version number.
	pub patch: i32,
	/// Publication status: `draft`, `published`, or `yanked`.
	#[diesel(deserialize_as = String, serialize_as = String)]
	pub status: VersionStatus,
	/// Changelog text for this version, as Markdown.
	pub changelog: String,
}

#[derive(Debug, Deserialize, Insertable)]
#[diesel(table_name = crate::schema::versions)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewVersion {
	pub major: i32,
	pub minor: i32,
	pub patch: i32,
	pub changelog: String,
	pub status: VersionStatus,
	pub device_id: Option<Uuid>,
}

impl Version {
	pub fn as_semver(&self) -> node_semver::Version {
		node_semver::Version::new(self.major as _, self.minor as _, self.patch as _)
	}

	pub async fn get_all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::versions::*;

		table
			.select(Version::as_select())
			.filter(status.eq(VersionStatus::Published))
			.order_by(major.desc())
			.then_order_by(minor.desc())
			.then_order_by(patch.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_all_including_drafts(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::versions::*;

		table
			.select(Version::as_select())
			.order_by(major.desc())
			.then_order_by(minor.desc())
			.then_order_by(patch.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_by_version(db: &mut AsyncPgConnection, version: VersionStr) -> Result<Self> {
		use crate::schema::versions::*;

		table
			.filter(predicate_version!(version.0))
			.select(Version::as_select())
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// The release rows for these exact versions, in one query. A version with
	/// no row is absent from the result rather than an error.
	pub async fn get_by_versions(
		db: &mut AsyncPgConnection,
		wanted: &[VersionStr],
	) -> Result<Vec<Self>> {
		use crate::schema::versions::dsl::*;

		if wanted.is_empty() {
			return Ok(Vec::new());
		}

		// The SQL narrows on each component and the triple is matched here: one
		// predicate per version builds a boxed OR chain as long as the fleet's
		// version spread, while three set predicates leave Postgres a cross
		// product small enough to sift in memory.
		let mut majors: Vec<i32> = wanted.iter().map(|want| want.0.major as i32).collect();
		majors.sort_unstable();
		majors.dedup();

		let mut minors: Vec<i32> = wanted.iter().map(|want| want.0.minor as i32).collect();
		minors.sort_unstable();
		minors.dedup();

		let mut patches: Vec<i32> = wanted.iter().map(|want| want.0.patch as i32).collect();
		patches.sort_unstable();
		patches.dedup();

		let rows: Vec<Self> = versions
			.filter(major.eq_any(majors))
			.filter(minor.eq_any(minors))
			.filter(patch.eq_any(patches))
			.select(Version::as_select())
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter(|row| {
				wanted.iter().any(|want| {
					(row.major, row.minor, row.patch)
						== (
							want.0.major as i32,
							want.0.minor as i32,
							want.0.patch as i32,
						)
				})
			})
			.collect())
	}

	pub async fn get_by_id(db: &mut AsyncPgConnection, version_id: Uuid) -> Result<Self> {
		use crate::schema::versions::dsl::*;

		versions
			.filter(id.eq(version_id))
			.select(Version::as_select())
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// The newest published patch of every `(major, minor)` line above
	/// `version`, within the same major.
	///
	/// The `version_updates` view ranks published rows only, so a line whose
	/// newest patch is draft or yanked still offers its newest *published*
	/// patch rather than dropping out entirely. The `status` filter below is
	/// belt-and-braces on top of that.
	pub async fn get_updates_for_version(
		db: &mut AsyncPgConnection,
		version: VersionStr,
	) -> Result<Vec<ViewVersion>> {
		use crate::views::version_updates::dsl::*;
		let node_semver::Version {
			major: target_major,
			minor: target_minor,
			patch: target_patch,
			..
		} = version.0;
		version_updates
			.filter(
				major
					.eq(target_major as i32)
					.and(status.eq(VersionStatus::Published))
					.and(
						minor.gt(target_minor as i32).or(minor
							.eq(target_minor as i32)
							.and(patch.gt(target_patch as i32))),
					),
			)
			.order_by(minor)
			.select(version_updates::all_columns())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_latest_matching(
		db: &mut AsyncPgConnection,
		range: node_semver::Range,
	) -> Result<Self> {
		use crate::schema::versions::*;

		let floor = range.min_version().ok_or(AppError::UnusableRange)?;

		table
			.select(Version::as_select())
			.filter(status.eq(VersionStatus::Published))
			.filter(at_or_above_version(&floor))
			.order_by(major.desc())
			.then_order_by(minor.desc())
			.then_order_by(patch.desc())
			.load(db)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.find(|v| range.satisfies(&v.as_semver()))
			.ok_or(AppError::NoMatchingVersions)
	}

	pub async fn get_head_release_date(
		db: &mut AsyncPgConnection,
		version: VersionStr,
	) -> Result<Timestamp> {
		use crate::schema::versions::*;

		let node_semver::Version {
			major: target_major,
			minor: target_minor,
			..
		} = version.0;

		table
			.select(Version::as_select())
			.filter(
				major
					.eq(target_major as i32)
					.and(minor.eq(target_minor as i32))
					.and(patch.eq(0)),
			)
			.first(db)
			.await
			.map(|v: Version| v.created_at)
			.map_err(AppError::from)
	}

	pub async fn update_status(
		db: &mut AsyncPgConnection,
		version: VersionStr,
		new_status: VersionStatus,
	) -> Result<()> {
		use crate::schema::versions::dsl::*;

		diesel::update(versions)
			.filter(predicate_version!(version.0))
			.set(status.eq(new_status))
			.execute(db)
			.await?;

		Ok(())
	}

	pub async fn update_changelog(
		db: &mut AsyncPgConnection,
		version: VersionStr,
		new_changelog: String,
	) -> Result<()> {
		use crate::schema::versions::dsl::*;

		diesel::update(versions)
			.filter(predicate_version!(version.0))
			.set(changelog.eq(new_changelog))
			.execute(db)
			.await?;

		Ok(())
	}

	pub async fn update_device_id(
		db: &mut AsyncPgConnection,
		version: VersionStr,
		new_device_id: Uuid,
	) -> Result<()> {
		use crate::schema::versions::dsl::*;

		diesel::update(versions)
			.filter(predicate_version!(version.0))
			.set(device_id.eq(new_device_id))
			.execute(db)
			.await?;

		Ok(())
	}

	pub async fn is_latest_in_minor(
		db: &mut AsyncPgConnection,
		version: VersionStr,
	) -> Result<bool> {
		use crate::schema::versions::dsl::*;

		let version_record = Self::get_by_version(db, version.clone()).await?;

		// `.optional()`, not `.ok()`: the latter folded every Diesel error —
		// a dropped connection, a serialisation failure — into `None`, and
		// `None` here reports "yes, latest". That is the permissive answer,
		// and it is the one guarding the publish-to-draft demotion in
		// `fns::versions::update_version_status`, so a transient database
		// fault would wave through a demotion the guard exists to refuse.
		let latest_in_minor: Option<Version> = versions
			.filter(major.eq(version_record.major))
			.filter(minor.eq(version_record.minor))
			.filter(status.eq(VersionStatus::Published))
			.order_by(patch.desc())
			.select(Version::as_select())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;

		Ok(latest_in_minor
			.as_ref()
			.map(|v| v.patch == version_record.patch)
			.unwrap_or(true))
	}

	pub async fn get_all_in_minor(
		db: &mut AsyncPgConnection,
		version: VersionStr,
	) -> Result<Vec<Self>> {
		use crate::schema::versions::dsl::*;

		let node_semver::Version {
			major: target_major,
			minor: target_minor,
			patch: target_patch,
			..
		} = version.0;

		versions
			.filter(major.eq(target_major as i32))
			.filter(minor.eq(target_minor as i32))
			.filter(patch.lt(target_patch as i32))
			.filter(status.ne(VersionStatus::Draft))
			.order_by(patch.desc())
			.select(Version::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}
}
