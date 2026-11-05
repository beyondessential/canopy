//! Vulnerabilities in what the production fleet runs, and the triage decisions
//! taken on them.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`) and
//! `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).
//!
//! An item is one OSV advisory within one source, Tamanu or Linux. What the
//! scan stores is the advisories and which package versions each matches;
//! exposure, which production applications and machines that reaches, is a join
//! worked out when read, so ranking or archiving something changes it without a
//! rescan.

use std::collections::{BTreeMap, HashMap, HashSet};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::{prelude::*, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{Timestamp, civil::Date};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	audit::{self, Action, Actor, RecordKind},
	osv::{self, Affected, Counted, Held, Record, Reopening, Severity},
	packages::NPM,
};

/// Where an item's exposure comes from.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilitySource)]
pub enum Source {
	/// The dependencies of Tamanu versions.
	Tamanu,
	/// The packages installed on machines.
	Linux,
}

impl Source {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Tamanu => "tamanu",
			Self::Linux => "linux",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"tamanu" => Some(Self::Tamanu),
			"linux" => Some(Self::Linux),
			_ => None,
		}
	}

	/// The source a matched package version belongs to, by its ecosystem.
	pub fn of_ecosystem(ecosystem: &str) -> Self {
		if ecosystem == NPM {
			Self::Tamanu
		} else {
			Self::Linux
		}
	}
}

/// What a decision says about an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityOutcome)]
pub enum Outcome {
	/// The fleet cannot be hit by the flaw.
	NotAffected,
	/// The fleet is exposed and lives with it, until the review date.
	AcceptedRisk,
	/// A fix is planned.
	WillFix,
}

impl Outcome {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::NotAffected => "not_affected",
			Self::AcceptedRisk => "accepted_risk",
			Self::WillFix => "will_fix",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"not_affected" => Some(Self::NotAffected),
			"accepted_risk" => Some(Self::AcceptedRisk),
			"will_fix" => Some(Self::WillFix),
			_ => None,
		}
	}
}

/// An item's triage status.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityStatus)]
pub enum Status {
	Open,
	NotAffected,
	AcceptedRisk,
	WillFix,
}

impl Status {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Open => "open",
			Self::NotAffected => "not_affected",
			Self::AcceptedRisk => "accepted_risk",
			Self::WillFix => "will_fix",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"open" => Some(Self::Open),
			other => Outcome::parse(other).map(Self::from),
		}
	}
}

impl From<Outcome> for Status {
	fn from(outcome: Outcome) -> Self {
		match outcome {
			Outcome::NotAffected => Self::NotAffected,
			Outcome::AcceptedRisk => Self::AcceptedRisk,
			Outcome::WillFix => Self::WillFix,
		}
	}
}

/// How a decision was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityVia)]
pub enum Via {
	/// An operator, in the operator interface.
	Ui,
	/// The model.
	Model,
	/// A caller of the fleet query interface.
	Mcp,
}

impl Via {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Ui => "ui",
			Self::Model => "model",
			Self::Mcp => "mcp",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"ui" => Some(Self::Ui),
			"model" => Some(Self::Model),
			"mcp" => Some(Self::Mcp),
			_ => None,
		}
	}
}

/// The grounds on which the model may find an item not affected on its own.
/// Canopy holds these fixed.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityGround)]
pub enum Ground {
	/// The package is reached only from Tamanu's web frontend, and the flaw
	/// needs server-side execution.
	FrontendOnly,
	/// The package runs only at build time, and the flaw needs it to run in
	/// production.
	BuildTimeOnly,
	/// The advisory affects only an operating system or architecture the fleet
	/// does not run.
	UnaffectedPlatform,
}

impl Ground {
	pub const ALL: [Self; 3] = [
		Self::FrontendOnly,
		Self::BuildTimeOnly,
		Self::UnaffectedPlatform,
	];

	pub fn as_str(self) -> &'static str {
		match self {
			Self::FrontendOnly => "frontend_only",
			Self::BuildTimeOnly => "build_time_only",
			Self::UnaffectedPlatform => "unaffected_platform",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		Self::ALL.into_iter().find(|ground| ground.as_str() == s)
	}
}

/// Today's date in UTC, which is what review dates are held to.
pub fn today() -> Date {
	Timestamp::now().to_zoned(jiff::tz::TimeZone::UTC).date()
}

/// An OSV timestamp, or `None` for one that does not parse: a malformed time
/// on one record does not hold up the rest.
fn parse_timestamp(s: &str) -> Option<Timestamp> {
	s.parse().ok()
}

fn strings(array: Vec<Option<String>>) -> Vec<String> {
	array.into_iter().flatten().collect()
}

// ── Advisories ──────────────────────────────────────────────────────────────

/// A cached OSV advisory.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityAdvisory)]
pub struct Advisory {
	/// The advisory's OSV identifier.
	pub id: String,
	/// Other identifiers for the same flaw, such as its CVE.
	pub aliases: Vec<String>,
	/// A one-line summary.
	pub summary: String,
	/// The advisory's full description, in Markdown.
	pub details: String,
	/// The severity Canopy ranks it by: its CVSS score where it has one,
	/// otherwise its database's rating.
	pub severity: Severity,
	/// The packages it affects, with their affected versions and ranges.
	pub affected: Vec<Affected>,
	/// Links it gives: articles, reports, fixes.
	pub references: Vec<osv::Reference>,
	/// When it was published, where OSV says.
	pub published_at: Option<Timestamp>,
	/// When OSV last revised it.
	pub modified_at: Timestamp,
}

#[derive(QueryableByName)]
struct AdvisoryRow {
	#[diesel(sql_type = sql_types::Text)]
	id: String,
	#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
	aliases: Vec<Option<String>>,
	#[diesel(sql_type = sql_types::Text)]
	summary: String,
	#[diesel(sql_type = sql_types::Text)]
	details: String,
	#[diesel(sql_type = sql_types::Text)]
	severity: String,
	#[diesel(sql_type = sql_types::Jsonb)]
	affected: serde_json::Value,
	#[diesel(sql_type = sql_types::Jsonb)]
	references: serde_json::Value,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>, deserialize_as = jiff_diesel::NullableTimestamp)]
	published_at: Option<Timestamp>,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	modified_at: Timestamp,
}

impl From<AdvisoryRow> for Advisory {
	fn from(row: AdvisoryRow) -> Self {
		Self {
			id: row.id,
			aliases: strings(row.aliases),
			summary: row.summary,
			details: row.details,
			severity: row.severity.parse().unwrap_or_default(),
			affected: serde_json::from_value(row.affected).unwrap_or_default(),
			references: serde_json::from_value(row.references).unwrap_or_default(),
			published_at: row.published_at,
			modified_at: row.modified_at,
		}
	}
}

const ADVISORY_COLUMNS: &str =
	"id, aliases, summary, details, severity, affected, \"references\", published_at, modified_at";

