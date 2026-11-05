//! The packages Canopy matches against published vulnerabilities: the
//! dependencies of Tamanu versions, and the Linux packages a machine reports.
//!
//! Spec: `.workhorse/specs/public-server/packages.md` (id `PKG`) and
//! `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Sources".

use commons_errors::{AppError, Result};
use diesel::{prelude::*, sql_types};
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// The OSV ecosystem Tamanu's dependencies are matched in.
pub const NPM: &str = "npm";

/// A package of a Tamanu version's lockfile, and where it sits in the tree.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct VersionPackage {
	pub name: String,
	pub version: String,
	/// The workspaces that reach the package.
	pub workspaces: Vec<String>,
	/// Whether any workspace depends on it directly.
	pub direct: bool,
	/// How each workspace reaches it: workspace name to the chain of packages
	/// from the workspace's own dependency down to this one.
	pub paths: std::collections::BTreeMap<String, Vec<String>>,
}

impl VersionPackage {
	/// Whether a version's lockfile has been read.
	pub async fn lockfile_read(conn: &mut AsyncPgConnection, version_id: Uuid) -> Result<bool> {
		use crate::schema::version_lockfiles::dsl;
		let read: Option<Uuid> = dsl::version_lockfiles
			.select(dsl::version_id)
			.filter(dsl::version_id.eq(version_id))
			.first(conn)
			.await
			.optional()?;
		Ok(read.is_some())
	}

	/// Record a version's packages, once. Reading the lockfile again changes
	/// nothing: a release tag does not move. The marker and the packages are
	/// written together, so a version is never marked read with part of its
	/// list.
	pub async fn store(
		conn: &mut AsyncPgConnection,
		version_id: Uuid,
		packages: &[Self],
	) -> Result<()> {
		use crate::schema::{version_lockfiles, version_packages};

		conn.transaction::<_, AppError, _>(async |conn| {
			let marked = diesel::insert_into(version_lockfiles::table)
				.values(version_lockfiles::version_id.eq(version_id))
				.on_conflict_do_nothing()
				.execute(conn)
				.await?;
			if marked == 0 {
				return Ok(());
			}

			for chunk in packages.chunks(2000) {
				let rows: Vec<_> = chunk
					.iter()
					.map(|package| {
						(
							version_packages::version_id.eq(version_id),
							version_packages::name.eq(&package.name),
							version_packages::version.eq(&package.version),
							version_packages::workspaces.eq(&package.workspaces),
							version_packages::direct.eq(package.direct),
							version_packages::paths
								.eq(serde_json::to_value(&package.paths).unwrap_or_default()),
						)
					})
					.collect();
				diesel::insert_into(version_packages::table)
					.values(&rows)
					.on_conflict_do_nothing()
					.execute(conn)
					.await?;
			}
			Ok(())
		})
		.await
	}
}

/// What a machine reports: the distribution, its release, and every installed
/// package by source name and source version.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct PackageReport {
	pub distribution: String,
	pub release: String,
	pub packages: Vec<ReportedPackage>,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct ReportedPackage {
	pub name: String,
	pub version: String,
}

impl PackageReport {
	/// The OSV ecosystem the report is placed in, or `None` for a distribution
	/// or release Canopy cannot place.
	///
	/// Ubuntu releases are named with their long-term support marking where
	/// they carry one: the April release of an even year.
	pub fn ecosystem(&self) -> Option<String> {
		let release = self.release.trim();
		match self.distribution.trim().to_ascii_lowercase().as_str() {
			"ubuntu" => {
				let (year, month) = release.split_once('.')?;
				let (year_n, month_n) = (year.parse::<u32>().ok()?, month.parse::<u32>().ok()?);
				let lts = year_n % 2 == 0 && month_n == 4;
				Some(if lts {
					format!("Ubuntu:{year}.{month}:LTS")
				} else {
					format!("Ubuntu:{year}.{month}")
				})
			}
			"debian" => {
				let major = release.split('.').next()?;
				major.parse::<u32>().ok()?;
				Some(format!("Debian:{major}"))
			}
			_ => None,
		}
	}

	/// Over what the report says, in a fixed order, so the same list always has
	/// the same digest.
	pub fn digest(&self) -> String {
		let mut packages: Vec<&ReportedPackage> = self.packages.iter().collect();
		packages.sort();
		packages.dedup();
		let mut hash = Sha256::new();
		hash.update(self.distribution.as_bytes());
		hash.update([0]);
		hash.update(self.release.as_bytes());
		for package in packages {
			hash.update([0]);
			hash.update(package.name.as_bytes());
			hash.update([1]);
			hash.update(package.version.as_bytes());
		}
		hex::encode(hash.finalize())
	}
}

/// What storing a report did.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Reported {
	/// Identical to the last report: nothing changed.
	Unchanged,
	/// Replaced the list.
	Changed {
		/// The distinct packages now held.
		package_count: i64,
		/// The ecosystem it was placed in, `None` when it could not be.
		ecosystem: Option<String>,
	},
}

