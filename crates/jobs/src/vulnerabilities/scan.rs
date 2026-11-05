//! The scan: read the lockfile of each Tamanu version that counts, ask OSV
//! which advisories affect every package version that counts, keep what it
//! says, and reopen what has stopped standing.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Matching".

use std::{
	collections::{HashMap, HashSet},
	hash::{DefaultHasher, Hash, Hasher},
	time::Duration,
};

use commons_errors::{AppError, Result};
use database::{
	audit::{self, Action, Actor, Record, RecordKind},
	osv::Counted,
	packages::VersionPackage,
	vulnerabilities::{
		Advisory, CountedFleet, Exposure, Reopened, prune_matches, refresh_counts, replace_matches,
	},
};
use diesel_async::{AsyncConnection, AsyncPgConnection};
use jiff::{Timestamp, tz::TimeZone};
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, info, warn};

use super::{
	lockfile,
	osv_client::{OsvClient, VulnRef},
};

/// The actor of what the scan does by itself.
pub const JOB: &str = "vulnerability-scan";

/// How long before everything is asked again, since advisories are published
/// and revised after a version is first scanned.
const RESCAN_EVERY: Duration = Duration::from_secs(6 * 3600);

/// How often to look for something that makes a scan due sooner: a version or
/// a machine's package list that has come to count.
const WATCH_EVERY: Duration = Duration::from_secs(60);

/// What a pass did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
	pub lockfiles_read: usize,
	pub package_versions_queried: usize,
	pub advisories_fetched: usize,
	pub reopened: usize,
}

/// Where each Tamanu version's lockfile is read from.
#[derive(Clone)]
pub struct Lockfiles {
	http: reqwest::Client,
	base: String,
}

impl Lockfiles {
	pub fn new(base: impl Into<String>) -> Self {
		Self {
			http: reqwest::Client::builder()
				.timeout(Duration::from_secs(120))
				.build()
				.unwrap_or_default(),
			base: base.into(),
		}
	}

	async fn fetch(&self, version: &str) -> Result<Vec<VersionPackage>> {
		let url = lockfile::url(&self.base, version);
		let body = self
			.http
			.get(&url)
			.send()
			.await
			.and_then(|r| r.error_for_status())
			.map_err(|e| AppError::Upstream(format!("{url}: {e}")))?
			.text()
			.await
			.map_err(|e| AppError::Upstream(format!("{url}: {e}")))?;
		lockfile::parse(&body)
	}
}

#[derive(Clone)]
pub struct Scanner {
	pub osv: OsvClient,
	pub lockfiles: Lockfiles,
}

impl Scanner {
	pub fn new(osv: OsvClient, lockfiles: Lockfiles) -> Self {
		Self { osv, lockfiles }
	}

