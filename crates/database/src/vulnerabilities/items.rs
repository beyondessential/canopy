//! Items as the list, the query interface and an item's page present them.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`).
//!
//! Everything reads one SQL view of the items, so the list, its facet counts,
//! grouping, the sidebar count and the model's queue agree. An item counts
//! while something carries an affected package version: a repository's default
//! branch as last read, or a live production machine. Exposure is joined when
//! read, so nothing here goes stale.

use std::collections::{BTreeMap, HashMap};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::sql_types;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use super::{
	Advisory, LINUX, REPOSITORIES, Source, Status,
	decisions::{Decision, Edit, Lapse, Withdrawal},
	strings,
	suggestions::Suggestion,
};
use crate::{
	osv::{self, Record, Severity},
	packages::{PackagePath, RepositoryRead, Scope},
};

/// What each item is exposed through. Binds `$1`, the identifiers to look at
/// (all when `NULL`), and `$2`, the rank that counts for Linux.
pub(crate) const EXPOSURE_CTE: &str = "exposure AS ( \
	SELECT m.vulnerability_id, rp.repository AS source, m.ecosystem, m.package AS name, \
	       m.package_version AS version, NULL::uuid AS machine_id \
	FROM vulnerability_matches m \
	JOIN repository_packages rp ON rp.ecosystem = m.ecosystem AND rp.name = m.package \
	     AND rp.version = m.package_version \
	WHERE ($1::text[] IS NULL OR m.vulnerability_id = ANY($1)) \
	UNION ALL \
	SELECT m.vulnerability_id, 'linux'::text, m.ecosystem, m.package, m.package_version, mp.machine_id \
	FROM vulnerability_matches m \
	JOIN machine_package_reports r ON r.ecosystem = m.ecosystem \
	JOIN machines ma ON ma.id = r.machine_id AND ma.deleted_at IS NULL AND ma.rank = $2 \
	JOIN machine_packages mp ON mp.machine_id = r.machine_id AND mp.name = m.package \
	     AND mp.version = m.package_version \
	WHERE ($1::text[] IS NULL OR m.vulnerability_id = ANY($1)))";

/// The rest of the view, over [`EXPOSURE_CTE`]: one row per item.
const ITEM_CTES: &str = "agg AS ( \
	  SELECT e.vulnerability_id, e.source, array_agg(DISTINCT e.name) AS packages, \
	         count(DISTINCT e.machine_id) AS machines \
	  FROM exposure e GROUP BY 1, 2), \
	paths AS ( \
	  SELECT e.vulnerability_id, e.source, array_agg(DISTINCT p.scope) AS scopes, \
	         bool_or(p.optional) AS any_optional, \
	         array_agg(DISTINCT p.workspace_package) AS workspace_packages \
	  FROM exposure e JOIN repository_package_paths p \
	    ON p.repository = e.source AND p.ecosystem = e.ecosystem \
	   AND p.name = e.name AND p.version = e.version \
	  GROUP BY 1, 2), \
	newest AS ( \
	  SELECT DISTINCT ON (vulnerability_id, source) id, vulnerability_id, source \
	  FROM vulnerability_decisions \
	  ORDER BY vulnerability_id, source, created_at DESC, id), \
	state AS ( \
	  SELECT n.vulnerability_id, n.source, n.id AS decision_id, \
	    EXISTS (SELECT 1 FROM vulnerability_decision_withdrawals w WHERE w.decision_id = n.id) AS withdrawn, \
	    EXISTS (SELECT 1 FROM vulnerability_lapses l WHERE l.decision_id = n.id) AS lapsed, \
	    EXISTS (SELECT 1 FROM vulnerability_decision_edits ed WHERE ed.decision_id = n.id) AS edited, \
	    (SELECT count(DISTINCT en.outcome) FROM vulnerability_decision_entries en \
	      WHERE en.decision_id = n.id) AS outcomes, \
	    (SELECT min(en.outcome) FROM vulnerability_decision_entries en \
	      WHERE en.decision_id = n.id) AS outcome \
	  FROM newest n \
	  WHERE EXISTS (SELECT 1 FROM vulnerability_decision_entries en WHERE en.decision_id = n.id)), \
	base AS ( \
	  SELECT i.vulnerability_id AS id, i.source, i.first_seen_at, i.search_text, \
	    v.summary, v.aliases, v.severity, v.published_at, v.details, \
	    CASE v.severity WHEN 'critical' THEN 4 WHEN 'high' THEN 3 WHEN 'medium' THEN 2 \
	                    WHEN 'low' THEN 1 ELSE 0 END AS sev, \
	    coalesce(a.packages, '{}') AS packages, \
	    coalesce(a.machines, 0) AS machines, \
	    coalesce(p.scopes, '{}') || CASE WHEN coalesce(p.any_optional, false) \
	      THEN ARRAY['optional']::text[] ELSE ARRAY[]::text[] END AS scopes, \
	    coalesce(p.workspace_packages, '{}') AS workspace_packages, \
	    a.vulnerability_id IS NULL AS closed, \
	    CASE WHEN s.decision_id IS NULL OR s.withdrawn OR s.lapsed THEN 'open' \
	         WHEN s.outcomes > 1 THEN 'mixed' ELSE s.outcome END AS status, \
	    coalesce(s.withdrawn, false) AS withdrawn, \
	    coalesce(s.edited, false) AS edited, \
	    EXISTS (SELECT 1 FROM vulnerability_suggestions sg \
	      WHERE sg.vulnerability_id = i.vulnerability_id AND sg.source = i.source) AS has_suggestion \
	  FROM vulnerability_items i \
	  JOIN vulnerabilities v ON v.id = i.vulnerability_id \
	  LEFT JOIN agg a ON a.vulnerability_id = i.vulnerability_id AND a.source = i.source \
	  LEFT JOIN paths p ON p.vulnerability_id = i.vulnerability_id AND p.source = i.source \
	  LEFT JOIN state s ON s.vulnerability_id = i.vulnerability_id AND s.source = i.source)";

