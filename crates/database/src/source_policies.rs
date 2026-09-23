//! Operator policy per reporting source: the reachability mode (how the
//! source's silence bears on its applications' reachability) and the ingest
//! mode (whether the device API accepts its reports). Absent rows mean the
//! defaults (`on`, `allow`).

use std::collections::HashMap;

use commons_errors::{AppError, Result};
use commons_types::{
	namespace::RESERVED_SOURCES,
	source::{IngestMode, ReachabilityMode},
};
use diesel::prelude::*;
use diesel::sql_types;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;

/// A reporting source with its policy and fleet-wide last-seen, for the
/// operator source list. Reserved sources are excluded.
#[derive(Clone, Debug)]
pub struct SourceInfo {
	pub source: String,
	pub reachability: ReachabilityMode,
	pub ingest: IngestMode,
	pub last_seen: Option<Timestamp>,
}

/// One source's policy row.
#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = crate::schema::source_policies)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct SourcePolicy {
	pub source: String,
	#[diesel(deserialize_as = String, serialize_as = String)]
	pub reachability: ReachabilityMode,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	#[diesel(deserialize_as = String, serialize_as = String)]
	pub ingest: IngestMode,
}

impl SourcePolicy {
	/// Every source policy row, ordered by source.
	pub async fn list(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::source_policies::dsl;
		dsl::source_policies
			.select(Self::as_select())
			.order(dsl::source.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every non-reserved reporting source (those with catalogued checks),
	/// with its reachability and ingest modes (defaulting to `on`/`allow`)
	/// and the most recent time any of its checks was seen fleet-wide.
	/// Ordered by source.
	pub async fn list_sources(db: &mut AsyncPgConnection) -> Result<Vec<SourceInfo>> {
		#[derive(QueryableByName)]
		struct Row {
			#[diesel(sql_type = sql_types::Text)]
			source: String,
			#[diesel(sql_type = sql_types::Text)]
			reachability: String,
			#[diesel(sql_type = sql_types::Text)]
			ingest: String,
			#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>)]
			last_seen: Option<jiff_diesel::Timestamp>,
		}
		let rows: Vec<Row> = diesel::sql_query(
			"SELECT cp.source, \
			 coalesce(sp.reachability, 'on') AS reachability, \
			 coalesce(sp.ingest, 'allow') AS ingest, \
			 max(cp.last_seen) AS last_seen \
			 FROM check_policies cp \
			 LEFT JOIN source_policies sp ON sp.source = cp.source \
			 WHERE NOT (lower(cp.source) = ANY($1)) \
			 GROUP BY cp.source, sp.reachability, sp.ingest \
			 ORDER BY cp.source",
		)
		.bind::<sql_types::Array<sql_types::Text>, _>(RESERVED_SOURCES.to_vec())
		.load(db)
		.await?;
		Ok(rows
			.into_iter()
			.map(|r| SourceInfo {
				source: r.source,
				reachability: r.reachability.parse().unwrap_or_default(),
				ingest: r.ingest.parse().unwrap_or_default(),
				last_seen: r.last_seen.map(Into::into),
			})
			.collect())
	}

	/// Each source's reachability mode. Sources without a row are absent
	/// here; callers default them to [`ReachabilityMode::On`].
	pub async fn modes(db: &mut AsyncPgConnection) -> Result<HashMap<String, ReachabilityMode>> {
		use crate::schema::source_policies::dsl;
		let rows: Vec<(String, String)> = dsl::source_policies
			.select((dsl::source, dsl::reachability))
			.load(db)
			.await?;
		Ok(rows
			.into_iter()
			.filter_map(|(source, mode)| mode.parse().ok().map(|m| (source, m)))
			.collect())
	}

	/// Each source's ingest mode. Sources without a row are absent here;
	/// callers default them to [`IngestMode::Allow`].
	pub async fn ingest_modes(db: &mut AsyncPgConnection) -> Result<HashMap<String, IngestMode>> {
		use crate::schema::source_policies::dsl;
		let rows: Vec<(String, String)> = dsl::source_policies
			.select((dsl::source, dsl::ingest))
			.load(db)
			.await?;
		Ok(rows
			.into_iter()
			.filter_map(|(source, mode)| mode.parse().ok().map(|m| (source, m)))
			.collect())
	}

	/// The ingest mode for one source (defaulting to `allow`). Used on the
	/// hot push path, so it's a single-row lookup rather than the full map.
	pub async fn ingest_for(db: &mut AsyncPgConnection, source: &str) -> Result<IngestMode> {
		use crate::schema::source_policies::dsl;
		let mode: Option<String> = dsl::source_policies
			.select(dsl::ingest)
			.filter(dsl::source.eq(source))
			.first(db)
			.await
			.optional()?;
		Ok(mode.and_then(|m| m.parse().ok()).unwrap_or_default())
	}

	/// Set a source's reachability mode, creating its policy row if needed.
	pub async fn set_reachability(
		db: &mut AsyncPgConnection,
		source: &str,
		mode: ReachabilityMode,
	) -> Result<()> {
		use crate::schema::source_policies::dsl;
		diesel::insert_into(dsl::source_policies)
			.values((
				dsl::source.eq(source),
				dsl::reachability.eq(mode.to_string()),
			))
			.on_conflict(dsl::source)
			.do_update()
			.set((
				dsl::reachability.eq(mode.to_string()),
				dsl::updated_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Set a source's ingest mode, creating its policy row if needed.
	pub async fn set_ingest(
		db: &mut AsyncPgConnection,
		source: &str,
		mode: IngestMode,
	) -> Result<()> {
		use crate::schema::source_policies::dsl;
		diesel::insert_into(dsl::source_policies)
			.values((dsl::source.eq(source), dsl::ingest.eq(mode.to_string())))
			.on_conflict(dsl::source)
			.do_update()
			.set((
				dsl::ingest.eq(mode.to_string()),
				dsl::updated_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.execute(db)
			.await?;
		Ok(())
	}
}
