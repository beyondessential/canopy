//! Vulnerabilities in the dependencies of the repositories Canopy tracks and in
//! the packages installed on production machines, and the triage decisions
//! taken on them.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`) and
//! `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).
//!
//! An item is one OSV advisory within one source, a repository or Linux. What
//! the scan stores is the advisories and which package versions each matches;
//! exposure, which repository packages and production machines that reaches, is
//! a join worked out when read, so a fix landing, or ranking or archiving a
//! machine, changes it without a rescan.

use std::collections::HashMap;

use commons_errors::Result;
use diesel::{prelude::*, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

use crate::{
	osv::{self, Affected, Record, Severity},
	packages::REPOSITORY_ECOSYSTEMS,
};

mod counted;
mod decisions;
mod items;
mod sources;
mod suggestions;

pub use counted::*;
pub use decisions::*;
pub use items::*;
pub use sources::*;
pub use suggestions::*;

/// A repository whose default branch Canopy reads dependencies from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Repository {
	/// The name items carry as their source.
	pub name: &'static str,
	/// The GitHub repository, `owner/name`.
	pub slug: &'static str,
}

/// The repositories Canopy tracks.
pub const REPOSITORIES: [Repository; 8] = [
	Repository {
		name: "tamanu",
		slug: "beyondessential/tamanu",
	},
	Repository {
		name: "canopy",
		slug: "beyondessential/canopy",
	},
	Repository {
		name: "bestool",
		slug: "beyondessential/bestool",
	},
	Repository {
		name: "seedling",
		slug: "beyondessential/seedling",
	},
	Repository {
		name: "infra",
		slug: "beyondessential/infra",
	},
	Repository {
		name: "tupaia",
		slug: "beyondessential/tupaia",
	},
	Repository {
		name: "bliti",
		slug: "beyondessential/bliti",
	},
	// Uncomment to track workhorse again; items already scanned from it are kept
	// with their decisions, out of view until it is tracked once more.
	// Repository {
	// 	name: "workhorse",
	// 	slug: "beyondessential/workhorse",
	// },
	Repository {
		name: "pollen",
		slug: "beyondessential/pollen",
	},
];

/// The source of the packages installed on machines.
pub const LINUX: &str = "linux";

/// Where an item's exposure comes from: a tracked repository, named as in
/// [`REPOSITORIES`], or the packages installed on machines. Written as its
/// name.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub enum Source {
	/// The default branch of a tracked repository.
	Repository(String),
	/// The packages installed on machines.
	Linux,
}

impl Source {
	pub fn as_str(&self) -> &str {
		match self {
			Self::Repository(name) => name,
			Self::Linux => LINUX,
		}
	}

	/// A source by name: `linux`, or one of the tracked repositories.
	pub fn parse(s: &str) -> Option<Self> {
		if s == LINUX {
			return Some(Self::Linux);
		}
		REPOSITORIES
			.iter()
			.find(|repository| repository.name == s)
			.map(|repository| Self::Repository(repository.name.to_owned()))
	}

	/// Whether packages of `ecosystem` come from a repository's lockfiles.
	pub fn is_repository_ecosystem(ecosystem: &str) -> bool {
		REPOSITORY_ECOSYSTEMS.contains(&ecosystem)
	}

	pub fn is_linux(&self) -> bool {
		matches!(self, Self::Linux)
	}
}

impl utoipa::PartialSchema for Source {
	fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
		utoipa::openapi::ObjectBuilder::new()
			.schema_type(utoipa::openapi::schema::SchemaType::Type(
				utoipa::openapi::schema::Type::String,
			))
			.description(Some(
				"Where an item's exposure comes from: the name of a tracked repository, \
				 such as `tamanu`, or `linux` for the packages installed on machines.",
			))
			.examples([serde_json::json!("tamanu")])
			.build()
			.into()
	}
}

impl utoipa::ToSchema for Source {
	fn name() -> std::borrow::Cow<'static, str> {
		"VulnerabilitySource".into()
	}
}

impl TryFrom<String> for Source {
	type Error = String;

	fn try_from(s: String) -> std::result::Result<Self, String> {
		Self::parse(&s).ok_or_else(|| format!("{s:?} is not a vulnerability source"))
	}
}

impl From<Source> for String {
	fn from(source: Source) -> Self {
		source.as_str().to_owned()
	}
}

/// What a decision says about an item.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityOutcome)]
pub enum Outcome {
	/// The source cannot be hit by the flaw.
	NotAffected,
	/// The source is exposed and lives with it.
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
	/// Needs triage: no decision, or one that was withdrawn or lapsed.
	Open,
	NotAffected,
	AcceptedRisk,
	WillFix,
	/// A split decision whose entries do not all give the same outcome.
	Mixed,
}

impl Status {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Open => "open",
			Self::NotAffected => "not_affected",
			Self::AcceptedRisk => "accepted_risk",
			Self::WillFix => "will_fix",
			Self::Mixed => "mixed",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"open" => Some(Self::Open),
			"mixed" => Some(Self::Mixed),
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

/// How a decision or a change to one was made.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityVia)]
pub enum Via {
	/// An operator, in the operator interface.
	Ui,
	/// The model. Decisions the model once made; it only suggests now.
	Model,
	/// A caller of the fleet query interface.
	Mcp,
	/// Canopy itself, as when it withdraws what the model once decided.
	Canopy,
}

impl Via {
	pub fn as_str(self) -> &'static str {
		match self {
			Self::Ui => "ui",
			Self::Model => "model",
			Self::Mcp => "mcp",
			Self::Canopy => "canopy",
		}
	}

	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"ui" => Some(Self::Ui),
			"model" => Some(Self::Model),
			"mcp" => Some(Self::Mcp),
			"canopy" => Some(Self::Canopy),
			_ => None,
		}
	}
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

	/// The severity and affected ranges of each of `ids`, which is all that
	/// telling whether a revision lapses a decision reads of an advisory: its
	/// long text and references are left in the database.
	pub async fn rated(
		conn: &mut AsyncPgConnection,
		ids: &[String],
	) -> Result<HashMap<String, (Severity, Vec<Affected>)>> {
		#[derive(QueryableByName)]
		struct Row {
			#[diesel(sql_type = sql_types::Text)]
			id: String,
			#[diesel(sql_type = sql_types::Text)]
			severity: String,
			#[diesel(sql_type = sql_types::Jsonb)]
			affected: serde_json::Value,
		}
		let rows: Vec<Row> = diesel::sql_query(
			"SELECT id, severity, affected FROM vulnerabilities WHERE id = ANY($1)",
		)
		.bind::<sql_types::Array<sql_types::Text>, _>(ids)
		.load(conn)
		.await?;
		Ok(rows
			.into_iter()
			.map(|row| {
				(
					row.id,
					(
						row.severity.parse().unwrap_or_default(),
						serde_json::from_value(row.affected).unwrap_or_default(),
					),
				)
			})
			.collect())
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