impl Advisory {
	/// Store an OSV record, replacing what is held for its identifier.
	pub async fn upsert(conn: &mut AsyncPgConnection, record: &Record) -> Result<()> {
		let published = record
			.published
			.as_deref()
			.and_then(parse_timestamp)
			.map(jiff_diesel::Timestamp::from);
		// As the scan reads it when deciding what to fetch.
		let modified = parse_timestamp(&record.modified).unwrap_or(Timestamp::UNIX_EPOCH);
		diesel::sql_query(
			"INSERT INTO vulnerabilities \
			 (id, aliases, summary, details, severity, affected, \"references\", published_at, modified_at, fetched_at) \
			 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, now()) \
			 ON CONFLICT (id) DO UPDATE SET aliases = EXCLUDED.aliases, summary = EXCLUDED.summary, \
			 details = EXCLUDED.details, severity = EXCLUDED.severity, affected = EXCLUDED.affected, \
			 \"references\" = EXCLUDED.\"references\", published_at = EXCLUDED.published_at, \
			 modified_at = EXCLUDED.modified_at, fetched_at = now()",
		)
		.bind::<sql_types::Text, _>(&record.id)
		.bind::<sql_types::Array<sql_types::Text>, _>(&record.aliases)
		.bind::<sql_types::Text, _>(&record.summary)
		.bind::<sql_types::Text, _>(&record.details)
		.bind::<sql_types::Text, _>(record.severity().as_str())
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(&record.affected).unwrap_or_default())
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(&record.references).unwrap_or_default())
		.bind::<sql_types::Nullable<sql_types::Timestamptz>, _>(published)
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(modified))
		.execute(conn)
		.await?;
		Ok(())
	}

	/// When each of `ids` was last revised, for those held. An advisory is
	/// fetched again only when OSV reports a later time than this.
	pub async fn modified_ats(
		conn: &mut AsyncPgConnection,
		ids: &[String],
	) -> Result<HashMap<String, Timestamp>> {
		#[derive(QueryableByName)]
		struct Row {
			#[diesel(sql_type = sql_types::Text)]
			id: String,
			#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
			modified_at: Timestamp,
		}
		let rows: Vec<Row> =
			diesel::sql_query("SELECT id, modified_at FROM vulnerabilities WHERE id = ANY($1)")
				.bind::<sql_types::Array<sql_types::Text>, _>(ids)
				.load(conn)
				.await?;
		Ok(rows
			.into_iter()
			.map(|row| (row.id, row.modified_at))
			.collect())
	}

	pub async fn get(conn: &mut AsyncPgConnection, id: &str) -> Result<Option<Self>> {
		let row: Option<AdvisoryRow> = diesel::sql_query(format!(
			"SELECT {ADVISORY_COLUMNS} FROM vulnerabilities WHERE id = $1"
		))
		.bind::<sql_types::Text, _>(id)
		.get_result(conn)
		.await
		.optional()?;
		Ok(row.map(Self::from))
	}

	pub async fn get_many(conn: &mut AsyncPgConnection, ids: &[String]) -> Result<Vec<Self>> {
		let rows: Vec<AdvisoryRow> = diesel::sql_query(format!(
			"SELECT {ADVISORY_COLUMNS} FROM vulnerabilities WHERE id = ANY($1)"
		))
		.bind::<sql_types::Array<sql_types::Text>, _>(ids)
		.load(conn)
		.await?;
		Ok(rows.into_iter().map(Self::from).collect())
	}
}

// ── What counts ─────────────────────────────────────────────────────────────

/// A Tamanu version that counts: one a production application runs, or the
/// newest published.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CountedVersion {
	pub version_id: Uuid,
	/// `major.minor.patch`.
	pub version: String,
	/// The live production applications running it.
	pub applications: Vec<Uuid>,
	pub newest_published: bool,
}

/// What of the fleet counts as exposure, as it stands now.
#[derive(Clone, Debug, Default)]
pub struct CountedFleet {
	pub versions: Vec<CountedVersion>,
	/// Live production machines that have reported packages.
	pub machines: Vec<Uuid>,
}

#[derive(QueryableByName)]
struct TripleRow {
	#[diesel(sql_type = sql_types::Text)]
	ecosystem: String,
	#[diesel(sql_type = sql_types::Text)]
	name: String,
	#[diesel(sql_type = sql_types::Text)]
	version: String,
}

impl CountedFleet {
	/// Which Tamanu versions and machines count: a version when a live
	/// production application runs it, or when it is the newest published; a
	/// machine when it is live and production.
	pub async fn load(conn: &mut AsyncPgConnection) -> Result<Self> {
		use crate::schema::{applications, versions};
		use commons_types::server::app_type::ApplicationType;

		let application_ids: Vec<Uuid> = applications::table
			.select(applications::id)
			.filter(applications::rank.eq(ServerRank::Production))
			.filter(applications::deleted_at.is_null())
			.filter(
				applications::type_.eq_any(ApplicationType::stored_values_where(
					ApplicationType::tracks_versions,
				)),
			)
			.load(conn)
			.await?;
		let running =
			crate::reported_detail::ReportedDetail::last_versions(conn, &application_ids).await?;

		let mut by_triple: BTreeMap<(i32, i32, i32), Vec<Uuid>> = BTreeMap::new();
		for (application, version) in running {
			by_triple
				.entry((
					version.0.major as i32,
					version.0.minor as i32,
					version.0.patch as i32,
				))
				.or_default()
				.push(application);
		}

		let newest = crate::versions::Version::newest_published(conn).await?;
		let known: Vec<(Uuid, i32, i32, i32)> = versions::table
			.select((
				versions::id,
				versions::major,
				versions::minor,
				versions::patch,
			))
			.load(conn)
			.await?;

		let mut counted = Vec::new();
		for (version_id, major, minor, patch) in known {
			let applications = by_triple.remove(&(major, minor, patch)).unwrap_or_default();
			let newest_published = newest
				.as_ref()
				.is_some_and(|newest| newest.id == version_id);
			if applications.is_empty() && !newest_published {
				continue;
			}
			counted.push(CountedVersion {
				version_id,
				version: format!("{major}.{minor}.{patch}"),
				applications,
				newest_published,
			});
		}

		#[derive(QueryableByName)]
		struct MachineRow {
			#[diesel(sql_type = sql_types::Uuid)]
			id: Uuid,
		}
		let machines: Vec<MachineRow> = diesel::sql_query(
			"SELECT m.id FROM machines m \
			 JOIN machine_package_reports r ON r.machine_id = m.id \
			 WHERE m.deleted_at IS NULL AND m.rank = $1",
		)
		.bind::<sql_types::Text, _>(ServerRank::Production.to_string())
		.load(conn)
		.await?;

		Ok(Self {
			versions: counted,
			machines: machines.into_iter().map(|row| row.id).collect(),
		})
	}

	/// The distinct package versions to ask OSV about, however many versions or
	/// machines carry each.
	pub async fn packages(&self, conn: &mut AsyncPgConnection) -> Result<Vec<Counted>> {
		let version_ids: Vec<Uuid> = self.versions.iter().map(|v| v.version_id).collect();
		let tamanu: Vec<TripleRow> = diesel::sql_query(
			"SELECT DISTINCT 'npm'::text AS ecosystem, name, version \
			 FROM version_packages WHERE version_id = ANY($1)",
		)
		.bind::<sql_types::Array<sql_types::Uuid>, _>(&version_ids)
		.load(conn)
		.await?;
		let linux: Vec<TripleRow> = diesel::sql_query(
			"SELECT DISTINCT r.ecosystem, p.name, p.version \
			 FROM machine_packages p JOIN machine_package_reports r USING (machine_id) \
			 WHERE p.machine_id = ANY($1) AND r.ecosystem IS NOT NULL",
		)
		.bind::<sql_types::Array<sql_types::Uuid>, _>(&self.machines)
		.load(conn)
		.await?;
		let mut out: Vec<Counted> = tamanu
			.into_iter()
			.chain(linux)
			.map(|row| Counted {
				ecosystem: row.ecosystem,
				name: row.name,
				version: row.version,
			})
			.collect();
		out.sort();
		out.dedup();
		Ok(out)
	}

	/// The Tamanu versions with no package list read yet: ones that count but
	/// whose lockfile has not been read.
	pub async fn versions_unread(
		&self,
		conn: &mut AsyncPgConnection,
	) -> Result<Vec<CountedVersion>> {
		let mut unread = Vec::new();
		for version in &self.versions {
			if !crate::packages::VersionPackage::lockfile_read(conn, version.version_id).await? {
				unread.push(version.clone());
			}
		}
		Ok(unread)
	}
}

// ── Matches ─────────────────────────────────────────────────────────────────

