//! The scan: keep each tracked repository's dependencies as its default branch
//! has them, ask OSV which advisories affect every package version that
//! counts, keep what it says, and let lapse what has stopped standing.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`), "Matching"
//! and "Exposure and closing".
//!
//! Two things run on their own timers. Repositories are watched every few
//! minutes, through their heads, which costs none of GitHub's API quota, and
//! read again only when one has moved (see [`super::repository`]). The scan
//! asks OSV, on a schedule and early when what counts has changed: a stored
//! read of a repository, or a machine's package list.

use std::{
	collections::{HashMap, HashSet},
	hash::{DefaultHasher, Hash, Hasher},
	time::Duration,
};

use commons_errors::{AppError, Result};
use database::{
	audit::{self, Actor},
	osv::{self, Counted},
	packages::RepositoryRead,
	self_alerts,
	vulnerabilities::{
		Advisory, Decision, Lapse, REPOSITORIES, Repository, Via, change_signal, counted_packages,
		prune_matches, refresh_items, replace_matches, suggest_from_decision,
	},
};
use diesel_async::{AsyncConnection, AsyncPgConnection};
use futures::{StreamExt as _, stream};
use jiff::{SignedDuration, Timestamp};
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, info, warn};

use super::{
	osv_client::{OsvClient, VulnRef},
	repository::{Endpoints, GitHub},
};

/// The actor of what the scan does by itself.
pub const JOB: &str = "vulnerability-scan";

/// The actor of withdrawing the decisions the model made before it only
/// suggested.
pub const MIGRATION_JOB: &str = "vulnerability-migration";

/// How long before everything is asked again, since advisories are published
/// and revised after a version is first scanned.
const RESCAN_EVERY: Duration = Duration::from_secs(6 * 3600);

/// How often to look for something that makes a scan due sooner: a repository
/// read, or a machine's package list, that has come to count.
const WATCH_EVERY: Duration = Duration::from_secs(60);

/// How often each repository's head is checked.
const REPOSITORIES_EVERY: Duration = Duration::from_secs(5 * 60);

/// How long after a failed attempt to read a repository to try it again.
const REPOSITORY_RETRY_AFTER: SignedDuration = SignedDuration::from_mins(15);

/// How many advisory records are fetched from OSV at once.
const FETCH_CONCURRENCY: usize = 8;

/// How long after a failed pass to try again.
const RETRY_AFTER: Duration = Duration::from_secs(10 * 60);

/// What a decision the model made is withdrawn with.
const WITHDRAWN_REASON: &str = "The model now only suggests; a person decides.";

/// The model a suggestion made from one of those decisions is credited to.
const MIGRATED_MODEL: &str = "earlier model decision";

/// What a pass over the repositories did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RepositoryReport {
	/// Repositories whose default branch had moved and was read again.
	pub read: usize,
	/// Repositories whose head had not moved.
	pub unchanged: usize,
	/// Repositories that could not be read.
	pub failed: usize,
	/// Repositories left alone for now, after a failed attempt.
	pub waiting: usize,
}

/// What a scan did.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Report {
	pub package_versions_queried: usize,
	pub advisories_fetched: usize,
	pub lapsed: usize,
	/// Matches OSV reported that the advisory's own ranges ruled out.
	pub matches_withheld: usize,
}

#[derive(Clone)]
pub struct Scanner {
	pub osv: OsvClient,
	pub github: GitHub,
	/// The repositories whose default branches are read.
	pub repositories: Vec<Repository>,
}

impl Scanner {
	pub fn new(osv: OsvClient, github: GitHub) -> Self {
		Self {
			osv,
			github,
			repositories: REPOSITORIES.to_vec(),
		}
	}

	pub fn with_repositories(mut self, repositories: Vec<Repository>) -> Self {
		self.repositories = repositories;
		self
	}

	/// Look at the head of each tracked repository, and read again those that
	/// have moved. One that cannot be read keeps what is held, and is tried again
	/// after a while; the self-alert says when that has lasted.
	pub async fn read_repositories(
		&self,
		conn: &mut AsyncPgConnection,
	) -> Result<RepositoryReport> {
		let mut report = RepositoryReport::default();
		let tracked: Vec<String> = self
			.repositories
			.iter()
			.map(|r| r.name.to_owned())
			.collect();
		RepositoryRead::forget_others(conn, tracked).await?;

		for repository in self.repositories.clone() {
			let held = RepositoryRead::get(conn, repository.name).await?;
			if let Some(held) = &held
				&& held.failure.is_some()
				&& Timestamp::now().duration_since(held.last_attempt_at) < REPOSITORY_RETRY_AFTER
			{
				report.waiting += 1;
				continue;
			}

			let head = match self.github.head(repository.slug).await {
				Ok(head) => head,
				Err(e) => {
					warn!("vulnerability scan: {}: {e}", repository.name);
					RepositoryRead::note_failure(conn, repository.name, &e.to_string()).await?;
					report.failed += 1;
					continue;
				}
			};
			if held.as_ref().and_then(|held| held.commit_sha.as_deref()) == Some(head.as_str()) {
				RepositoryRead::unchanged(conn, repository.name).await?;
				report.unchanged += 1;
				continue;
			}
			match self.github.read(&repository, &head).await {
				Ok(packages) => {
					RepositoryRead::store(conn, repository.name, &head, &packages).await?;
					report.read += 1;
				}
				Err(e) => {
					warn!("vulnerability scan: reading {}: {e}", repository.name);
					RepositoryRead::note_failure(conn, repository.name, &e.to_string()).await?;
					report.failed += 1;
				}
			}
		}

		if let Err(e) = self_alerts::sweep_vulnerability_repositories(conn).await {
			error!("vulnerability scan: the self-alert could not be updated: {e}");
		}
		Ok(report)
	}