/// The conditions of a [`ListQuery`], over `base`, bound from `$3` (and the identifiers of `$1`).
const FILTERS: &str = "($1::text[] IS NULL OR base.id = ANY($1)) \
	AND ($3::text IS NULL OR base.source = $3) \
	AND ($4::text IS NULL OR base.status = $4) \
	AND ($5::text IS NULL OR $5 = ANY(base.scopes)) \
	AND ($6::int IS NULL OR base.sev >= $6) \
	AND ($7::int IS NULL OR base.sev = $7) \
	AND ($8::bool IS NULL OR base.has_suggestion = $8) \
	AND ($9::bool OR NOT base.closed) \
	AND ($10::text IS NULL OR $10 = ANY(base.packages)) \
	AND ($11::text IS NULL OR $11 = ANY(base.workspace_packages)) \
	AND ($12::text IS NULL OR $12 <% base.search_text OR $12 <% base.details)";

/// How closely an item matches the search, the main fields counting in full
/// and the advisory's details for half.
const RANK: &str = "CASE WHEN $12::text IS NULL THEN 0::float8 \
	ELSE word_similarity($12, base.search_text) + 0.5 * word_similarity($12, base.details) END";

/// The word similarity a search must reach to match.
const SEARCH_THRESHOLD: &str = "0.4";

fn view(tail: &str) -> String {
	format!("WITH {EXPOSURE_CTE}, {ITEM_CTES} {tail}")
}

fn production() -> String {
	ServerRank::Production.to_string()
}

/// Bind a query's parameters, `$1` onward, for the SQL of [`view`] over
/// [`FILTERS`].
macro_rules! bind_list {
	($sql:expr, $ids:expr, $query:expr) => {{
		let query: &ListQuery = $query;
		diesel::sql_query($sql)
			.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>($ids)
			.bind::<sql_types::Text, _>(production())
			.bind::<sql_types::Nullable<sql_types::Text>, _>(
				query.source.as_ref().map(Source::as_str),
			)
			.bind::<sql_types::Nullable<sql_types::Text>, _>(query.status.map(Status::as_str))
			.bind::<sql_types::Nullable<sql_types::Text>, _>(query.scope.as_deref())
			.bind::<sql_types::Nullable<sql_types::Integer>, _>(
				query.min_severity.map(|s| s as i32),
			)
			.bind::<sql_types::Nullable<sql_types::Integer>, _>(query.severity.map(|s| s as i32))
			.bind::<sql_types::Nullable<sql_types::Bool>, _>(query.has_suggestion)
			.bind::<sql_types::Bool, _>(query.include_closed)
			.bind::<sql_types::Nullable<sql_types::Text>, _>(query.package.as_deref())
			.bind::<sql_types::Nullable<sql_types::Text>, _>(query.workspace_package.as_deref())
			.bind::<sql_types::Nullable<sql_types::Text>, _>(query.search_term())
	}};
}

// ── Queries ─────────────────────────────────────────────────────────────────

/// A column the list sorts by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilitySortColumn)]
pub enum SortColumn {
	Severity,
	Summary,
	Package,
	Source,
	Scope,
	Status,
	Published,
	Machines,
	FirstSeen,
}