/// Replace what advisories match each queried package version with what the
/// last OSV query for it found. The advisories found must already be stored.
pub async fn replace_matches(
	conn: &mut AsyncPgConnection,
	answers: &[(Counted, Vec<String>)],
) -> Result<()> {
	let (mut ecosystems, mut names, mut versions) = (Vec::new(), Vec::new(), Vec::new());
	let (mut ids, mut sources) = (Vec::new(), Vec::new());
	let (mut found_ecosystems, mut found_names, mut found_versions) =
		(Vec::new(), Vec::new(), Vec::new());
	for (queried, found) in answers {
		ecosystems.push(queried.ecosystem.as_str());
		names.push(queried.name.as_str());
		versions.push(queried.version.as_str());
		let source = Source::of_ecosystem(&queried.ecosystem);
		for id in found {
			ids.push(id.as_str());
			sources.push(source.as_str());
			found_ecosystems.push(queried.ecosystem.as_str());
			found_names.push(queried.name.as_str());
			found_versions.push(queried.version.as_str());
		}
	}

	diesel::sql_query(
		"DELETE FROM vulnerability_matches m \
		 USING unnest($1::text[], $2::text[], $3::text[]) AS q(e, p, v) \
		 WHERE m.ecosystem = q.e AND m.package = q.p AND m.package_version = q.v \
		 AND NOT EXISTS ( \
		   SELECT 1 FROM unnest($4::text[], $5::text[], $6::text[], $7::text[]) AS f(id, e, p, v) \
		   WHERE f.id = m.vulnerability_id AND f.e = m.ecosystem \
		   AND f.p = m.package AND f.v = m.package_version)",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&versions)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_versions)
	.execute(conn)
	.await?;

	diesel::sql_query(
		"INSERT INTO vulnerability_items (vulnerability_id, source) \
		 SELECT DISTINCT id, src FROM unnest($1::text[], $2::text[]) AS f(id, src) \
		 ON CONFLICT DO NOTHING",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
	.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
	.execute(conn)
	.await?;
	diesel::sql_query(
		"INSERT INTO vulnerability_matches \
		 (vulnerability_id, source, ecosystem, package, package_version) \
		 SELECT * FROM unnest($1::text[], $2::text[], $3::text[], $4::text[], $5::text[]) \
		 ON CONFLICT DO NOTHING",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
	.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&found_versions)
	.execute(conn)
	.await?;
	Ok(())
}

/// Drop matches for package versions no longer in `keep`.
pub async fn prune_matches(conn: &mut AsyncPgConnection, keep: &[Counted]) -> Result<usize> {
	let ecosystems: Vec<&str> = keep.iter().map(|c| c.ecosystem.as_str()).collect();
	let names: Vec<&str> = keep.iter().map(|c| c.name.as_str()).collect();
	let versions: Vec<&str> = keep.iter().map(|c| c.version.as_str()).collect();
	let pruned = diesel::sql_query(
		"DELETE FROM vulnerability_matches m WHERE NOT EXISTS ( \
		   SELECT 1 FROM unnest($1::text[], $2::text[], $3::text[]) AS k(e, p, v) \
		   WHERE k.e = m.ecosystem AND k.p = m.package AND k.v = m.package_version)",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ecosystems)
	.bind::<sql_types::Array<sql_types::Text>, _>(&names)
	.bind::<sql_types::Array<sql_types::Text>, _>(&versions)
	.execute(conn)
	.await?;
	Ok(pruned)
}

// ── Exposure ────────────────────────────────────────────────────────────────

/// A Tamanu version exposed to an advisory.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityExposedVersion)]
pub struct ExposedVersion {
	/// The Tamanu version's identifier.
	pub version_id: Uuid,
	/// `major.minor.patch`.
	pub version: String,
	/// The affected package, as this version carries it.
	pub package: String,
	/// The version of the package this version carries.
	pub package_version: String,
	/// The Tamanu workspaces that reach the package.
	pub workspaces: Vec<String>,
	/// Whether any of them depends on it directly.
	pub direct: bool,
	/// How each workspace reaches it.
	pub paths: BTreeMap<String, Vec<String>>,
	/// How many live production applications run this version.
	pub applications: i64,
	/// Whether this is the newest published version.
	pub newest_published: bool,
}

/// A machine exposed to an advisory.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityExposedMachine)]
pub struct ExposedMachine {
	/// The machine's identifier.
	pub machine_id: Uuid,
	/// The machine's name.
	pub machine_name: String,
	/// The group the machine is in, if any.
	pub group_id: Option<Uuid>,
	/// That group's name.
	pub group_name: Option<String>,
	/// The affected package, as installed on the machine.
	pub package: String,
	/// The version of it installed.
	pub package_version: String,
}

/// Everything the fleet is exposed to, or one advisory's share of it.
#[derive(Clone, Debug, Default)]
pub struct Exposure {
	pub versions: Vec<(String, ExposedVersion)>,
	pub machines: Vec<(String, ExposedMachine)>,
}

impl Exposure {
	/// Worked out from the fleet as it stands: for one advisory, or for all.
	pub async fn load(
		conn: &mut AsyncPgConnection,
		fleet: &CountedFleet,
		only: Option<&str>,
	) -> Result<Self> {
		#[derive(QueryableByName)]
		struct VersionRow {
			#[diesel(sql_type = sql_types::Text)]
			vulnerability_id: String,
			#[diesel(sql_type = sql_types::Uuid)]
			version_id: Uuid,
			#[diesel(sql_type = sql_types::Text)]
			package: String,
			#[diesel(sql_type = sql_types::Text)]
			package_version: String,
			#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
			workspaces: Vec<Option<String>>,
			#[diesel(sql_type = sql_types::Bool)]
			direct: bool,
			#[diesel(sql_type = sql_types::Jsonb)]
			paths: serde_json::Value,
		}
		#[derive(QueryableByName)]
		struct MachineRow {
			#[diesel(sql_type = sql_types::Text)]
			vulnerability_id: String,
			#[diesel(sql_type = sql_types::Uuid)]
			machine_id: Uuid,
			#[diesel(sql_type = sql_types::Text)]
			machine_name: String,
			#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
			group_id: Option<Uuid>,
			#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
			group_name: Option<String>,
			#[diesel(sql_type = sql_types::Text)]
			package: String,
			#[diesel(sql_type = sql_types::Text)]
			package_version: String,
		}

		let version_ids: Vec<Uuid> = fleet.versions.iter().map(|v| v.version_id).collect();
		let version_rows: Vec<VersionRow> = diesel::sql_query(
			"SELECT m.vulnerability_id, p.version_id, p.name AS package, \
			        p.version AS package_version, p.workspaces, p.direct, p.paths \
			 FROM vulnerability_matches m \
			 JOIN version_packages p ON p.name = m.package AND p.version = m.package_version \
			 WHERE m.source = 'tamanu' AND p.version_id = ANY($1) \
			 AND ($2::text IS NULL OR m.vulnerability_id = $2) \
			 ORDER BY m.vulnerability_id, p.name",
		)
		.bind::<sql_types::Array<sql_types::Uuid>, _>(&version_ids)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(only)
		.load(conn)
		.await?;

		let counted: HashMap<Uuid, &CountedVersion> =
			fleet.versions.iter().map(|v| (v.version_id, v)).collect();
		let versions = version_rows
			.into_iter()
			.filter_map(|row| {
				let counted = counted.get(&row.version_id)?;
				Some((
					row.vulnerability_id,
					ExposedVersion {
						version_id: row.version_id,
						version: counted.version.clone(),
						package: row.package,
						package_version: row.package_version,
						workspaces: strings(row.workspaces),
						direct: row.direct,
						paths: serde_json::from_value(row.paths).unwrap_or_default(),
						applications: counted.applications.len() as i64,
						newest_published: counted.newest_published,
					},
				))
			})
			.collect();

		let machine_rows: Vec<MachineRow> = diesel::sql_query(
			"SELECT m.vulnerability_id, mp.machine_id, ma.name AS machine_name, ma.group_id, \
			        g.name AS group_name, mp.name AS package, mp.version AS package_version \
			 FROM vulnerability_matches m \
			 JOIN machine_package_reports r ON r.ecosystem = m.ecosystem \
			 JOIN machine_packages mp ON mp.machine_id = r.machine_id \
			      AND mp.name = m.package AND mp.version = m.package_version \
			 JOIN machines ma ON ma.id = mp.machine_id \
			 LEFT JOIN server_groups g ON g.id = ma.group_id \
			 WHERE m.source = 'linux' AND mp.machine_id = ANY($1) \
			 AND ($2::text IS NULL OR m.vulnerability_id = $2) \
			 ORDER BY m.vulnerability_id, ma.name, mp.name",
		)
		.bind::<sql_types::Array<sql_types::Uuid>, _>(&fleet.machines)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(only)
		.load(conn)
		.await?;
		let machines = machine_rows
			.into_iter()
			.map(|row| {
				(
					row.vulnerability_id,
					ExposedMachine {
						machine_id: row.machine_id,
						machine_name: row.machine_name,
						group_id: row.group_id,
						group_name: row.group_name,
						package: row.package,
						package_version: row.package_version,
					},
				)
			})
			.collect();

		Ok(Self { versions, machines })
	}

