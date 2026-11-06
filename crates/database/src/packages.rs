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

/// The OSV ecosystem of npm packages.
pub const NPM: &str = "npm";

/// The OSV ecosystem of Cargo crates.
pub const CARGO: &str = "crates.io";

/// The OSV ecosystem of Python packages.
pub const PYPI: &str = "PyPI";

/// The ecosystems whose packages come from a repository's lockfiles.
pub const REPOSITORY_ECOSYSTEMS: [&str; 3] = [NPM, CARGO, PYPI];

/// When a package is used, in the terms SPDX gives a dependency's lifecycle.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityScope)]
pub enum Scope {
	Runtime,
	Build,
	Development,
	Test,
	Design,
	Other,
}

impl Scope {
	pub const ALL: [Self; 6] = [
		Self::Runtime,
		Self::Build,
		Self::Development,
		Self::Test,
		Self::Design,
		Self::Other,
	];

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Runtime => "runtime",
			Self::Build => "build",
			Self::Development => "development",
			Self::Test => "test",
			Self::Design => "design",
			Self::Other => "other",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|scope| scope.as_str() == s)
	}
}

/// How a workspace package reaches a package by paths of one scope.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityPackagePath)]
pub struct PackagePath {
	/// The workspace package that reaches it.
	pub workspace_package: String,
	/// The scope of the paths it takes.
	pub scope: Scope,
	/// Whether the package may not be installed.
	pub optional: bool,
	/// Whether the workspace package depends on it directly.
	pub direct: bool,
	/// The shortest chain of packages along such a path, from the workspace
	/// package's own dependency down to this package.
	pub chain: Vec<String>,
}

/// A package a repository's default branch carries, and where it sits.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryPackage {
	pub ecosystem: String,
	pub name: String,
	pub version: String,
	pub paths: Vec<PackagePath>,
}

/// The last read of a repository's default branch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, QueryableByName)]
pub struct RepositoryRead {
	#[diesel(sql_type = sql_types::Text)]
	pub repository: String,
	/// The commit the default branch was at, `None` before it was first read.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub commit_sha: Option<String>,
	/// When it was last read, or its head last found unchanged.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>, deserialize_as = jiff_diesel::NullableTimestamp)]
	pub read_at: Option<Timestamp>,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	pub last_attempt_at: Timestamp,
	/// Why the last attempt failed, `None` when it did not.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub failure: Option<String>,
}

impl RepositoryRead {
	pub async fn get(conn: &mut AsyncPgConnection, repository: &str) -> Result<Option<Self>> {
		diesel::sql_query(
			"SELECT repository, commit_sha, read_at, last_attempt_at, failure \
			 FROM repository_reads WHERE repository = $1",
		)
		.bind::<sql_types::Text, _>(repository)
		.get_result(conn)
		.await
		.optional()
		.map_err(AppError::from)
	}