impl SortColumn {
	fn expression(self) -> &'static str {
		match self {
			Self::Severity => "base.sev",
			Self::Summary => "lower(base.summary)",
			Self::Package => "array_to_string(base.packages, ',')",
			Self::Source => "base.source",
			Self::Scope => "array_to_string(base.scopes, ',')",
			Self::Status => "base.status",
			Self::Published => "base.published_at",
			Self::Machines => "base.machines",
			Self::FirstSeen => "base.first_seen_at",
		}
	}
}

/// A sort the caller chose.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilitySort)]
pub struct Sort {
	pub column: SortColumn,
	/// Highest first. Defaults to ascending.
	#[serde(default)]
	pub descending: bool,
}

/// Which items to list, how to order them, and which page.
#[derive(Clone, Debug, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityListQuery)]
pub struct ListQuery {
	/// Only items of this source.
	#[serde(default)]
	pub source: Option<Source>,
	/// Only items with this status. Omitted, every status.
	#[serde(default)]
	pub status: Option<Status>,
	/// Only items reached through a path of this scope: a scope, or `optional`.
	#[serde(default)]
	pub scope: Option<String>,
	/// Only items at or above this severity.
	#[serde(default)]
	pub min_severity: Option<Severity>,
	/// Only items of exactly this severity.
	#[serde(default)]
	pub severity: Option<Severity>,
	/// Only items that have a suggestion (true) or none (false).
	#[serde(default)]
	pub has_suggestion: Option<bool>,
	/// Include closed items.
	#[serde(default)]
	pub include_closed: bool,
	/// Only items affecting a package of this name.
	#[serde(default)]
	pub package: Option<String>,
	/// Only items a workspace package of this name reaches.
	#[serde(default)]
	pub workspace_package: Option<String>,
	/// Partial words, tolerating small typos, over the identifier and aliases,
	/// summary, package names, workspace packages, source, and the advisory's
	/// details.
	#[serde(default)]
	pub search: Option<String>,
	/// The order. Without one, the order is by severity, then publication date,
	/// or by how well an item matches the search.
	#[serde(default)]
	pub sort: Option<Sort>,
	/// Where the last page ended, as that page gave it.
	#[serde(default)]
	pub cursor: Option<String>,
	/// How many items a page holds. Defaults to 50, at most 200.
	#[serde(default)]
	pub page_size: Option<u32>,
}

pub const DEFAULT_PAGE: u32 = 50;
pub const MAX_PAGE: u32 = 200;

impl ListQuery {
	fn search_term(&self) -> Option<&str> {
		self.search
			.as_deref()
			.map(str::trim)
			.filter(|term| !term.is_empty())
	}

	fn page_size(&self) -> i64 {
		i64::from(self.page_size.unwrap_or(DEFAULT_PAGE).clamp(1, MAX_PAGE))
	}

	fn offset(&self) -> Result<i64> {
		match self.cursor.as_deref() {
			None => Ok(0),
			Some(cursor) => cursor
				.strip_prefix('o')
				.and_then(|offset| offset.parse::<i64>().ok())
				.filter(|offset| *offset >= 0)
				.ok_or_else(|| AppError::BadRequest("the cursor is not one a page gave".into())),
		}
	}

	fn order_by(&self) -> String {
		const TIE: &str = "base.sev DESC, base.published_at DESC NULLS LAST, base.id, base.source";
		match (self.sort, self.search_term()) {
			(Some(sort), _) => format!(
				"{} {} NULLS LAST, {TIE}",
				sort.column.expression(),
				if sort.descending { "DESC" } else { "ASC" }
			),
			(None, Some(_)) => format!("rank DESC, {TIE}"),
			(None, None) => TIE.to_owned(),
		}
	}
}

/// A page of items.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityListing)]
pub struct Listing {
	/// The items, in order.
	pub items: Vec<ItemSummary>,
	/// How many items match in all.
	pub total: i64,
	/// The cursor of the next page, absent on the last.
	pub next_cursor: Option<String>,
}

/// How many items a choice of a filter would give.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityFacetCount)]
pub struct FacetCount {
	pub value: String,
	pub count: i64,
}

/// The choices of each filter, with how many items each would give with every
/// other filter, the closed switch and the search applied.
#[derive(Clone, Debug, Default, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityFacets)]
pub struct Facets {
	pub source: Vec<FacetCount>,
	pub status: Vec<FacetCount>,
	pub scope: Vec<FacetCount>,
	pub severity: Vec<FacetCount>,
	/// `yes` and `no`: whether the item has a suggestion.
	pub suggestion: Vec<FacetCount>,
}