	/// How much each item exposes, and which items anything counts for. For a
	/// Tamanu item the figure is the applications on the exposed versions; for a
	/// Linux item, the exposed machines. A version or machine carrying several
	/// affected packages counts once.
	fn tallies(&self) -> HashMap<(String, Source), (i64, bool)> {
		let mut tallies: HashMap<(String, Source), (i64, bool)> = HashMap::new();
		let mut seen: HashSet<(&str, Uuid)> = HashSet::new();
		for (id, version) in &self.versions {
			let entry = tallies.entry((id.clone(), Source::Tamanu)).or_default();
			if seen.insert((id, version.version_id)) {
				entry.0 += version.applications;
			}
			entry.1 = true;
		}
		let mut seen: HashSet<(&str, Uuid)> = HashSet::new();
		for (id, machine) in &self.machines {
			let entry = tallies.entry((id.clone(), Source::Linux)).or_default();
			if seen.insert((id, machine.machine_id)) {
				entry.0 += 1;
			}
			entry.1 = true;
		}
		tallies
	}
}

/// Record which items anything counts for, so the model is asked only about
/// those.
pub async fn refresh_counts(conn: &mut AsyncPgConnection, exposure: &Exposure) -> Result<()> {
	let counting: Vec<(String, Source)> = exposure
		.tallies()
		.into_iter()
		.filter(|(_, (_, counts))| *counts)
		.map(|(key, _)| key)
		.collect();
	let ids: Vec<&str> = counting.iter().map(|(id, _)| id.as_str()).collect();
	let sources: Vec<&str> = counting.iter().map(|(_, source)| source.as_str()).collect();
	diesel::sql_query(
		"UPDATE vulnerability_items i SET counts = EXISTS ( \
		   SELECT 1 FROM unnest($1::text[], $2::text[]) AS k(id, src) \
		   WHERE k.id = i.vulnerability_id AND k.src = i.source)",
	)
	.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
	.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
	.execute(conn)
	.await?;
	Ok(())
}

// ── Decisions ───────────────────────────────────────────────────────────────

/// A triage decision. The newest one for an item is its current decision.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityDecision)]
pub struct Decision {
	/// The decision's identifier.
	pub id: Uuid,
	/// The advisory's OSV identifier.
	pub vulnerability_id: String,
	/// The source it decides the advisory for.
	pub source: Source,
	/// What it decides.
	pub outcome: Outcome,
	/// Why it was decided so.
	pub reason: Option<String>,
	/// For an accepted risk, the date it is looked at again, `YYYY-MM-DD`.
	pub review_by: Option<String>,
	/// For will fix, a link to the issue tracking the fix.
	pub issue_url: Option<String>,
	/// The ground the model decided on.
	pub ground: Option<Ground>,
	/// Who decided it.
	pub actor: Actor,
	/// How it was decided.
	pub via: Via,
	/// When it was decided.
	pub created_at: Timestamp,
	#[serde(skip)]
	#[schema(ignore)]
	pub held: Option<Held>,
}

#[derive(QueryableByName)]
struct DecisionRow {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Text)]
	outcome: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	reason: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	review_by: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	issue_url: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	ground: Option<String>,
	#[diesel(sql_type = sql_types::Jsonb)]
	actor: serde_json::Value,
	#[diesel(sql_type = sql_types::Text)]
	via: String,
	#[diesel(sql_type = sql_types::Text)]
	severity_held: String,
	#[diesel(sql_type = sql_types::Jsonb)]
	affected_held: serde_json::Value,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
}

impl From<DecisionRow> for Decision {
	fn from(row: DecisionRow) -> Self {
		Self {
			id: row.id,
			vulnerability_id: row.vulnerability_id,
			source: Source::parse(&row.source).unwrap_or(Source::Tamanu),
			outcome: Outcome::parse(&row.outcome).unwrap_or(Outcome::WillFix),
			reason: row.reason,
			review_by: row.review_by,
			issue_url: row.issue_url,
			ground: row.ground.as_deref().and_then(Ground::parse),
			actor: serde_json::from_value(row.actor).unwrap_or_else(|_| Actor::canopy("unknown")),
			via: Via::parse(&row.via).unwrap_or(Via::Ui),
			created_at: row.created_at,
			held: Some(Held {
				severity: row.severity_held.parse().unwrap_or_default(),
				affected: serde_json::from_value(row.affected_held).unwrap_or_default(),
			}),
		}
	}
}

const DECISION_COLUMNS: &str = "id, vulnerability_id, source, outcome, reason, \
	to_char(review_by, 'YYYY-MM-DD') AS review_by, issue_url, ground, actor, via, \
	severity_held, affected_held, created_at";

/// A decision to record.
#[derive(Clone, Debug)]
pub struct NewDecision {
	pub vulnerability_id: String,
	pub source: Source,
	pub outcome: Outcome,
	pub reason: Option<String>,
	pub review_by: Option<Date>,
	pub issue_url: Option<String>,
	pub ground: Option<Ground>,
	pub actor: Actor,
	pub via: Via,
}

impl NewDecision {
	/// A review date as a surface takes it, `YYYY-MM-DD`; blank is none.
	pub fn parse_review_by(value: Option<&str>) -> Result<Option<Date>> {
		value
			.map(str::trim)
			.filter(|date| !date.is_empty())
			.map(|date| {
				date.parse().map_err(|_| {
					AppError::BadRequest(format!("{date:?} is not a date (YYYY-MM-DD)"))
				})
			})
			.transpose()
	}

	/// What the outcome requires: a reason for not affected and accepted risk,
	/// a review date in the future for accepted risk, and for will fix an issue
	/// link that is a web address, since it is presented as a link.
	pub fn validate(&self, today: Date) -> Result<()> {
		let reason = self.reason.as_deref().map(str::trim).unwrap_or_default();
		let issue_url = self.issue_url.as_deref().map(str::trim).unwrap_or_default();
		match self.outcome {
			Outcome::WillFix if !issue_url.is_empty() => match url::Url::parse(issue_url) {
				Ok(url) if matches!(url.scheme(), "http" | "https") => Ok(()),
				_ => Err(AppError::BadRequest(
					"the issue link must be an http or https address".into(),
				)),
			},
			Outcome::NotAffected if reason.is_empty() => Err(AppError::BadRequest(
				"a not affected decision needs a reason".into(),
			)),
			Outcome::AcceptedRisk if reason.is_empty() => Err(AppError::BadRequest(
				"an accepted risk needs a reason".into(),
			)),
			Outcome::AcceptedRisk => match self.review_by {
				None => Err(AppError::BadRequest(
					"an accepted risk needs a review date".into(),
				)),
				Some(date) if date <= today => Err(AppError::BadRequest(
					"the review date must be in the future".into(),
				)),
				Some(_) => Ok(()),
			},
			_ => Ok(()),
		}
	}
}