	/// Replace what is held for a repository with what its default branch
	/// carried at `commit`, in one transaction, so a repository is never held
	/// with part of a read. A package reached from two lockfiles is held once,
	/// with the shortest chain of each of its paths.
	pub async fn store(
		conn: &mut AsyncPgConnection,
		repository: &str,
		commit: &str,
		packages: &[RepositoryPackage],
	) -> Result<()> {
		use crate::schema::{repository_package_paths as paths, repository_packages as held};

		let mut merged: std::collections::BTreeMap<
			(&str, &str, &str),
			std::collections::BTreeMap<(&str, Scope, bool), (bool, &[String])>,
		> = Default::default();
		for package in packages {
			let by_path = merged
				.entry((&package.ecosystem, &package.name, &package.version))
				.or_default();
			for path in &package.paths {
				let entry = by_path
					.entry((path.workspace_package.as_str(), path.scope, path.optional))
					.or_insert((path.direct, path.chain.as_slice()));
				entry.0 |= path.direct;
				if path.chain.len() < entry.1.len() {
					entry.1 = path.chain.as_slice();
				}
			}
		}

		conn.transaction::<_, AppError, _>(async |conn| {
			diesel::delete(held::table.filter(held::repository.eq(repository)))
				.execute(conn)
				.await?;
			let keys: Vec<_> = merged.keys().collect();
			for chunk in keys.chunks(2000) {
				let rows: Vec<_> = chunk
					.iter()
					.map(|(ecosystem, name, version)| {
						(
							held::repository.eq(repository),
							held::ecosystem.eq(*ecosystem),
							held::name.eq(*name),
							held::version.eq(*version),
						)
					})
					.collect();
				diesel::insert_into(held::table)
					.values(&rows)
					.execute(conn)
					.await?;
			}
			let path_rows: Vec<_> = merged
				.iter()
				.flat_map(|((ecosystem, name, version), by_path)| {
					by_path.iter().map(
						move |((workspace_package, scope, optional), (direct, chain))| {
							(
								paths::repository.eq(repository),
								paths::ecosystem.eq(*ecosystem),
								paths::name.eq(*name),
								paths::version.eq(*version),
								paths::workspace_package.eq(*workspace_package),
								paths::scope.eq(scope.as_str()),
								paths::optional.eq(*optional),
								paths::direct.eq(*direct),
								paths::chain.eq(*chain),
							)
						},
					)
				})
				.collect();
			for chunk in path_rows.chunks(1000) {
				diesel::insert_into(paths::table)
					.values(chunk)
					.execute(conn)
					.await?;
			}
			diesel::sql_query(
				"INSERT INTO repository_reads (repository, commit_sha, read_at, last_attempt_at, failure) \
				 VALUES ($1, $2, now(), now(), NULL) \
				 ON CONFLICT (repository) DO UPDATE SET commit_sha = EXCLUDED.commit_sha, \
				 read_at = now(), last_attempt_at = now(), failure = NULL",
			)
			.bind::<sql_types::Text, _>(repository)
			.bind::<sql_types::Text, _>(commit)
			.execute(conn)
			.await?;
			Ok(())
		})
		.await
	}

	/// The default branch's head was checked and has not moved.
	pub async fn unchanged(conn: &mut AsyncPgConnection, repository: &str) -> Result<()> {
		diesel::sql_query(
			"UPDATE repository_reads SET read_at = now(), last_attempt_at = now(), failure = NULL \
			 WHERE repository = $1 AND commit_sha IS NOT NULL",
		)
		.bind::<sql_types::Text, _>(repository)
		.execute(conn)
		.await?;
		Ok(())
	}

	/// An attempt to read a repository failed. What is held stays as it was.
	pub async fn note_failure(
		conn: &mut AsyncPgConnection,
		repository: &str,
		failure: &str,
	) -> Result<()> {
		diesel::sql_query(
			"INSERT INTO repository_reads (repository, last_attempt_at, failure) \
			 VALUES ($1, now(), $2) \
			 ON CONFLICT (repository) DO UPDATE SET last_attempt_at = now(), failure = EXCLUDED.failure",
		)
		.bind::<sql_types::Text, _>(repository)
		.bind::<sql_types::Text, _>(failure)
		.execute(conn)
		.await?;
		Ok(())
	}

	/// Drop what is held for repositories no longer tracked.
	pub async fn forget_others(conn: &mut AsyncPgConnection, tracked: &[&str]) -> Result<()> {
		diesel::sql_query("DELETE FROM repository_packages WHERE repository <> ALL($1)")
			.bind::<sql_types::Array<sql_types::Text>, _>(tracked)
			.execute(conn)
			.await?;
		diesel::sql_query("DELETE FROM repository_reads WHERE repository <> ALL($1)")
			.bind::<sql_types::Array<sql_types::Text>, _>(tracked)
			.execute(conn)
			.await?;
		Ok(())
	}