/// The last report a machine made.
#[derive(
	Clone, Debug, PartialEq, Eq, Serialize, Deserialize, QueryableByName, utoipa::ToSchema,
)]
#[schema(as = MachinePackageReport)]
pub struct MachinePackageReport {
	#[diesel(sql_type = sql_types::Uuid)]
	pub machine_id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	pub distribution: String,
	#[diesel(sql_type = sql_types::Text)]
	pub release: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub ecosystem: Option<String>,
	#[diesel(sql_type = sql_types::Integer)]
	pub package_count: i32,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	pub reported_at: Timestamp,
}

impl MachinePackageReport {
	/// Replace a machine's package list with `report`, unless it is identical to
	/// the one held.
	pub async fn store(
		conn: &mut AsyncPgConnection,
		machine_id: Uuid,
		report: &PackageReport,
	) -> Result<Reported> {
		use crate::schema::{machine_package_reports as reports, machine_packages as packages};

		let digest = report.digest();
		let held: Option<String> = reports::table
			.select(reports::digest)
			.filter(reports::machine_id.eq(machine_id))
			.first(conn)
			.await
			.optional()?;
		if held.as_deref() == Some(digest.as_str()) {
			return Ok(Reported::Unchanged);
		}

		let mut distinct: Vec<&ReportedPackage> = report.packages.iter().collect();
		distinct.sort();
		distinct.dedup();
		let ecosystem = report.ecosystem();
		let count = i32::try_from(distinct.len())
			.map_err(|_| AppError::BadRequest("too many packages".into()))?;

		diesel::insert_into(reports::table)
			.values((
				reports::machine_id.eq(machine_id),
				reports::distribution.eq(&report.distribution),
				reports::release.eq(&report.release),
				reports::ecosystem.eq(&ecosystem),
				reports::digest.eq(&digest),
				reports::package_count.eq(count),
			))
			.on_conflict(reports::machine_id)
			.do_update()
			.set((
				reports::distribution.eq(&report.distribution),
				reports::release.eq(&report.release),
				reports::ecosystem.eq(&ecosystem),
				reports::digest.eq(&digest),
				reports::package_count.eq(count),
				reports::reported_at.eq(diesel::dsl::now),
			))
			.execute(conn)
			.await?;

		diesel::delete(packages::table.filter(packages::machine_id.eq(machine_id)))
			.execute(conn)
			.await?;
		for chunk in distinct.chunks(2000) {
			let rows: Vec<_> = chunk
				.iter()
				.map(|package| {
					(
						packages::machine_id.eq(machine_id),
						packages::name.eq(&package.name),
						packages::version.eq(&package.version),
					)
				})
				.collect();
			diesel::insert_into(packages::table)
				.values(&rows)
				.execute(conn)
				.await?;
		}

		Ok(Reported::Changed {
			package_count: i64::from(count),
			ecosystem,
		})
	}

	/// What a machine last reported, if it ever has.
	pub async fn get(conn: &mut AsyncPgConnection, machine_id: Uuid) -> Result<Option<Self>> {
		diesel::sql_query(
			"SELECT machine_id, distribution, release, ecosystem, package_count, reported_at \
			 FROM machine_package_reports WHERE machine_id = $1",
		)
		.bind::<sql_types::Uuid, _>(machine_id)
		.get_result(conn)
		.await
		.optional()
		.map_err(AppError::from)
	}
}

#[cfg(test)]
mod tests {
	use super::*;

	fn report(distribution: &str, release: &str) -> PackageReport {
		PackageReport {
			distribution: distribution.into(),
			release: release.into(),
			packages: vec![],
		}
	}

	#[test]
	fn ubuntu_lts_and_interim_releases_are_placed() {
		assert_eq!(
			report("ubuntu", "24.04").ecosystem().as_deref(),
			Some("Ubuntu:24.04:LTS")
		);
		assert_eq!(
			report("ubuntu", "22.04").ecosystem().as_deref(),
			Some("Ubuntu:22.04:LTS")
		);
		assert_eq!(
			report("ubuntu", "24.10").ecosystem().as_deref(),
			Some("Ubuntu:24.10")
		);
		assert_eq!(
			report("ubuntu", "23.04").ecosystem().as_deref(),
			Some("Ubuntu:23.04")
		);
	}

	#[test]
	fn debian_is_placed_by_its_major() {
		assert_eq!(
			report("debian", "12").ecosystem().as_deref(),
			Some("Debian:12")
		);
		assert_eq!(
			report("debian", "12.5").ecosystem().as_deref(),
			Some("Debian:12")
		);
	}

	#[test]
	fn an_unknown_distribution_or_release_is_not_placed() {
		assert_eq!(report("alpine", "3.20").ecosystem(), None);
		assert_eq!(report("ubuntu", "noble").ecosystem(), None);
		assert_eq!(report("debian", "sid").ecosystem(), None);
	}

	#[test]
	fn the_digest_ignores_package_order_and_repeats() {
		let package = |name: &str, version: &str| ReportedPackage {
			name: name.into(),
			version: version.into(),
		};
		let mut a = report("ubuntu", "24.04");
		a.packages = vec![package("openssl", "3.0.13"), package("bash", "5.2")];
		let mut b = a.clone();
		b.packages = vec![
			package("bash", "5.2"),
			package("openssl", "3.0.13"),
			package("bash", "5.2"),
		];
		assert_eq!(a.digest(), b.digest());
		b.packages[0].version = "5.3".into();
		assert_ne!(a.digest(), b.digest());
	}
}