impl Decision {
	/// Record a decision as the item's current one. The advisory's severity and
	/// affected ranges as they stand are kept with it.
	pub async fn record(
		conn: &mut AsyncPgConnection,
		new: NewDecision,
		today: Date,
	) -> Result<Self> {
		new.validate(today)?;
		let (review_by, reason) = match new.outcome {
			Outcome::AcceptedRisk => (new.review_by.map(|d| d.to_string()), new.reason),
			Outcome::NotAffected => (None, new.reason),
			Outcome::WillFix => (None, new.reason.filter(|r| !r.trim().is_empty())),
		};
		let issue_url = match new.outcome {
			Outcome::WillFix => new
				.issue_url
				.map(|u| u.trim().to_owned())
				.filter(|u| !u.is_empty()),
			_ => None,
		};

		Self::lock_item(conn, &new.vulnerability_id, new.source).await?;

		let row: DecisionRow = diesel::sql_query(format!(
			"INSERT INTO vulnerability_decisions \
			 (vulnerability_id, source, outcome, reason, review_by, issue_url, ground, actor, via, \
			  severity_held, affected_held) \
			 SELECT $1, $2, $3, $4, $5::date, $6, $7, $8, $9, a.severity, a.affected \
			 FROM vulnerabilities a WHERE a.id = $1 \
			 RETURNING {DECISION_COLUMNS}"
		))
		.bind::<sql_types::Text, _>(&new.vulnerability_id)
		.bind::<sql_types::Text, _>(new.source.as_str())
		.bind::<sql_types::Text, _>(new.outcome.as_str())
		.bind::<sql_types::Nullable<sql_types::Text>, _>(reason)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(review_by)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(issue_url)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(new.ground.map(Ground::as_str))
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(&new.actor).unwrap_or_default())
		.bind::<sql_types::Text, _>(new.via.as_str())
		.get_result(conn)
		.await?;
		Ok(row.into())
	}

	/// The audit entry for having recorded this decision.
	pub fn audited(&self) -> Action {
		Action::VulnerabilityDecided {
			advisory: audit::Record::new(
				RecordKind::Advisory,
				&self.vulnerability_id,
				&self.vulnerability_id,
			),
			source: self.source.as_str().to_owned(),
			outcome: self.outcome.as_str().to_owned(),
			reason: self.reason.clone(),
			review_by: self.review_by.clone(),
			issue_url: self.issue_url.clone(),
			via: self.via.as_str().to_owned(),
		}
	}

	/// Hold the item until the transaction ends, so decisions on it are
	/// recorded one at a time and a check made under the hold stays true until
	/// the decision is written. An item that does not exist is not found.
	pub async fn lock_item(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: Source,
	) -> Result<()> {
		let held = diesel::sql_query(
			"SELECT 1 FROM vulnerability_items \
			 WHERE vulnerability_id = $1 AND source = $2 FOR UPDATE",
		)
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.execute(conn)
		.await?;
		if held == 0 {
			return Err(AppError::NotFound(format!(
				"no {} vulnerability {vulnerability_id}",
				source.as_str()
			)));
		}
		Ok(())
	}

	/// An item's decisions, newest first.
	pub async fn history(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: Source,
	) -> Result<Vec<Self>> {
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT {DECISION_COLUMNS} FROM vulnerability_decisions \
			 WHERE vulnerability_id = $1 AND source = $2 ORDER BY created_at DESC, id"
		))
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await?;
		Ok(rows.into_iter().map(Self::from).collect())
	}

	/// Whether a person, in the operator interface or through the query
	/// interface, has ever decided the item.
	pub async fn person_has_decided(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: Source,
	) -> Result<bool> {
		#[derive(QueryableByName)]
		struct Row {
			#[diesel(sql_type = sql_types::Bool)]
			decided: bool,
		}
		let row: Row = diesel::sql_query(
			"SELECT EXISTS (SELECT 1 FROM vulnerability_decisions \
			   WHERE vulnerability_id = $1 AND source = $2 AND via <> 'model') AS decided",
		)
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.get_result(conn)
		.await?;
		Ok(row.decided)
	}
}

// ── Reopening ───────────────────────────────────────────────────────────────

/// A decided item that opened again, with the decision it opened from.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityReopened)]
pub struct Reopened {
	/// The reopening's identifier.
	pub id: Uuid,
	/// The advisory's OSV identifier.
	pub vulnerability_id: String,
	/// The source the item belongs to.
	pub source: Source,
	/// The decision it opened from.
	pub decision_id: Uuid,
	/// Why, in words.
	pub why: String,
	/// Why, as a tagged reason: `review_due`, `severity_rose` or `range_widened`.
	#[schema(value_type = Object)]
	pub reason: Reopening,
	/// When it opened again.
	pub created_at: Timestamp,
}

#[derive(QueryableByName)]
struct ReopenedRow {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Uuid)]
	decision_id: Uuid,
	#[diesel(sql_type = sql_types::Jsonb)]
	reason: serde_json::Value,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
}

impl From<ReopenedRow> for Reopened {
	fn from(row: ReopenedRow) -> Self {
		let reason: Reopening = serde_json::from_value(row.reason).unwrap_or(Reopening::ReviewDue);
		Self {
			id: row.id,
			vulnerability_id: row.vulnerability_id,
			source: Source::parse(&row.source).unwrap_or(Source::Tamanu),
			decision_id: row.decision_id,
			why: reason.describe(),
			reason,
			created_at: row.created_at,
		}
	}
}

impl Reopened {
	/// Open a decided item again. Its suggestion goes, so the model looks at it
	/// afresh, and a decision is reopened once.
	pub async fn record(
		conn: &mut AsyncPgConnection,
		decision: &Decision,
		reason: &Reopening,
	) -> Result<Option<Self>> {
		let row: Option<ReopenedRow> = diesel::sql_query(
			"INSERT INTO vulnerability_reopenings (vulnerability_id, source, decision_id, reason) \
			 VALUES ($1, $2, $3, $4) ON CONFLICT (decision_id) DO NOTHING \
			 RETURNING id, vulnerability_id, source, decision_id, reason, created_at",
		)
		.bind::<sql_types::Text, _>(&decision.vulnerability_id)
		.bind::<sql_types::Text, _>(decision.source.as_str())
		.bind::<sql_types::Uuid, _>(decision.id)
		.bind::<sql_types::Jsonb, _>(serde_json::to_value(reason).unwrap_or_default())
		.get_result(conn)
		.await
		.optional()?;
		let Some(row) = row else { return Ok(None) };

		diesel::sql_query(
			"DELETE FROM vulnerability_suggestions WHERE vulnerability_id = $1 AND source = $2",
		)
		.bind::<sql_types::Text, _>(&decision.vulnerability_id)
		.bind::<sql_types::Text, _>(decision.source.as_str())
		.execute(conn)
		.await?;
		diesel::sql_query(
			"UPDATE vulnerability_items SET model_attempted_at = NULL \
			 WHERE vulnerability_id = $1 AND source = $2",
		)
		.bind::<sql_types::Text, _>(&decision.vulnerability_id)
		.bind::<sql_types::Text, _>(decision.source.as_str())
		.execute(conn)
		.await?;
		Ok(Some(row.into()))
	}