/// What a result is grouped by.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
#[schema(as = VulnerabilityGroupBy)]
pub enum GroupBy {
	Package,
	WorkspacePackage,
	Scope,
	Source,
	Status,
}

impl GroupBy {
	pub fn parse(s: &str) -> Option<Self> {
		match s {
			"package" => Some(Self::Package),
			"workspace_package" => Some(Self::WorkspacePackage),
			"scope" => Some(Self::Scope),
			"source" => Some(Self::Source),
			"status" => Some(Self::Status),
			_ => None,
		}
	}

	fn from_clause(self) -> &'static str {
		match self {
			Self::Package => "base, unnest(base.packages) AS grp",
			Self::WorkspacePackage => "base, unnest(base.workspace_packages) AS grp",
			Self::Scope => "base, unnest(base.scopes) AS grp",
			Self::Source => "base, LATERAL (SELECT base.source) AS g(grp)",
			Self::Status => "base, LATERAL (SELECT base.status) AS g(grp)",
		}
	}
}

/// A group of a result and how many items fall in it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityGroup)]
pub struct Group {
	pub key: String,
	pub count: i64,
}

#[derive(diesel::QueryableByName)]
struct CountRow {
	#[diesel(sql_type = sql_types::Text)]
	value: String,
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

/// Run a body with the search threshold set for its transaction only.
macro_rules! searching {
	($conn:ident, $body:block) => {
		$conn
			.transaction::<_, AppError, _>(async |$conn| {
				diesel::sql_query(format!(
					"SELECT set_config('pg_trgm.word_similarity_threshold', '{SEARCH_THRESHOLD}', true)"
				))
				.execute($conn)
				.await?;
				$body
			})
			.await
	};
}

// ── Summaries ───────────────────────────────────────────────────────────────

/// An affected package of an item, as its source carries it.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityAffectedPackage)]
pub struct AffectedPackage {
	pub name: String,
	/// The version carried.
	pub version: String,
	/// The lowest version above it that fixes the flaw, where the advisory names
	/// one.
	pub min_fix_version: Option<String>,
	/// The scopes of the paths that reach it, with `optional` where one is
	/// optional. Empty for Linux.
	pub scopes: Vec<String>,
}

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
	/// When the advisory was published, where OSV says.
	pub published_at: Option<Timestamp>,
	/// The affected packages the source carries.
	pub packages: Vec<AffectedPackage>,
	/// Every scope the item is reached through, with `optional`.
	pub scopes: Vec<String>,
	/// The workspace packages that reach it.
	pub workspace_packages: Vec<String>,
	/// For a Linux item, how many machines it exposes.
	pub machines: i64,
	/// Whether nothing counts for it any more.
	pub closed: bool,
	/// Where its triage stands.
	pub status: Status,
	/// The decision that stands, absent when the item needs triage.
	pub decision: Option<Decision>,
	/// Whether the item's latest decision was withdrawn.
	pub withdrawn: bool,
	/// Whether the decision that stands was edited.
	pub edited: bool,
	/// Why the item's decision lapsed, when its latest one did.
	pub lapse: Option<Lapse>,
	/// The model's latest suggestion, if it has made one.
	pub suggestion: Option<Suggestion>,
	/// When a scan first found it.
	pub first_seen_at: Timestamp,
}

#[derive(diesel::QueryableByName)]
struct ItemRow {
	#[diesel(sql_type = sql_types::Text)]
	id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Timestamptz, deserialize_as = jiff_diesel::Timestamp)]
	first_seen_at: Timestamp,
	#[diesel(sql_type = sql_types::Text)]
	summary: String,
	#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
	aliases: Vec<Option<String>>,
	#[diesel(sql_type = sql_types::Text)]
	severity: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>, deserialize_as = jiff_diesel::NullableTimestamp)]
	published_at: Option<Timestamp>,
	#[diesel(sql_type = sql_types::BigInt)]
	machines: i64,
	#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
	scopes: Vec<Option<String>>,
	#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
	workspace_packages: Vec<Option<String>>,
	#[diesel(sql_type = sql_types::Bool)]
	closed: bool,
	#[diesel(sql_type = sql_types::Text)]
	status: String,
	#[diesel(sql_type = sql_types::Bool)]
	withdrawn: bool,
	#[diesel(sql_type = sql_types::Bool)]
	edited: bool,
}