	/// One pass over everything that counts.
	pub async fn scan(&self, conn: &mut AsyncPgConnection) -> Result<Report> {
		let mut report = Report::default();
		let fleet = CountedFleet::load(conn).await?;

		for version in fleet.versions_unread(conn).await? {
			match self.lockfiles.fetch(&version.version).await {
				Ok(packages) => {
					VersionPackage::store(conn, version.version_id, &packages).await?;
					report.lockfiles_read += 1;
				}
				Err(e) => warn!(
					"vulnerability scan: reading the lockfile of {}: {e}",
					version.version
				),
			}
		}

		let counted = fleet.packages(conn).await?;
		report.package_versions_queried = counted.len();
		let answers = self.osv.query(&counted).await?;

		let mut revisions: HashMap<&str, Timestamp> = HashMap::new();
		for VulnRef { id, modified } in answers.iter().flatten() {
			let modified = modified
				.parse::<Timestamp>()
				.unwrap_or(Timestamp::UNIX_EPOCH);
			let latest = revisions.entry(id).or_insert(modified);
			*latest = (*latest).max(modified);
		}
		let ids: Vec<String> = revisions.keys().map(|id| (*id).to_owned()).collect();
		let held = Advisory::modified_ats(conn, &ids).await?;
		let mut fetched = Vec::new();
		let mut unavailable: HashSet<&str> = HashSet::new();
		for (id, modified) in &revisions {
			let newer = held.get(*id).is_none_or(|held| modified > held);
			if !newer {
				continue;
			}
			match self.osv.get(id).await {
				Ok(record) => fetched.push(record),
				Err(e) => {
					warn!("vulnerability scan: fetching {id}: {e}");
					if !held.contains_key(*id) {
						unavailable.insert(id);
					}
				}
			}
		}
		report.advisories_fetched = fetched.len();

		let today = Timestamp::now().to_zoned(TimeZone::UTC).date();
		let reopened = conn
			.transaction::<_, AppError, _>(async |conn| {
				for record in &fetched {
					Advisory::upsert(conn, record).await?;
				}
				for (query, found) in counted.iter().zip(&answers) {
					let found: Vec<String> = found
						.iter()
						.map(|v| v.id.clone())
						.filter(|id| !unavailable.contains(id.as_str()))
						.collect::<HashSet<_>>()
						.into_iter()
						.collect();
					replace_matches(conn, query, &found).await?;
				}
				prune_matches(conn, &counted).await?;

				let exposure = Exposure::load(conn, &fleet, None).await?;
				refresh_counts(conn, &exposure).await?;

				let reopened = Reopened::pass(conn, &counted, today).await?;
				for done in &reopened {
					audit::record(
						conn,
						&Actor::canopy(JOB),
						&Action::VulnerabilityReopened {
							advisory: Record::new(
								RecordKind::Advisory,
								&done.vulnerability_id,
								&done.vulnerability_id,
							),
							source: done.source.as_str().to_owned(),
							why: done.why.clone(),
						},
					)
					.await?;
				}
				Ok(reopened)
			})
			.await?;
		report.reopened = reopened.len();
		Ok(report)
	}

	/// What the counted package versions are, to tell when a pass is due sooner
	/// than the schedule says.
	pub async fn fingerprint(&self, conn: &mut AsyncPgConnection) -> Result<u64> {
		let fleet = CountedFleet::load(conn).await?;
		let mut hasher = DefaultHasher::new();
		for version in &fleet.versions {
			version.version_id.hash(&mut hasher);
		}
		fleet.machines.hash(&mut hasher);
		let packages: Vec<Counted> = fleet.packages(conn).await?;
		packages.hash(&mut hasher);
		Ok(hasher.finish())
	}
}

/// Scan at once on starting, then on the schedule, and early when something
/// comes to count.
pub fn spawn() -> JoinHandle<()> {
	let pool = database::init();
	let scanner = Scanner::new(
		OsvClient::new(super::osv_client::BASE_URL),
		Lockfiles::new(lockfile::BASE_URL),
	);
	task::spawn(async move {
		let mut last_scan: Option<tokio::time::Instant> = None;
		let mut last_seen: Option<u64> = None;
		loop {
			let Ok(mut conn) = pool.get().await else {
				error!("vulnerability scan: failed to get database connection");
				sleep(WATCH_EVERY).await;
				continue;
			};
			let seen = scanner.fingerprint(&mut conn).await.ok();
			let due = match last_scan {
				None => true,
				Some(at) => at.elapsed() >= RESCAN_EVERY || (seen.is_some() && seen != last_seen),
			};
			if due {
				match scanner.scan(&mut conn).await {
					Ok(report) => {
						info!(
							"vulnerability scan: {} lockfile(s) read, {} package version(s) queried, \
							 {} advisory record(s) fetched, {} reopened",
							report.lockfiles_read,
							report.package_versions_queried,
							report.advisories_fetched,
							report.reopened
						);
						last_seen = scanner.fingerprint(&mut conn).await.ok();
					}
					Err(e) => error!("vulnerability scan failed: {e}"),
				}
				last_scan = Some(tokio::time::Instant::now());
			} else {
				debug!("vulnerability scan: nothing new to scan");
			}
			drop(conn);
			sleep(WATCH_EVERY).await;
		}
	})
}