	/// Look at every standing decision: an accepted risk past its review date
	/// reopens, and so does a decision whose advisory has changed so that it no
	/// longer covers what `counted` holds. Returns what reopened, for the caller
	/// to audit.
	pub async fn pass(
		conn: &mut AsyncPgConnection,
		counted: &[Counted],
		today: Date,
	) -> Result<Vec<Self>> {
		// A decision reopens once, so only current decisions not yet reopened are
		// looked at.
		let rows: Vec<DecisionRow> = diesel::sql_query(format!(
			"SELECT * FROM ( \
			   SELECT DISTINCT ON (vulnerability_id, source) {DECISION_COLUMNS} \
			   FROM vulnerability_decisions ORDER BY vulnerability_id, source, created_at DESC, id \
			 ) d WHERE NOT EXISTS (SELECT 1 FROM vulnerability_reopenings r WHERE r.decision_id = d.id)"
		))
		.load(conn)
		.await?;
		let standing: Vec<Decision> = rows.into_iter().map(Decision::from).collect();

		let (tamanu, linux): (Vec<Counted>, Vec<Counted>) = counted
			.iter()
			.cloned()
			.partition(|c| Source::of_ecosystem(&c.ecosystem) == Source::Tamanu);

		let ids: Vec<String> = standing
			.iter()
			.map(|decision| decision.vulnerability_id.clone())
			.collect();
		let advisories: HashMap<String, Advisory> = Advisory::get_many(conn, &ids)
			.await?
			.into_iter()
			.map(|advisory| (advisory.id.clone(), advisory))
			.collect();

		let mut reopened = Vec::new();
		for decision in standing {
			let review_due = decision.outcome == Outcome::AcceptedRisk
				&& decision
					.review_by
					.as_deref()
					.and_then(|date| date.parse::<Date>().ok())
					.is_some_and(|date| date <= today);
			let reason = if review_due {
				Some(Reopening::ReviewDue)
			} else {
				let counted_here = match decision.source {
					Source::Tamanu => &tamanu,
					Source::Linux => &linux,
				};
				match (&decision.held, advisories.get(&decision.vulnerability_id)) {
					(Some(held), Some(advisory)) => {
						held.reopened_by(advisory.severity, &advisory.affected, counted_here)
					}
					_ => None,
				}
			};
			if let Some(reason) = reason
				&& let Some(done) = Self::record(conn, &decision, &reason).await?
			{
				reopened.push(done);
			}
		}
		Ok(reopened)
	}
}

// ── Suggestions ─────────────────────────────────────────────────────────────

/// The model's latest suggestion for an item. Not a decision: the item stays
/// open.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilitySuggestion)]
pub struct Suggestion {
	/// The decision the model suggests.
	pub outcome: Outcome,
	/// The ground it suggests not affected on, when it does.
	pub ground: Option<Ground>,
	/// Why, in the model's words.
	pub reason: String,
	/// The model that made it.
	pub model: String,
	/// When it was made.
	pub created_at: Timestamp,
}

#[derive(QueryableByName)]
struct SuggestionRow {
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Text)]
	outcome: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	ground: Option<String>,
	#[diesel(sql_type = sql_types::Text)]
	reason: String,
	#[diesel(sql_type = sql_types::Text)]
	model: String,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	created_at: Timestamp,
}

impl SuggestionRow {
	fn split(self) -> ((String, Source), Suggestion) {
		(
			(
				self.vulnerability_id,
				Source::parse(&self.source).unwrap_or(Source::Tamanu),
			),
			Suggestion {
				outcome: Outcome::parse(&self.outcome).unwrap_or(Outcome::WillFix),
				ground: self.ground.as_deref().and_then(Ground::parse),
				reason: self.reason,
				model: self.model,
				created_at: self.created_at,
			},
		)
	}
}

impl Suggestion {
	/// Keep `suggestion` as the item's suggestion, replacing the one before.
	pub async fn record(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: Source,
		outcome: Outcome,
		ground: Option<Ground>,
		reason: &str,
		model: &str,
	) -> Result<()> {
		diesel::sql_query(
			"INSERT INTO vulnerability_suggestions \
			 (vulnerability_id, source, outcome, ground, reason, model) \
			 VALUES ($1, $2, $3, $4, $5, $6) \
			 ON CONFLICT (vulnerability_id, source) DO UPDATE SET outcome = EXCLUDED.outcome, \
			 ground = EXCLUDED.ground, reason = EXCLUDED.reason, model = EXCLUDED.model, \
			 created_at = now()",
		)
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.bind::<sql_types::Text, _>(outcome.as_str())
		.bind::<sql_types::Nullable<sql_types::Text>, _>(ground.map(Ground::as_str))
		.bind::<sql_types::Text, _>(reason)
		.bind::<sql_types::Text, _>(model)
		.execute(conn)
		.await?;
		Ok(())
	}

	pub async fn get(
		conn: &mut AsyncPgConnection,
		vulnerability_id: &str,
		source: Source,
	) -> Result<Option<Self>> {
		let row: Option<SuggestionRow> = diesel::sql_query(
			"SELECT vulnerability_id, source, outcome, ground, reason, model, created_at \
			 FROM vulnerability_suggestions WHERE vulnerability_id = $1 AND source = $2",
		)
		.bind::<sql_types::Text, _>(vulnerability_id)
		.bind::<sql_types::Text, _>(source.as_str())
		.get_result(conn)
		.await
		.optional()?;
		Ok(row.map(|row| row.split().1))
	}
}

// ── Items ───────────────────────────────────────────────────────────────────

/// An item as the list and the query interface present it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityItem)]
pub struct ItemSummary {
	/// The advisory's OSV identifier.
	pub id: String,
	/// Where the item's exposure comes from.
	pub source: Source,
	/// Other identifiers for the same flaw, such as its CVE.
	pub aliases: Vec<String>,
	/// The advisory's one-line summary.
	pub summary: String,
	/// The advisory's severity.
	pub severity: Severity,
	/// The affected packages that match something the fleet counts.
	pub packages: Vec<String>,
	/// How many applications (Tamanu) or machines (Linux) it exposes.
	pub exposed: i64,
	/// Whether anything counts for it.
	pub counts: bool,
	/// Where its triage stands.
	pub status: Status,
	/// The current decision, if the item has been decided and not reopened.
	pub decision: Option<Decision>,
	/// Why the item opened again, when it has.
	pub reopened: Option<ReopenedBecause>,
	/// The model's latest suggestion, if it has made one.
	pub suggestion: Option<Suggestion>,
	/// When a scan first found it.
	pub first_seen_at: Timestamp,
}

/// Why a decided item is open again: a reopening Canopy recorded, or an
/// accepted risk past its review date that the scan has yet to record.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityReopenedBecause)]
pub struct ReopenedBecause {
	/// Why, in words.
	pub why: String,
	/// Why, as a tagged reason: `review_due`, `severity_rose` or `range_widened`.
	#[schema(value_type = Object)]
	pub reason: Reopening,
}

impl From<Reopening> for ReopenedBecause {
	fn from(reason: Reopening) -> Self {
		Self {
			why: reason.describe(),
			reason,
		}
	}
}

/// Which items to list.
#[derive(Clone, Debug, Default)]
pub struct ListFilter {
	pub source: Option<Source>,
	pub status: Option<Status>,
	pub min_severity: Option<Severity>,
	/// Include items nothing counts for any more.
	pub include_uncounted: bool,
	pub limit: Option<usize>,
}

/// The items matching a filter, in order.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityListing)]
pub struct Listing {
	/// The items, in order.
	pub items: Vec<ItemSummary>,
	/// Whether more matched than are listed.
	pub truncated: bool,
}

#[derive(QueryableByName)]
struct ItemRow {
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	first_seen_at: Timestamp,
}