#[derive(diesel::QueryableByName)]
struct PackageRow {
	#[diesel(sql_type = sql_types::Text)]
	vulnerability_id: String,
	#[diesel(sql_type = sql_types::Text)]
	source: String,
	#[diesel(sql_type = sql_types::Text)]
	ecosystem: String,
	#[diesel(sql_type = sql_types::Text)]
	name: String,
	#[diesel(sql_type = sql_types::Text)]
	version: String,
	#[diesel(sql_type = sql_types::Array<sql_types::Nullable<sql_types::Text>>)]
	scopes: Vec<Option<String>>,
	#[diesel(sql_type = sql_types::Bool)]
	optional: bool,
}

#[derive(diesel::QueryableByName)]
struct AffectedRow {
	#[diesel(sql_type = sql_types::Text)]
	id: String,
	#[diesel(sql_type = sql_types::Jsonb)]
	affected: serde_json::Value,
}

impl ItemSummary {
	/// A page of the items matching `query`, in order.
	pub async fn list(conn: &mut AsyncPgConnection, query: &ListQuery) -> Result<Listing> {
		Self::list_among(conn, None, query).await
	}

	async fn list_among(
		conn: &mut AsyncPgConnection,
		ids: Option<Vec<String>>,
		query: &ListQuery,
	) -> Result<Listing> {
		let offset = query.offset()?;
		let size = query.page_size();
		searching!(conn, {
			let total: Vec<CountRow> = bind_list!(
				view(&format!(
					"SELECT 'all' AS value, count(*) AS count FROM base WHERE {FILTERS}"
				)),
				ids.clone(),
				query
			)
			.load(conn)
			.await?;
			let total = total.iter().next().map_or(0, |row| row.count);

			let rows: Vec<ItemRow> = bind_list!(
				view(&format!(
					"SELECT base.id, base.source, base.first_seen_at, base.summary, base.aliases, \
					        base.severity, base.published_at, base.machines, base.scopes, \
					        base.workspace_packages, base.closed, base.status, base.withdrawn, \
					        base.edited, {RANK} AS rank \
					 FROM base WHERE {FILTERS} ORDER BY {} LIMIT {size} OFFSET {offset}",
					query.order_by()
				)),
				ids.clone(),
				query
			)
			.load(conn)
			.await?;
			let next_cursor = (offset + (rows.len() as i64) < total && !rows.is_empty())
				.then(|| format!("o{}", offset + rows.len() as i64));
			let items = Self::hydrate(conn, rows).await?;
			Ok(Listing {
				items,
				total,
				next_cursor,
			})
		})
	}

	/// One item, closed or not.
	pub async fn one(
		conn: &mut AsyncPgConnection,
		id: &str,
		source: &Source,
	) -> Result<Option<Self>> {
		let query = ListQuery {
			source: Some(source.clone()),
			include_closed: true,
			page_size: Some(1),
			..Default::default()
		};
		Ok(Self::list_among(conn, Some(vec![id.to_owned()]), &query)
			.await?
			.items
			.into_iter()
			.next())
	}