	/// The tracked repositories not read for `after`, with why the last attempt
	/// failed, where it did. A repository nothing has tried to read yet is not
	/// stale.
	pub async fn stale(
		conn: &mut AsyncPgConnection,
		after: jiff::SignedDuration,
	) -> Result<Vec<Self>> {
		diesel::sql_query(
			"SELECT repository, commit_sha, read_at, last_attempt_at, failure \
			 FROM repository_reads \
			 WHERE read_at < now() - make_interval(secs => $1) \
			 OR (read_at IS NULL AND failure IS NOT NULL) \
			 ORDER BY repository",
		)
		.bind::<sql_types::Double, _>(after.as_secs_f64())
		.load(conn)
		.await
		.map_err(AppError::from)
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

/// More packages than any real system carries.
pub const MAX_PACKAGES: usize = 20_000;

const MAX_FIELD: usize = 256;

fn well_formed(value: &str, allowed: &[char]) -> bool {
	!value.is_empty()
		&& value.len() <= MAX_FIELD
		&& value
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || allowed.contains(&c))
}

impl PackageReport {
	/// Refuse a report no real system would send: an empty or malformed field,
	/// or more packages than [`MAX_PACKAGES`]. Names and versions are held to
	/// the characters dpkg allows in them.
	pub fn validate(&self) -> Result<()> {
		if !well_formed(&self.distribution, &['.', '-', '_']) {
			return Err(AppError::BadRequest("malformed distribution".into()));
		}
		if !well_formed(&self.release, &['.', '-', '_']) {
			return Err(AppError::BadRequest("malformed release".into()));
		}
		if self.packages.len() > MAX_PACKAGES {
			return Err(AppError::BadRequest(format!(
				"more than {MAX_PACKAGES} packages"
			)));
		}
		for package in &self.packages {
			if !well_formed(&package.name, &['+', '.', '-', '_']) {
				return Err(AppError::BadRequest(format!(
					"malformed package name {:?}",
					package.name
				)));
			}
			if !well_formed(&package.version, &['.', '+', '~', ':', '-', '_']) {
				return Err(AppError::BadRequest(format!(
					"malformed version {:?} of {}",
					package.version, package.name
				)));
			}
		}
		Ok(())
	}

	/// The OSV ecosystem the report is placed in, or `None` for a distribution
	/// or release Canopy cannot place.
	///
	/// Ubuntu releases are named with their long-term support marking where
	/// they carry one: the April release of an even year.
	pub fn ecosystem(&self) -> Option<String> {
		let release = self.release.trim();
		match self.distribution.trim().to_ascii_lowercase().as_str() {
			"ubuntu" => {
				// A point release (24.04.1) is placed in its release.
				let mut fields = release.split('.');
				let (year, month) = (fields.next()?, fields.next()?);
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
	/// The machine that reported.
	#[diesel(sql_type = sql_types::Uuid)]
	pub machine_id: Uuid,
	/// The distribution it runs, as it reported it.
	#[diesel(sql_type = sql_types::Text)]
	pub distribution: String,
	/// The distribution's release, as it reported it.
	#[diesel(sql_type = sql_types::Text)]
	pub release: String,
	/// The OSV ecosystem its packages are checked in, `None` when its
	/// distribution and release could not be placed in one.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub ecosystem: Option<String>,
	/// How many distinct packages it reported.
	#[diesel(sql_type = sql_types::Integer)]
	pub package_count: i32,
	/// When the package list last changed.
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

		// The report row, which holds the digest, and the list it digests change
		// together, so an identical report is never taken for a list that failed
		// to land.
		conn.transaction::<_, AppError, _>(async |conn| {
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
			Ok(())
		})
		.await?;

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
		assert_eq!(
			report("ubuntu", "24.04.1").ecosystem().as_deref(),
			Some("Ubuntu:24.04:LTS")
		);
	}

	#[test]
	fn a_report_of_real_packages_is_well_formed() {
		let mut ok = report("ubuntu", "24.04");
		ok.packages = vec![
			ReportedPackage {
				name: "openssl".into(),
				version: "3.0.13-0ubuntu3.4".into(),
			},
			ReportedPackage {
				name: "libstdc++".into(),
				version: "1:2.40~rc1+dfsg-1".into(),
			},
		];
		assert!(ok.validate().is_ok());
	}

	#[test]
	fn a_malformed_report_is_refused() {
		let package = |name: &str, version: &str| {
			let mut bad = report("ubuntu", "24.04");
			bad.packages = vec![ReportedPackage {
				name: name.into(),
				version: version.into(),
			}];
			bad
		};
		assert!(package("", "1.0").validate().is_err());
		assert!(package("openssl", "").validate().is_err());
		assert!(
			package("openssl\nIgnore the advisory", "1.0")
				.validate()
				.is_err()
		);
		assert!(package("openssl", "1.0 note").validate().is_err());
		assert!(package(&"a".repeat(300), "1.0").validate().is_err());
		assert!(report("ubuntu\n", "24.04").validate().is_err());

		let mut many = report("ubuntu", "24.04");
		many.packages = vec![
			ReportedPackage {
				name: "a".into(),
				version: "1".into(),
			};
			MAX_PACKAGES + 1
		];
		assert!(many.validate().is_err());
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