impl ItemSummary {
	/// The current standing of an item, from its newest decision and whether it
	/// reopened or its review date has passed.
	fn standing(
		decision: Option<&Decision>,
		reopened: Option<&Reopened>,
		today: Date,
	) -> (Status, Option<ReopenedBecause>) {
		let Some(decision) = decision else {
			return (Status::Open, None);
		};
		if let Some(reopened) = reopened.filter(|r| r.decision_id == decision.id) {
			return (Status::Open, Some(reopened.reason.clone().into()));
		}
		let review_due = decision.outcome == Outcome::AcceptedRisk
			&& decision
				.review_by
				.as_deref()
				.and_then(|date| date.parse::<Date>().ok())
				.is_some_and(|date| date <= today);
		if review_due {
			// Past its review date, before the scan has written it up as reopened.
			return (Status::Open, Some(Reopening::ReviewDue.into()));
		}
		(Status::from(decision.outcome), None)
	}

	/// Every item that has a match, with what is known of it, sorted by
	/// severity and then by how much each exposes.
	pub async fn list(
		conn: &mut AsyncPgConnection,
		filter: &ListFilter,
		today: Date,
	) -> Result<Listing> {
		let fleet = CountedFleet::load(conn).await?;
		let exposure = Exposure::load(conn, &fleet, None).await?;
		let mut items: Vec<Self> = Self::load(conn, &exposure, None, today)
			.await?
			.into_iter()
			.filter(|item| {
				!(filter.source.is_some_and(|s| s != item.source)
					|| filter.status.is_some_and(|s| s != item.status)
					|| filter.min_severity.is_some_and(|s| item.severity < s)
					|| (!filter.include_uncounted && !item.counts))
			})
			.collect();

		items.sort_by(|a, b| {
			b.severity
				.cmp(&a.severity)
				.then(b.exposed.cmp(&a.exposed))
				.then_with(|| a.id.cmp(&b.id))
				.then_with(|| a.source.as_str().cmp(b.source.as_str()))
		});
		let truncated = filter.limit.is_some_and(|limit| items.len() > limit);
		if let Some(limit) = filter.limit {
			items.truncate(limit);
		}
		Ok(Listing { items, truncated })
	}

	/// The items `exposure` was worked out for, or the one item `only` names.
	async fn load(
		conn: &mut AsyncPgConnection,
		exposure: &Exposure,
		only: Option<(&str, Source)>,
		today: Date,
	) -> Result<Vec<Self>> {
		let tallies = exposure.tallies();
		let only_id = only.map(|(id, _)| id);
		let only_source = only.map(|(_, source)| source.as_str());
		const ONLY: &str = "($1::text IS NULL OR vulnerability_id = $1) \
			AND ($2::text IS NULL OR source = $2)";

		let rows: Vec<ItemRow> = diesel::sql_query(format!(
			"SELECT vulnerability_id, source, first_seen_at FROM vulnerability_items WHERE {ONLY}"
		))
		.bind::<sql_types::Nullable<sql_types::Text>, _>(only_id)
		.bind::<sql_types::Nullable<sql_types::Text>, _>(only_source)
		.load(conn)
		.await?;
		let ids: Vec<String> = rows
			.iter()
			.map(|row| row.vulnerability_id.clone())
			.collect();
		let advisories: HashMap<String, Advisory> = Advisory::get_many(conn, &ids)
			.await?
			.into_iter()
			.map(|advisory| (advisory.id.clone(), advisory))
			.collect();

		let decisions: HashMap<(String, Source), Decision> = {
			let rows: Vec<DecisionRow> = diesel::sql_query(format!(
				"SELECT DISTINCT ON (vulnerability_id, source) {DECISION_COLUMNS} \
				 FROM vulnerability_decisions WHERE {ONLY} \
				 ORDER BY vulnerability_id, source, created_at DESC, id"
			))
			.bind::<sql_types::Nullable<sql_types::Text>, _>(only_id)
			.bind::<sql_types::Nullable<sql_types::Text>, _>(only_source)
			.load(conn)
			.await?;
			rows.into_iter()
				.map(Decision::from)
				.map(|d| ((d.vulnerability_id.clone(), d.source), d))
				.collect()
		};
		let reopenings: HashMap<(String, Source), Reopened> = {
			let rows: Vec<ReopenedRow> = diesel::sql_query(format!(
				"SELECT DISTINCT ON (vulnerability_id, source) \
				 id, vulnerability_id, source, decision_id, reason, created_at \
				 FROM vulnerability_reopenings WHERE {ONLY} \
				 ORDER BY vulnerability_id, source, created_at DESC, id"
			))
			.bind::<sql_types::Nullable<sql_types::Text>, _>(only_id)
			.bind::<sql_types::Nullable<sql_types::Text>, _>(only_source)
			.load(conn)
			.await?;
			rows.into_iter()
				.map(Reopened::from)
				.map(|r| ((r.vulnerability_id.clone(), r.source), r))
				.collect()
		};
		let suggestions: HashMap<(String, Source), Suggestion> = {
			let rows: Vec<SuggestionRow> = diesel::sql_query(format!(
				"SELECT vulnerability_id, source, outcome, ground, reason, model, created_at \
				 FROM vulnerability_suggestions WHERE {ONLY}"
			))
			.bind::<sql_types::Nullable<sql_types::Text>, _>(only_id)
			.bind::<sql_types::Nullable<sql_types::Text>, _>(only_source)
			.load(conn)
			.await?;
			rows.into_iter().map(SuggestionRow::split).collect()
		};
		let packages: HashMap<(String, Source), Vec<String>> = {
			let mut map: HashMap<(String, Source), Vec<String>> = HashMap::new();
			for (id, version) in &exposure.versions {
				map.entry((id.clone(), Source::Tamanu))
					.or_default()
					.push(version.package.clone());
			}
			for (id, machine) in &exposure.machines {
				map.entry((id.clone(), Source::Linux))
					.or_default()
					.push(machine.package.clone());
			}
			for names in map.values_mut() {
				names.sort();
				names.dedup();
			}
			map
		};

		let mut items = Vec::new();
		for row in rows {
			let source = Source::parse(&row.source).unwrap_or(Source::Tamanu);
			let key = (row.vulnerability_id.clone(), source);
			let Some(advisory) = advisories.get(&row.vulnerability_id) else {
				continue;
			};
			let (exposed, counts) = tallies.get(&key).copied().unwrap_or_default();
			let decision = decisions.get(&key);
			let reopened = reopenings.get(&key);
			let (status, reopened) = Self::standing(decision, reopened, today);

			items.push(Self {
				id: row.vulnerability_id,
				source,
				aliases: advisory.aliases.clone(),
				summary: advisory.summary.clone(),
				severity: advisory.severity,
				packages: packages.get(&key).cloned().unwrap_or_default(),
				exposed,
				counts,
				status,
				decision: decision.filter(|_| status != Status::Open).cloned(),
				reopened,
				suggestion: (status == Status::Open)
					.then(|| suggestions.get(&key).cloned())
					.flatten(),
				first_seen_at: row.first_seen_at,
			});
		}
		Ok(items)
	}
}

/// One entry of an item's history.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
#[schema(as = VulnerabilityHistoryEntry)]
pub enum HistoryEntry {
	Decision {
		at: Timestamp,
		decision: Decision,
	},
	Reopening {
		at: Timestamp,
		reopened: Reopened,
	},
	Suggestion {
		at: Timestamp,
		suggestion: Suggestion,
	},
}

impl HistoryEntry {
	fn at(&self) -> Timestamp {
		match self {
			Self::Decision { at, .. }
			| Self::Reopening { at, .. }
			| Self::Suggestion { at, .. } => *at,
		}
	}
}

/// An item in full.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityDetail)]
pub struct ItemDetail {
	/// The item as the list presents it.
	pub item: ItemSummary,
	/// The advisory, as cached from OSV.
	pub advisory: Advisory,
	/// The versions that fix it, for each affected package it matches.
	pub fixed_in: BTreeMap<String, Vec<String>>,
	/// For a Tamanu item, the exposed versions.
	pub versions: Vec<ExposedVersion>,
	/// For a Linux item, the exposed machines.
	pub machines: Vec<ExposedMachine>,
	/// Decisions, reopenings and the suggestion, newest first.
	pub history: Vec<HistoryEntry>,
	/// Whether a person has ever decided the item.
	pub person_has_decided: bool,
}

