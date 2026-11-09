//! Where the items come from: each tracked repository with the state of its
//! last read, and Linux with what the fleet has reported.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "The
//! sources page".

use std::collections::HashMap;

use commons_errors::Result;
use commons_types::server::rank::ServerRank;
use diesel::sql_types;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use super::{LINUX, REPOSITORIES, Source};
use crate::packages::RepositoryRead;

/// One source, with what Canopy holds for it and how it came to hold it.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilitySourceState)]
pub struct SourceState {
	/// The source: a tracked repository by name, or Linux.
	pub source: Source,
	/// The GitHub repository it is read from, absent for Linux.
	pub slug: Option<String>,
	/// The commit it was last read at, absent before it has been read and once
	/// a rescan has been asked for.
	pub commit_sha: Option<String>,
	/// When it was last read, or its head last found unchanged.
	pub read_at: Option<Timestamp>,
	/// When reading it was last attempted.
	pub last_attempt_at: Option<Timestamp>,
	/// Why the last attempt failed, absent when it did not.
	pub failure: Option<String>,
	/// How many package versions it carries.
	pub packages: i64,
	/// How many live production machines have reported packages, for Linux.
	pub machines: Option<i64>,
}

#[derive(diesel::QueryableByName)]
struct CountRow {
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::BigInt)]
	packages: i64,
}

/// Every source, tracked repositories in the order Canopy reads them and then
/// Linux. A repository nothing has tried to read yet is listed all the same,
/// with no read of its own.
pub async fn list(conn: &mut AsyncPgConnection) -> Result<Vec<SourceState>> {
	let reads: HashMap<String, RepositoryRead> = RepositoryRead::all(conn)
		.await?
		.into_iter()
		.map(|read| (read.repository.clone(), read))
		.collect();

	let counts: Vec<CountRow> = diesel::sql_query(
		"SELECT repository AS source, count(*) AS packages FROM repository_packages GROUP BY 1",
	)
	.load(conn)
	.await?;
	let counts: HashMap<String, i64> = counts
		.into_iter()
		.map(|row| (row.source, row.packages))
		.collect();

	let mut sources: Vec<SourceState> = REPOSITORIES
		.iter()
		.map(|repository| {
			let read = reads.get(repository.name);
			SourceState {
				source: Source::Repository(repository.name.to_owned()),
				slug: Some(repository.slug.to_owned()),
				commit_sha: read.and_then(|read| read.commit_sha.clone()),
				read_at: read.and_then(|read| read.read_at),
				last_attempt_at: read.map(|read| read.last_attempt_at),
				failure: read.and_then(|read| read.failure.clone()),
				packages: counts.get(repository.name).copied().unwrap_or_default(),
				machines: None,
			}
		})
		.collect();

	#[derive(diesel::QueryableByName)]
	struct LinuxRow {
		#[diesel(sql_type = sql_types::BigInt)]
		packages: i64,
		#[diesel(sql_type = sql_types::BigInt)]
		machines: i64,
	}
	let linux: LinuxRow = diesel::sql_query(
		"SELECT count(DISTINCT (p.name, p.version)) AS packages, \
		        count(DISTINCT p.machine_id) AS machines \
		 FROM machine_packages p \
		 JOIN machine_package_reports r USING (machine_id) \
		 JOIN machines m ON m.id = p.machine_id \
		 WHERE m.deleted_at IS NULL AND m.rank = $1 AND r.ecosystem IS NOT NULL",
	)
	.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
	.get_result(conn)
	.await?;
	sources.push(SourceState {
		source: Source::Linux,
		slug: None,
		commit_sha: None,
		read_at: None,
		last_attempt_at: None,
		failure: None,
		packages: linux.packages,
		machines: Some(linux.machines),
	});

	Ok(sources)
}

/// Ask for a repository to be read again, whatever its default branch is at. A
/// repository nothing has read yet is already due.
// spec: VUL#where-a-package-sits
pub async fn rescan(conn: &mut AsyncPgConnection, source: &Source) -> Result<()> {
	let Source::Repository(repository) = source else {
		return Err(commons_errors::AppError::BadRequest(format!(
			"{LINUX} is reported by machines, not read from a repository"
		)));
	};
	RepositoryRead::mark_due(conn, repository).await
}