	async fn hydrate(conn: &mut AsyncPgConnection, rows: Vec<ItemRow>) -> Result<Vec<Self>> {
		let keys: Vec<(String, String)> = rows
			.iter()
			.map(|row| (row.id.clone(), row.source.clone()))
			.collect();
		if keys.is_empty() {
			return Ok(Vec::new());
		}
		let ids: Vec<String> = keys.iter().map(|(id, _)| id.clone()).collect();
		let sources: Vec<&str> = keys.iter().map(|(_, source)| source.as_str()).collect();

		let affected: HashMap<String, Vec<osv::Affected>> = {
			let rows: Vec<AffectedRow> =
				diesel::sql_query("SELECT id, affected FROM vulnerabilities WHERE id = ANY($1)")
					.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
					.load(conn)
					.await?;
			rows.into_iter()
				.map(|row| {
					(
						row.id,
						serde_json::from_value(row.affected).unwrap_or_default(),
					)
				})
				.collect()
		};
		let package_rows: Vec<PackageRow> = diesel::sql_query(format!(
			"WITH {EXPOSURE_CTE} \
			 SELECT e.vulnerability_id, e.source, e.ecosystem, e.name, e.version, \
			   coalesce(array_agg(DISTINCT p.scope) FILTER (WHERE p.scope IS NOT NULL), '{{}}') AS scopes, \
			   coalesce(bool_or(p.optional), false) AS optional \
			 FROM exposure e LEFT JOIN repository_package_paths p \
			   ON p.repository = e.source AND p.ecosystem = e.ecosystem \
			  AND p.name = e.name AND p.version = e.version \
			 WHERE (e.vulnerability_id, e.source) IN (SELECT * FROM unnest($3::text[], $4::text[])) \
			 GROUP BY 1, 2, 3, 4, 5 ORDER BY 1, 2, 4, 5"
		))
		.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(Some(&ids))
		.bind::<sql_types::Text, _>(production())
		.bind::<sql_types::Array<sql_types::Text>, _>(&ids)
		.bind::<sql_types::Array<sql_types::Text>, _>(&sources)
		.load(conn)
		.await?;
		let mut packages: HashMap<(String, String), Vec<AffectedPackage>> = HashMap::new();
		for row in package_rows {
			let mut scopes = strings(row.scopes);
			if row.optional {
				scopes.push("optional".into());
			}
			let min_fix_version = affected.get(&row.vulnerability_id).and_then(|affected| {
				osv::min_fix_version(affected, &row.ecosystem, &row.name, &row.version)
			});
			packages
				.entry((row.vulnerability_id, row.source))
				.or_default()
				.push(AffectedPackage {
					name: row.name,
					version: row.version,
					min_fix_version,
					scopes,
				});
		}

		let mut decisions = Decision::newest_of(conn, &keys).await?;
		let mut lapses = Lapse::of_newest(conn, &keys).await?;
		let mut suggestions = Suggestion::for_items(conn, &keys).await?;

		Ok(rows
			.into_iter()
			.filter_map(|row| {
				let source = Source::parse(&row.source)?;
				let key = (row.id.clone(), source.clone());
				let status = Status::parse(&row.status).unwrap_or(Status::Open);
				let newest = decisions.remove(&key);
				let lapse = lapses.remove(&key);
				let decision = newest.filter(|d| d.withdrawal.is_none() && lapse.is_none());
				let suggestion = suggestions.remove(&key);
				Some(Self {
					packages: packages
						.remove(&(row.id.clone(), row.source.clone()))
						.unwrap_or_default(),
					id: row.id,
					source,
					aliases: strings(row.aliases),
					summary: row.summary,
					severity: row.severity.parse().unwrap_or_default(),
					published_at: row.published_at,
					scopes: strings(row.scopes),
					workspace_packages: strings(row.workspace_packages),
					machines: row.machines,
					closed: row.closed,
					status,
					edited: row.edited && decision.is_some(),
					decision,
					withdrawn: row.withdrawn,
					lapse,
					suggestion,
					first_seen_at: row.first_seen_at,
				})
			})
			.collect())
	}

	/// How many items need triage, closed ones left out.
	pub async fn needs_triage_count(conn: &mut AsyncPgConnection) -> Result<i64> {
		let query = ListQuery {
			status: Some(Status::Open),
			..Default::default()
		};
		let rows: Vec<CountRow> = bind_list!(
			view(&format!(
				"SELECT 'all' AS value, count(*) AS count FROM base WHERE {FILTERS}"
			)),
			None::<Vec<String>>,
			&query
		)
		.load(conn)
		.await?;
		Ok(rows.iter().next().map_or(0, |row| row.count))
	}

	/// How many items each choice of each filter would give, with every other
	/// filter, the closed switch and the search applied but not its own.
	pub async fn facets(conn: &mut AsyncPgConnection, query: &ListQuery) -> Result<Facets> {
		searching!(conn, {
			let count = async |conn: &mut AsyncPgConnection,
			                   query: &ListQuery,
			                   from: &str,
			                   value: &str|
			       -> Result<HashMap<String, i64>> {
				let rows: Vec<CountRow> = bind_list!(
					view(&format!(
						"SELECT {value} AS value, count(*) AS count FROM {from} \
						 WHERE {FILTERS} GROUP BY 1"
					)),
					None::<Vec<String>>,
					query
				)
				.load(conn)
				.await?;
				Ok(rows.into_iter().map(|row| (row.value, row.count)).collect())
			};
			let choices =
				|mut counts: HashMap<String, i64>, values: Vec<String>| -> Vec<FacetCount> {
					values
						.into_iter()
						.map(|value| FacetCount {
							count: counts.remove(&value).unwrap_or(0),
							value,
						})
						.collect()
				};

			let without = |edit: &dyn Fn(&mut ListQuery)| {
				let mut query = query.clone();
				edit(&mut query);
				query
			};

			let source = count(conn, &without(&|q| q.source = None), "base", "base.source").await?;
			let status = count(conn, &without(&|q| q.status = None), "base", "base.status").await?;
			let scope = count(
				conn,
				&without(&|q| q.scope = None),
				"base, unnest(base.scopes) AS scope",
				"scope",
			)
			.await?;
			let severity = count(
				conn,
				&without(&|q| {
					q.severity = None;
					q.min_severity = None;
				}),
				"base",
				"base.severity",
			)
			.await?;
			let suggestion = count(
				conn,
				&without(&|q| q.has_suggestion = None),
				"base",
				"CASE WHEN base.has_suggestion THEN 'yes' ELSE 'no' END",
			)
			.await?;

			let sources = REPOSITORIES
				.iter()
				.map(|repository| repository.name.to_owned())
				.chain([LINUX.to_owned()])
				.collect();
			Ok(Facets {
				source: choices(source, sources),
				status: choices(
					status,
					[
						Status::Open,
						Status::NotAffected,
						Status::AcceptedRisk,
						Status::WillFix,
						Status::Mixed,
					]
					.iter()
					.map(|s| s.as_str().to_owned())
					.collect(),
				),
				scope: choices(
					scope,
					Scope::ALL
						.iter()
						.map(|s| s.as_str().to_owned())
						.chain(["optional".to_owned()])
						.collect(),
				),
				severity: choices(
					severity,
					["critical", "high", "medium", "low", "unrated"]
						.map(String::from)
						.to_vec(),
				),
				suggestion: choices(suggestion, vec!["yes".into(), "no".into()]),
			})
		})
	}