	/// Withdraw every decision the model made and still stands, now that it
	/// only suggests, and keep each as the item's suggestion, with a basis that
	/// says nothing was checked, so a person looks at it. Each is its own
	/// transaction and is audited as Canopy's. A decision already withdrawn is
	/// not looked at again, so this does nothing once it is done.
	pub async fn withdraw_model_decisions(&self, conn: &mut AsyncPgConnection) -> Result<usize> {
		let mut withdrawn = 0;
		for decision in Decision::standing_by_model(conn).await? {
			let done = conn
				.transaction::<_, AppError, _>(async |conn| {
					let actor = Actor::canopy(MIGRATION_JOB);
					let withdrawal = Decision::withdraw(
						conn,
						&decision.vulnerability_id,
						&decision.source,
						WITHDRAWN_REASON,
						actor.clone(),
						Via::Canopy,
						Some(decision.id),
					)
					.await?;
					audit::record(conn, &actor, &withdrawal.audited()).await?;
					suggest_from_decision(conn, &decision, MIGRATED_MODEL).await?;
					Ok(())
				})
				.await;
			match done {
				Ok(()) => withdrawn += 1,
				Err(e) => warn!(
					"vulnerability scan: withdrawing the model's decision on {} ({}): {e}",
					decision.vulnerability_id,
					decision.source.as_str()
				),
			}
		}
		Ok(withdrawn)
	}

	/// One pass over everything that counts.
	pub async fn scan(&self, conn: &mut AsyncPgConnection) -> Result<Report> {
		let mut report = Report::default();
		let counted = counted_packages(conn).await?;
		report.package_versions_queried = counted.len();
		let answers = self.osv.query(&counted).await?;

		let mut revisions: HashMap<&str, Timestamp> = HashMap::new();
		// Answers whose `modified` does not read, so there is no telling
		// whether what is held for them is current.
		let mut undated: HashSet<&str> = HashSet::new();
		for VulnRef { id, modified } in answers.iter().flatten() {
			let modified = modified.parse::<Timestamp>().unwrap_or_else(|_| {
				undated.insert(id);
				Timestamp::UNIX_EPOCH
			});
			let latest = revisions.entry(id).or_insert(modified);
			*latest = (*latest).max(modified);
		}
		let ids: Vec<String> = revisions.keys().map(|id| (*id).to_owned()).collect();
		let held = Advisory::modified_ats(conn, &ids).await?;
		let mut fetched = Vec::new();
		let mut unavailable: HashSet<&str> = HashSet::new();
		// Revised advisories this pass could not fetch, so what is held for
		// them is behind what OSV answered with.
		let mut stale: HashSet<&str> = HashSet::new();
		let wanted: Vec<&str> = revisions
			.iter()
			.filter(|(id, modified)| {
				undated.contains(**id) || held.get(**id).is_none_or(|held| *modified > held)
			})
			.map(|(id, _)| *id)
			.collect();
		let osv = &self.osv;
		let fetches: Vec<_> = wanted
			.into_iter()
			.map(|id| fetch_advisory(osv, id))
			.collect();
		let mut records = stream::iter(fetches).buffer_unordered(FETCH_CONCURRENCY);
		while let Some((id, record)) = records.next().await {
			match record {
				Ok(record) => fetched.push(record),
				Err(e) => {
					warn!("vulnerability scan: fetching {id}: {e}");
					if held.contains_key(id) {
						stale.insert(id);
					} else {
						unavailable.insert(id);
					}
				}
			}
		}
		drop(records);
		report.advisories_fetched = fetched.len();

		let lapsed = conn
			.transaction::<_, AppError, _>(async |conn| {
				for record in &fetched {
					Advisory::upsert(conn, record).await?;
				}
				let found_ids: Vec<String> = answers
					.iter()
					.flatten()
					.map(|v| v.id.clone())
					.filter(|id| !unavailable.contains(id.as_str()))
					.collect::<HashSet<_>>()
					.into_iter()
					.collect();
				let ranges = Advisory::rated(conn, &found_ids).await?;
				let mut withheld = 0;
				let matched: Vec<(Counted, Vec<String>)> = counted
					.iter()
					.zip(&answers)
					.map(|(query, found)| {
						let found: HashSet<String> = found
							.iter()
							.map(|v| v.id.clone())
							.filter(|id| !unavailable.contains(id.as_str()))
							.filter(|id| {
								// Only a record as current as the answer can
								// withhold what the answer reported.
								let keep = stale.contains(id.as_str())
									|| ranges.get(id).is_none_or(|(_, affected)| {
										!osv::rules_out(
											affected,
											&query.ecosystem,
											&query.name,
											&query.version,
										)
									});
								if !keep {
									withheld += 1;
									debug!(
										"vulnerability scan: {id} is not filed for {} {} {}: \
										 its ranges rule the version out",
										query.ecosystem, query.name, query.version
									);
								}
								keep
							})
							.collect();
						(query.clone(), found.into_iter().collect())
					})
					.collect();
				replace_matches(conn, &matched).await?;
				prune_matches(conn, &counted).await?;
				refresh_items(conn).await?;

				let lapsed = Lapse::pass(conn).await?;
				for done in &lapsed {
					audit::record(conn, &Actor::canopy(JOB), &done.audited()).await?;
				}
				Ok((lapsed, withheld))
			})
			.await?;
		let (lapsed, withheld) = lapsed;
		report.lapsed = lapsed.len();
		report.matches_withheld = withheld;
		Ok(report)
	}