impl ItemDetail {
	pub async fn load(
		conn: &mut AsyncPgConnection,
		id: &str,
		source: Source,
		today: Date,
	) -> Result<Self> {
		let advisory = Advisory::get(conn, id)
			.await?
			.ok_or_else(|| AppError::NotFound(format!("no vulnerability {id}")))?;
		let fleet = CountedFleet::load(conn).await?;
		let exposure = Exposure::load(conn, &fleet, Some(id)).await?;
		let item = ItemSummary::load(conn, &exposure, Some((id, source)), today)
			.await?
			.into_iter()
			.next()
			.ok_or_else(|| {
				AppError::NotFound(format!("no {} vulnerability {id}", source.as_str()))
			})?;
		let versions: Vec<ExposedVersion> = exposure
			.versions
			.into_iter()
			.filter(|_| source == Source::Tamanu)
			.map(|(_, version)| version)
			.collect();
		let machines: Vec<ExposedMachine> = exposure
			.machines
			.into_iter()
			.filter(|_| source == Source::Linux)
			.map(|(_, machine)| machine)
			.collect();

		let mut fixed_in = BTreeMap::new();
		for package in &item.packages {
			let ecosystems: Vec<String> = match source {
				Source::Tamanu => vec![NPM.to_owned()],
				Source::Linux => advisory
					.affected
					.iter()
					.filter_map(|a| a.package.as_ref().map(|p| p.ecosystem.clone()))
					.collect(),
			};
			let record = Record {
				id: advisory.id.clone(),
				affected: advisory.affected.clone(),
				..Default::default()
			};
			let mut fixes: Vec<String> = ecosystems
				.iter()
				.flat_map(|ecosystem| record.fixed_in(ecosystem, package))
				.collect();
			fixes.sort();
			fixes.dedup();
			fixed_in.insert(package.clone(), fixes);
		}

		let mut history: Vec<HistoryEntry> = Decision::history(conn, id, source)
			.await?
			.into_iter()
			.map(|decision| HistoryEntry::Decision {
				at: decision.created_at,
				decision,
			})
			.collect();
		let reopenings: Vec<ReopenedRow> = diesel::sql_query(
			"SELECT id, vulnerability_id, source, decision_id, reason, created_at \
			 FROM vulnerability_reopenings WHERE vulnerability_id = $1 AND source = $2",
		)
		.bind::<sql_types::Text, _>(id)
		.bind::<sql_types::Text, _>(source.as_str())
		.load(conn)
		.await?;
		history.extend(reopenings.into_iter().map(|row| {
			let reopened = Reopened::from(row);
			HistoryEntry::Reopening {
				at: reopened.created_at,
				reopened,
			}
		}));
		if let Some(suggestion) = Suggestion::get(conn, id, source).await? {
			history.push(HistoryEntry::Suggestion {
				at: suggestion.created_at,
				suggestion,
			});
		}
		history.sort_by_key(|entry| std::cmp::Reverse(entry.at()));

		Ok(Self {
			item,
			advisory,
			fixed_in,
			versions,
			machines,
			history,
			person_has_decided: Decision::person_has_decided(conn, id, source).await?,
		})
	}
}

// ── The model's queue ───────────────────────────────────────────────────────

/// Claim up to `limit` items for the model to look at: open, counted for, with
/// no suggestion, and not looked at within `retry_after`. Claiming marks the
/// attempt, so a reply that never comes is asked for again only after
/// `retry_after`, and two workers do not take the same item.
pub async fn claim_for_model(
	conn: &mut AsyncPgConnection,
	limit: i64,
	retry_after: jiff::SignedDuration,
) -> Result<Vec<(String, Source)>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Text)]
		vulnerability_id: String,
		#[diesel(sql_type = sql_types::Text)]
		source: String,
	}
	let rows: Vec<Row> = diesel::sql_query(
		"UPDATE vulnerability_items i SET model_attempted_at = now() \
		 WHERE (i.vulnerability_id, i.source) IN ( \
		   SELECT c.vulnerability_id, c.source FROM vulnerability_items c \
		   WHERE c.counts \
		   AND NOT EXISTS (SELECT 1 FROM vulnerability_suggestions s \
		        WHERE s.vulnerability_id = c.vulnerability_id AND s.source = c.source) \
		   AND ( \
		     NOT EXISTS (SELECT 1 FROM vulnerability_decisions d \
		          WHERE d.vulnerability_id = c.vulnerability_id AND d.source = c.source) \
		     OR EXISTS (SELECT 1 FROM vulnerability_reopenings r \
		          WHERE r.vulnerability_id = c.vulnerability_id AND r.source = c.source \
		          AND r.decision_id = (SELECT d.id FROM vulnerability_decisions d \
		               WHERE d.vulnerability_id = c.vulnerability_id AND d.source = c.source \
		               ORDER BY d.created_at DESC, d.id LIMIT 1)) \
		   ) \
		   AND (c.model_attempted_at IS NULL OR c.model_attempted_at < now() - make_interval(secs => $2)) \
		   ORDER BY c.first_seen_at LIMIT $1 \
		   FOR UPDATE OF c SKIP LOCKED) \
		 RETURNING i.vulnerability_id, i.source",
	)
	.bind::<sql_types::BigInt, _>(limit)
	.bind::<sql_types::Double, _>(retry_after.as_secs_f64())
	.load(conn)
	.await?;
	Ok(rows
		.into_iter()
		.filter_map(|row| Some((row.vulnerability_id, Source::parse(&row.source)?)))
		.collect())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn new_decision(outcome: Outcome) -> NewDecision {
		NewDecision {
			vulnerability_id: "GHSA-x".into(),
			source: Source::Tamanu,
			outcome,
			reason: None,
			review_by: None,
			issue_url: None,
			ground: None,
			actor: Actor::canopy("test"),
			via: Via::Ui,
		}
	}

	fn today() -> Date {
		jiff::civil::date(2026, 11, 5)
	}

	#[test]
	fn not_affected_and_accepted_risk_need_a_reason() {
		assert!(
			new_decision(Outcome::NotAffected)
				.validate(today())
				.is_err()
		);
		let mut risk = new_decision(Outcome::AcceptedRisk);
		risk.review_by = Some(jiff::civil::date(2027, 1, 1));
		assert!(risk.validate(today()).is_err());
		risk.reason = Some("   ".into());
		assert!(risk.validate(today()).is_err());
		risk.reason = Some("behind the firewall".into());
		assert!(risk.validate(today()).is_ok());
	}

	#[test]
	fn an_accepted_risk_needs_a_review_date_in_the_future() {
		let mut risk = new_decision(Outcome::AcceptedRisk);
		risk.reason = Some("accepted".into());
		assert!(risk.validate(today()).is_err());
		risk.review_by = Some(today());
		assert!(risk.validate(today()).is_err());
		risk.review_by = Some(jiff::civil::date(2026, 11, 6));
		assert!(risk.validate(today()).is_ok());
	}

	#[test]
	fn will_fix_needs_nothing() {
		assert!(new_decision(Outcome::WillFix).validate(today()).is_ok());
	}

	#[test]
	fn an_issue_link_must_be_a_web_address() {
		let mut fix = new_decision(Outcome::WillFix);
		fix.issue_url = Some("https://github.com/beyondessential/tamanu/issues/1".into());
		assert!(fix.validate(today()).is_ok());
		for bad in ["javascript:alert(1)", "data:text/html,x", "not a link"] {
			fix.issue_url = Some(bad.into());
			assert!(fix.validate(today()).is_err(), "{bad}");
		}
	}
}