	/// The result of `query` grouped by `by`, each group with how many items
	/// fall in it, largest first. An item in several groups counts in each.
	pub async fn group(
		conn: &mut AsyncPgConnection,
		query: &ListQuery,
		by: GroupBy,
	) -> Result<Vec<Group>> {
		searching!(conn, {
			let rows: Vec<CountRow> = bind_list!(
				view(&format!(
					"SELECT grp AS value, count(*) AS count FROM {} \
					 WHERE {FILTERS} GROUP BY 1 ORDER BY 2 DESC, 1",
					by.from_clause()
				)),
				None::<Vec<String>>,
				query
			)
			.load(conn)
			.await?;
			Ok(rows
				.into_iter()
				.map(|row| Group {
					key: row.value,
					count: row.count,
				})
				.collect())
		})
	}
}

// ── Detail ──────────────────────────────────────────────────────────────────

/// An affected package of a repository item, with every way it is reached.
#[derive(Clone, Debug, Serialize, Deserialize, utoipa::ToSchema)]
#[schema(as = VulnerabilityPackageDetail)]
pub struct PackageDetail {
	pub name: String,
	pub version: String,
	/// The lowest version above it that fixes the flaw.
	pub min_fix_version: Option<String>,
	/// Every version of it that fixes the flaw.
	pub fixed_in: Vec<String>,
	/// How workspace packages reach it.
	pub paths: Vec<PackagePath>,
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
	/// The lowest version above it that fixes the flaw.
	pub min_fix_version: Option<String>,
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
	Edit {
		at: Timestamp,
		edit: Edit,
	},
	Withdrawal {
		at: Timestamp,
		withdrawal: Withdrawal,
	},
	Lapse {
		at: Timestamp,
		lapse: Lapse,
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
			| Self::Edit { at, .. }
			| Self::Withdrawal { at, .. }
			| Self::Lapse { at, .. }
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
	/// For a repository item, the affected packages on the default branch.
	pub packages: Vec<PackageDetail>,
	/// For a repository item, the commit of the default branch they were read at.
	pub commit: Option<String>,
	/// For a Linux item, the exposed machines.
	pub machines: Vec<ExposedMachine>,
	/// Decisions and changes to them, lapses and the suggestion, newest first.
	pub history: Vec<HistoryEntry>,
}

#[derive(diesel::QueryableByName)]
struct PathRow {
	#[diesel(sql_type = sql_types::Text)]
	ecosystem: String,
	#[diesel(sql_type = sql_types::Text)]
	name: String,
	#[diesel(sql_type = sql_types::Text)]
	version: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	workspace_package: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	scope: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Bool>)]
	optional: Option<bool>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Bool>)]
	direct: Option<bool>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Array<sql_types::Nullable<sql_types::Text>>>)]
	chain: Option<Vec<Option<String>>>,
}

#[derive(diesel::QueryableByName)]
struct MachineRow {
	#[diesel(sql_type = sql_types::Uuid)]
	machine_id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	machine_name: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
	group_id: Option<Uuid>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	group_name: Option<String>,
	#[diesel(sql_type = sql_types::Text)]
	ecosystem: String,
	#[diesel(sql_type = sql_types::Text)]
	package: String,
	#[diesel(sql_type = sql_types::Text)]
	package_version: String,
}