	/// What came to count, to tell when a pass is due sooner than the schedule
	/// says: where each repository was last read to, and which production
	/// machines have which package lists. Not the package versions themselves,
	/// which run to many thousands and would be read and hashed every look.
	pub async fn fingerprint(&self, conn: &mut AsyncPgConnection) -> Result<u64> {
		let mut hasher = DefaultHasher::new();
		change_signal(conn).await?.hash(&mut hasher);
		Ok(hasher.finish())
	}
}

async fn fetch_advisory<'a>(osv: &OsvClient, id: &'a str) -> (&'a str, Result<osv::Record>) {
	(id, osv.get(id).await)
}

async fn watch_repositories(scanner: Scanner, pool: database::Db) {
	loop {
		match pool.get().await {
			Ok(mut conn) => match scanner.read_repositories(&mut conn).await {
				Ok(report) => info!(
					"vulnerability scan: {} repositor(ies) read, {} unchanged, {} failed, {} waiting",
					report.read, report.unchanged, report.failed, report.waiting
				),
				Err(e) => error!("vulnerability scan: reading repositories failed: {e}"),
			},
			Err(_) => error!("vulnerability scan: failed to get database connection"),
		}
		sleep(REPOSITORIES_EVERY).await;
	}
}

/// Read the repositories at once on starting and then every few minutes, and
/// scan at once, then on the schedule, and early when something comes to count.
pub fn spawn() -> JoinHandle<()> {
	let pool = database::init();
	let scanner = Scanner::new(
		OsvClient::new(super::osv_client::BASE_URL),
		GitHub::new(Endpoints::default()),
	);

	task::spawn(watch_repositories(scanner.clone(), pool.clone()));

	task::spawn(async move {
		let mut next_scan: Option<tokio::time::Instant> = None;
		let mut last_seen: Option<u64> = None;
		loop {
			let Ok(mut conn) = pool.get().await else {
				error!("vulnerability scan: failed to get database connection");
				sleep(WATCH_EVERY).await;
				continue;
			};
			// Idempotent, and cheap once there is nothing left, so it is tried on
			// every look: a database that was down when the process started does
			// not leave the model's decisions standing until the next restart.
			match scanner.withdraw_model_decisions(&mut conn).await {
				Ok(0) => {}
				Ok(n) => info!("vulnerability scan: withdrew {n} decision(s) the model made"),
				Err(e) => {
					error!("vulnerability scan: withdrawing the model's decisions failed: {e}")
				}
			}
			let seen = scanner.fingerprint(&mut conn).await.ok();
			let due = match next_scan {
				None => true,
				Some(at) => {
					tokio::time::Instant::now() >= at || (seen.is_some() && seen != last_seen)
				}
			};
			if due {
				match scanner.scan(&mut conn).await {
					Ok(report) => {
						info!(
							"vulnerability scan: {} package version(s) queried, \
							 {} advisory record(s) fetched, {} lapsed, \
							 {} match(es) withheld",
							report.package_versions_queried,
							report.advisories_fetched,
							report.lapsed,
							report.matches_withheld
						);
						// What was seen before the scan, so a change that landed
						// during it still makes the next look due.
						last_seen = seen;
						next_scan = Some(tokio::time::Instant::now() + RESCAN_EVERY);
					}
					Err(e) => {
						error!("vulnerability scan failed: {e}");
						// Retried after a while, not on the next look: what made it
						// due has been seen.
						last_seen = seen;
						next_scan = Some(tokio::time::Instant::now() + RETRY_AFTER);
					}
				}
			} else {
				debug!("vulnerability scan: nothing new to scan");
			}
			drop(conn);
			sleep(WATCH_EVERY).await;
		}
	})
}