impl ItemDetail {
	pub async fn load(conn: &mut AsyncPgConnection, id: &str, source: &Source) -> Result<Self> {
		let advisory = Advisory::get(conn, id)
			.await?
			.ok_or_else(|| AppError::NotFound(format!("no vulnerability {id}")))?;
		let item = ItemSummary::one(conn, id, source).await?.ok_or_else(|| {
			AppError::NotFound(format!("no {} vulnerability {id}", source.as_str()))
		})?;
		let record = Record {
			id: advisory.id.clone(),
			affected: advisory.affected.clone(),
			..Default::default()
		};

		let mut packages: Vec<PackageDetail> = Vec::new();
		let mut commit = None;
		let mut machines: Vec<ExposedMachine> = Vec::new();
		if source.is_linux() {
			let rows: Vec<MachineRow> = diesel::sql_query(format!(
				"WITH {EXPOSURE_CTE} \
				 SELECT mp.machine_id, ma.name AS machine_name, ma.group_id, g.name AS group_name, \
				        e.ecosystem, e.name AS package, e.version AS package_version \
				 FROM exposure e \
				 JOIN machine_packages mp ON mp.machine_id = (e.machine_id) \
				      AND mp.name = e.name AND mp.version = e.version \
				 JOIN machines ma ON ma.id = mp.machine_id \
				 LEFT JOIN server_groups g ON g.id = ma.group_id \
				 WHERE e.source = 'linux' ORDER BY ma.name, e.name"
			))
			.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(Some(vec![id]))
			.bind::<sql_types::Text, _>(production())
			.load(conn)
			.await?;
			machines = rows
				.into_iter()
				.map(|row| ExposedMachine {
					min_fix_version: osv::min_fix_version(
						&advisory.affected,
						&row.ecosystem,
						&row.package,
						&row.package_version,
					),
					machine_id: row.machine_id,
					machine_name: row.machine_name,
					group_id: row.group_id,
					group_name: row.group_name,
					package: row.package,
					package_version: row.package_version,
				})
				.collect();
		} else {
			let rows: Vec<PathRow> = diesel::sql_query(format!(
				"WITH {EXPOSURE_CTE} \
				 SELECT e.ecosystem, e.name, e.version, p.workspace_package, p.scope, p.optional, \
				        p.direct, p.chain \
				 FROM exposure e LEFT JOIN repository_package_paths p \
				   ON p.repository = e.source AND p.ecosystem = e.ecosystem \
				  AND p.name = e.name AND p.version = e.version \
				 WHERE e.source = $3 \
				 ORDER BY e.name, e.version, p.workspace_package, p.scope, p.optional"
			))
			.bind::<sql_types::Nullable<sql_types::Array<sql_types::Text>>, _>(Some(vec![id]))
			.bind::<sql_types::Text, _>(production())
			.bind::<sql_types::Text, _>(source.as_str())
			.load(conn)
			.await?;
			let mut by_package: BTreeMap<(String, String, String), Vec<PackagePath>> =
				BTreeMap::new();
			for row in rows {
				let paths = by_package
					.entry((row.name, row.version, row.ecosystem))
					.or_default();
				if let (Some(workspace_package), Some(scope)) = (row.workspace_package, row.scope)
					&& let Some(scope) = Scope::parse(&scope)
				{
					paths.push(PackagePath {
						workspace_package,
						scope,
						optional: row.optional.unwrap_or(false),
						direct: row.direct.unwrap_or(false),
						chain: row.chain.map(strings).unwrap_or_default(),
					});
				}
			}
			packages = by_package
				.into_iter()
				.map(|((name, version, ecosystem), paths)| PackageDetail {
					min_fix_version: osv::min_fix_version(
						&advisory.affected,
						&ecosystem,
						&name,
						&version,
					),
					fixed_in: record.fixed_in(&ecosystem, &name),
					name,
					version,
					paths,
				})
				.collect();
			commit = RepositoryRead::get(conn, source.as_str())
				.await?
				.and_then(|read| read.commit_sha);
		}

		let mut history: Vec<HistoryEntry> = Vec::new();
		history.extend(
			Decision::history(conn, id, source)
				.await?
				.into_iter()
				.map(|decision| HistoryEntry::Decision {
					at: decision.created_at,
					decision,
				}),
		);
		history.extend(
			Edit::history(conn, id, source)
				.await?
				.into_iter()
				.map(|edit| HistoryEntry::Edit {
					at: edit.created_at,
					edit,
				}),
		);
		history.extend(
			Withdrawal::history(conn, id, source)
				.await?
				.into_iter()
				.map(|withdrawal| HistoryEntry::Withdrawal {
					at: withdrawal.created_at,
					withdrawal,
				}),
		);
		history.extend(
			Lapse::history(conn, id, source)
				.await?
				.into_iter()
				.map(|lapse| HistoryEntry::Lapse {
					at: lapse.created_at,
					lapse,
				}),
		);
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
			packages,
			commit,
			machines,
			history,
		})
	}
}
