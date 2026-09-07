//! Each source's current server-wide detail, as a table rather than a
//! search through status history.
//!
//! The same facts are in `statuses.extra`, but that table is partitioned by
//! week and a predicate on `server_id` alone can't be pruned, so resolving
//! one server's figures means a bounded scan over recent partitions —
//! affordable once per page, not once per server on a fleet-wide view. This
//! is the current-state projection: ingest keeps it fresh, and every read
//! that only wants "what is this server running now" stops here.
//!
//! Status history stays the record of what was reported *when*; this is the
//! record of what stands.

use commons_errors::{AppError, Result};
use commons_types::{
	server::{app_type::ApplicationType, rank::ServerRank},
	version::VersionStr,
};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use uuid::Uuid;

use crate::statuses::MergedDetail;

/// How recently a server must have reported to count as still running what it
/// last reported.
///
/// Most reads here deliberately have no such bound — a figure is what the
/// server runs, and that doesn't stop being true because the server went
/// quiet. But "what is the fleet *actively* running" is a different question:
/// a decommissioned server that was never archived would otherwise keep its
/// release branch in the count forever.
const ACTIVE_LOOKBACK_SQL: &str = "NOW() - INTERVAL '7 days'";

/// One source's latest server-wide detail for one server.
// spec: FIG#sourcing
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::application_reported_detail)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ReportedDetail {
	/// The application this detail describes.
	pub application_id: Uuid,
	/// The source that reported it.
	pub source: String,
	/// That source's whole server-wide detail as last pushed.
	pub extra: serde_json::Value,
	/// The application version that push reported, if it reported one.
	pub version: Option<VersionStr>,
	/// When the push carrying this detail landed.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub reported_at: Timestamp,
	/// When this source last carried a version. A push without one leaves it,
	/// so the version is dated by when it was said rather than by when the
	/// source last spoke.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub version_reported_at: Option<Timestamp>,
}

impl ReportedDetail {
	/// Record one source's push, splitting it by grain: the box's fields to
	/// the machine, the rest to the application.
	///
	/// A host running two workloads reports its platform, memory and
	/// filesystems once rather than once per workload. Reads are unaffected:
	/// [`Self::for_server`] merges the two back together.
	///
	/// Either side replaces what `source` reported before rather than merging
	/// with it, a push being that source's whole truth. Other sources' rows
	/// are untouched.
	///
	/// The version is the exception: a push that carries none keeps the last
	/// one this source reported. An agent omits the version when it can't
	/// read it — the application is down, or mid-upgrade — which says nothing
	/// about what the application is installed to run, and blanking on it
	/// would make a group's headline version flicker off exactly when an
	/// operator is looking at it.
	///
	/// `server` is `None` for a push from a box Canopy holds no application
	/// for. There is no second grain to split into, so the whole push is the
	/// machine's rather than half of it being dropped.
	// spec: FIG
	pub async fn record(
		db: &mut AsyncPgConnection,
		server: Option<Uuid>,
		machine: Uuid,
		source: &str,
		extra: &serde_json::Value,
		version: Option<&VersionStr>,
	) -> Result<()> {
		let Some(server) = server else {
			return MachineReportedDetail::record(db, machine, source, extra).await;
		};
		let (machine_extra, application_extra) = split_by_grain(extra);
		MachineReportedDetail::record(db, machine, source, &machine_extra).await?;
		Self::record_for_application(db, server, source, &application_extra, version).await
	}

	/// Record one source's detail for one application, the reporter having
	/// separated the grains itself.
	///
	/// No split to do: a split-shape push says which grain each field belongs
	/// to, so what arrives here is the application's and the machine's went to
	/// [`MachineReportedDetail::record`] directly. The replace and version
	/// rules are [`Self::record`]'s.
	// spec: FIG#sourcing
	pub async fn record_for_application(
		db: &mut AsyncPgConnection,
		server: Uuid,
		source: &str,
		extra: &serde_json::Value,
		version: Option<&VersionStr>,
	) -> Result<()> {
		use crate::schema::application_reported_detail::dsl;

		let extra = object_body(extra);
		let extra = extra.as_ref();
		let carried_at = version.map(|_| jiff_diesel::Timestamp::from(Timestamp::now()));

		diesel::insert_into(dsl::application_reported_detail)
			.values((
				dsl::application_id.eq(server),
				dsl::source.eq(source),
				dsl::extra.eq(extra),
				dsl::version.eq(version),
				dsl::reported_at.eq(diesel::dsl::now),
				dsl::version_reported_at.eq(carried_at),
			))
			.on_conflict((dsl::application_id, dsl::source))
			.do_update()
			.set((
				dsl::extra.eq(extra),
				// COALESCE over the excluded row: a version-less push keeps
				// the version this source last reported.
				dsl::version.eq(diesel::dsl::sql::<
					diesel::sql_types::Nullable<diesel::sql_types::Text>,
				>(
					"COALESCE(EXCLUDED.version, application_reported_detail.version)",
				)),
				dsl::reported_at.eq(diesel::dsl::now),
				dsl::version_reported_at.eq(diesel::dsl::sql::<
					diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
				>(
					"CASE WHEN EXCLUDED.version IS NULL THEN application_reported_detail.version_reported_at ELSE now() END",
				)),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Every source's current detail for one server.
	/// Every source's current detail for one application: its own, plus its
	/// machine's.
	///
	/// The two grains are stored apart and read together. A figure is a figure
	/// whichever grain reported it — an application presents its box's
	/// platform because that is what it runs on — so every consumer sees the
	/// same merged view it saw before the split.
	// spec: FIG
	pub async fn for_server(db: &mut AsyncPgConnection, server: Uuid) -> Result<Vec<Self>> {
		use crate::schema::application_reported_detail::dsl;

		let mut rows: Vec<Self> = dsl::application_reported_detail
			.select(Self::as_select())
			.filter(dsl::application_id.eq(server))
			.load(db)
			.await
			.map_err(AppError::from)?;

		// An application that no longer exists has no detail, rather than being
		// an error: its rows went with it, and so did its claim on a machine's.
		let machine_id = {
			use crate::schema::applications::dsl as a;
			a::applications
				.select(a::machine_id)
				.filter(a::id.eq(server))
				.first::<Uuid>(db)
				.await
				.optional()
				.map_err(AppError::from)?
		};
		if let Some(machine_id) = machine_id {
			rows.extend(
				MachineReportedDetail::for_machine(db, machine_id)
					.await?
					.into_iter()
					.map(|m| m.into_application_detail(server)),
			);
		}
		Ok(rows)
	}

	/// When each of `application_ids` last reported, whichever source it was
	/// and however long ago. Applications that have never reported are absent
	/// from the map.
	///
	/// This is what reachability is graded on, so a target quiet for months
	/// reads as unreachable rather than as never heard from. Answering it from
	/// status history needs a lookback cap (`statuses::GRACE_LOOKBACK_SQL`),
	/// which is what made a long silence indistinguishable from no report at
	/// all. Here there is one row per (application, source) and the read is
	/// driven by the primary key, so the question is affordable unbounded —
	/// the same read [`MachineReportedDetail::latest_for_machines`] does at the
	/// machine grain.
	// spec: CHK#reachability
	pub async fn last_reported_ats(
		db: &mut AsyncPgConnection,
		application_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Timestamp>> {
		use crate::schema::application_reported_detail::dsl;
		use std::collections::HashMap;

		if application_ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, jiff_diesel::Timestamp)> = dsl::application_reported_detail
			.select((dsl::application_id, dsl::reported_at))
			.filter(dsl::application_id.eq_any(application_ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		let mut latest: HashMap<Uuid, Timestamp> = HashMap::new();
		for (id, at) in rows {
			let at = Timestamp::from(at);
			latest
				.entry(id)
				.and_modify(|held| {
					if at > *held {
						*held = at;
					}
				})
				.or_insert(at);
		}
		Ok(latest)
	}

	/// When `application` last reported, however long ago — the single form of
	/// [`Self::last_reported_ats`], and unbounded for the same reason.
	// spec: CHK#reachability
	pub async fn last_reported_at(
		db: &mut AsyncPgConnection,
		application: Uuid,
	) -> Result<Option<Timestamp>> {
		use crate::schema::application_reported_detail::dsl;

		let at: Option<jiff_diesel::Timestamp> = dsl::application_reported_detail
			.select(dsl::reported_at)
			.filter(dsl::application_id.eq(application))
			.order(dsl::reported_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;
		Ok(at.map(Timestamp::from))
	}

	/// Every source's current detail for every application that has any, the
	/// application's own only. Small enough to read whole — one row per
	/// (application, source) across the fleet.
	///
	/// The fleet spread counts each grain separately, so the box's fields must
	/// not present as a workload's here; [`Self::all`] merges them in for a
	/// view that asks about one application at a time.
	// spec: FIG#fleet-spread
	pub async fn all_own(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::application_reported_detail::dsl;

		dsl::application_reported_detail
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The same, with each machine's detail presented on every application it
	/// hosts, so a caller reading one application at a time sees the box's
	/// fields alongside the workload's.
	pub async fn all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		let mut rows = Self::all_own(db).await?;

		// Each machine's detail presents on every application it hosts, the
		// same merge `for_server` does, one application at a time.
		let machines: Vec<(Uuid, Uuid)> = {
			use crate::schema::applications::dsl as a;
			a::applications
				.select((a::id, a::machine_id))
				.load(db)
				.await
				.map_err(AppError::from)?
		};
		let detail = MachineReportedDetail::all(db).await?;
		for (application_id, machine_id) in machines {
			rows.extend(
				detail
					.iter()
					.filter(|m| m.machine_id == machine_id)
					.map(|m| m.clone().into_application_detail(application_id)),
			);
		}
		Ok(rows)
	}

	/// The last application version `server` reported, from the most recent
	/// source to report one.
	///
	/// Unbounded by design: this answers "what was it running", which stays
	/// true however long the server has been down — a group's headline
	/// version shouldn't blank out because its canonical member went quiet.
	/// Reading the current-detail table is what makes that affordable; the
	/// same question against status history needed a lookback cap.
	// spec: FIG#sourcing
	pub async fn last_version(
		db: &mut AsyncPgConnection,
		server: Uuid,
	) -> Result<Option<VersionStr>> {
		use crate::schema::application_reported_detail::dsl;

		let version: Option<Option<VersionStr>> = dsl::application_reported_detail
			.select(dsl::version)
			.filter(dsl::application_id.eq(server))
			.filter(dsl::version.is_not_null())
			.order(version_time().desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?;

		Ok(version.flatten())
	}

	/// The last version each of `server_ids` reported, however long ago — the
	/// batch form of [`Self::last_version`], and unbounded for the same
	/// reason: this answers "what was it running", which stays true while a
	/// server is offline. Servers that have never reported a version are
	/// absent from the map.
	pub async fn last_versions(
		db: &mut AsyncPgConnection,
		server_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, VersionStr>> {
		use crate::schema::application_reported_detail::dsl;

		if server_ids.is_empty() {
			return Ok(std::collections::HashMap::new());
		}

		let rows: Vec<(Uuid, Option<VersionStr>)> = dsl::application_reported_detail
			.select((dsl::application_id, dsl::version))
			.filter(dsl::application_id.eq_any(server_ids))
			.filter(dsl::version.is_not_null())
			.distinct_on(dsl::application_id)
			.order((dsl::application_id, version_time().desc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter_map(|(id, version)| version.map(|v| (id, v)))
			.collect())
	}

	/// The application version each still-reporting production server runs,
	/// one per server.
	///
	/// A server's version is the one the most recent source to report a
	/// version gave: a source that reports none doesn't drop the server from
	/// the count just by having pushed last. Bounded by
	/// [`ACTIVE_LOOKBACK_SQL`], so this answers what is *running*, not what
	/// was last seen at any point in the past.
	// spec: FIG#active-versions
	pub async fn production_versions(db: &mut AsyncPgConnection) -> Result<Vec<VersionStr>> {
		use crate::schema::{application_reported_detail as detail, applications};

		let rows: Vec<Option<VersionStr>> = detail::table
			.inner_join(applications::table.on(applications::id.eq(detail::application_id)))
			.filter(applications::rank.eq(ServerRank::Production))
			// A release branch only means something for a type Canopy holds a
			// release train for. Others would each contribute a branch of
			// their own to a count of what the fleet is running.
			// spec: APP#capabilities
			.filter(
				applications::type_.eq_any(ApplicationType::stored_values_where(
					ApplicationType::tracks_versions,
				)),
			)
			.filter(applications::deleted_at.is_null())
			.filter(detail::version.is_not_null())
			.filter(detail::reported_at.ge(diesel::dsl::sql(ACTIVE_LOOKBACK_SQL)))
			.distinct_on(detail::application_id)
			.order((detail::application_id, detail::reported_at.desc()))
			.select(detail::version)
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows.into_iter().flatten().collect())
	}

	/// Resolve one server's figures from its sources' current reports.
	// spec: FIG#sourcing
	pub fn merge(reports: &[Self]) -> MergedDetail {
		MergedDetail::from_reports(reports.iter().map(|r| (r.reported_at, &r.extra)))
	}

	/// The same resolution for a whole fleet's worth of rows, keyed by
	/// server. Rows for applications the caller didn't ask about are ignored.
	pub fn merge_by_server(
		reports: Vec<Self>,
	) -> std::collections::HashMap<Uuid, (MergedDetail, Option<VersionStr>)> {
		let mut by_server: std::collections::HashMap<Uuid, Vec<Self>> =
			std::collections::HashMap::new();
		for report in reports {
			by_server
				.entry(report.application_id)
				.or_default()
				.push(report);
		}
		by_server
			.into_iter()
			.map(|(server, mut rows)| {
				// The newest report that carried a version wins it, on the
				// same rule as any other figure: a source that reports no
				// version doesn't erase one another source reported.
				rows.sort_by_key(|r| r.reported_at);
				let version = rows.iter().rev().find_map(|r| r.version.clone());
				(server, (Self::merge(&rows), version))
			})
			.collect()
	}
}

/// When a row's version was carried. Rows written before the column existed
/// date their version by the push that wrote them.
fn version_time() -> diesel::expression::SqlLiteral<diesel::sql_types::Timestamptz> {
	diesel::dsl::sql::<diesel::sql_types::Timestamptz>("COALESCE(version_reported_at, reported_at)")
}

/// Coerce a pushed body to an object, so that storing one cannot plant a value
/// its readers reject.
///
/// A body is `JSONB NOT NULL`, which does not make it an object: JSON `null` is
/// a value, so the column admits it. Anything that walks the column then breaks
/// on it — `jsonb_each` refuses a non-object, and so does a `-` key delete — and
/// that break surfaces wherever the body is next read in bulk rather than at the
/// push that stored it. A non-object body carries no fields, so the empty object
/// says the same thing and every reader can handle it.
///
/// Applied at each point a body is stored rather than at each entry point, so no
/// caller can route around it.
fn object_body(extra: &serde_json::Value) -> Cow<'_, serde_json::Value> {
	if extra.is_object() {
		Cow::Borrowed(extra)
	} else {
		Cow::Owned(serde_json::json!({}))
	}
}

/// Split a pushed detail body into the box's fields and the workload's.
fn split_by_grain(extra: &serde_json::Value) -> (serde_json::Value, serde_json::Value) {
	use commons_types::subject::CheckSubject;
	let Some(obj) = extra.as_object() else {
		// Not an object, so there are no fields to attribute to either grain.
		// Passing the body on whole would hand a scalar to the application's
		// row; `object_body` would flatten it there anyway, and an empty pair
		// says the same thing without relying on that.
		return (serde_json::json!({}), serde_json::json!({}));
	};
	let mut machine = serde_json::Map::new();
	let mut application = serde_json::Map::new();
	for (key, value) in obj {
		if CheckSubject::of_detail_field(key).is_machine() {
			machine.insert(key.clone(), value.clone());
		} else {
			application.insert(key.clone(), value.clone());
		}
	}
	(
		serde_json::Value::Object(machine),
		serde_json::Value::Object(application),
	)
}

/// One source's latest machine-wide detail for one machine: the facts that
/// describe the box rather than any workload on it.
// spec: FIG
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::machine_reported_detail)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct MachineReportedDetail {
	pub machine_id: Uuid,
	pub source: String,
	pub extra: serde_json::Value,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub reported_at: Timestamp,
}

impl MachineReportedDetail {
	/// Upsert this source's machine-wide detail. Whole-body replace, as the
	/// application side is: a source's latest push is what stands.
	///
	/// An empty body is still recorded, so a reporter that stops sending a
	/// field clears it rather than leaving a stale value behind.
	pub async fn record(
		db: &mut AsyncPgConnection,
		machine: Uuid,
		source: &str,
		extra: &serde_json::Value,
	) -> Result<()> {
		use crate::schema::machine_reported_detail::dsl;

		let extra = object_body(extra);
		let extra = extra.as_ref();

		diesel::insert_into(dsl::machine_reported_detail)
			.values((
				dsl::machine_id.eq(machine),
				dsl::source.eq(source),
				dsl::extra.eq(extra),
				dsl::reported_at.eq(diesel::dsl::now),
			))
			.on_conflict((dsl::machine_id, dsl::source))
			.do_update()
			.set((dsl::extra.eq(extra), dsl::reported_at.eq(diesel::dsl::now)))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// When each of these machines last reported anything, across every source.
	///
	/// A machine's reachability is judged from this, so a view showing many
	/// boxes asks once rather than once per box.
	// spec: CHK#reachability
	pub async fn latest_for_machines(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Timestamp>> {
		use crate::schema::machine_reported_detail::dsl;
		use std::collections::HashMap;
		if ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, jiff_diesel::Timestamp)> = dsl::machine_reported_detail
			.select((dsl::machine_id, dsl::reported_at))
			.filter(dsl::machine_id.eq_any(ids))
			.load(db)
			.await?;
		let mut latest: HashMap<Uuid, Timestamp> = HashMap::new();
		for (id, at) in rows {
			let at = Timestamp::from(at);
			latest
				.entry(id)
				.and_modify(|held| {
					if at > *held {
						*held = at;
					}
				})
				.or_insert(at);
		}
		Ok(latest)
	}

	pub async fn for_machine(db: &mut AsyncPgConnection, machine: Uuid) -> Result<Vec<Self>> {
		use crate::schema::machine_reported_detail::dsl;
		dsl::machine_reported_detail
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every source's current detail for each of these machines.
	///
	/// The plural of [`Self::for_machine`], for a view showing many boxes at
	/// once: it asks in one round trip and folds the rows per machine with
	/// [`Self::merge_by_machine`], rather than once per box.
	pub async fn for_machines(db: &mut AsyncPgConnection, ids: &[Uuid]) -> Result<Vec<Self>> {
		use crate::schema::machine_reported_detail::dsl;
		if ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::machine_reported_detail
			.select(Self::as_select())
			.filter(dsl::machine_id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::machine_reported_detail::dsl;
		dsl::machine_reported_detail
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every machine's current detail resolved across the sources reporting on
	/// it, keyed by machine. The box's counterpart of
	/// [`ReportedDetail::merge_by_server`], carrying no version: a version is
	/// the workload's.
	// spec: FIG#sourcing
	pub fn merge_by_machine(
		reports: Vec<Self>,
	) -> std::collections::HashMap<Uuid, crate::statuses::MergedDetail> {
		let mut by_machine: std::collections::HashMap<Uuid, Vec<Self>> =
			std::collections::HashMap::new();
		for report in reports {
			by_machine
				.entry(report.machine_id)
				.or_default()
				.push(report);
		}
		by_machine
			.into_iter()
			.map(|(machine, rows)| {
				(
					machine,
					crate::statuses::MergedDetail::from_reports(
						rows.iter().map(|r| (r.reported_at, &r.extra)),
					),
				)
			})
			.collect()
	}

	/// Present this machine's detail as one of `application`'s, so the merged
	/// figure view can treat both grains alike. Carries no version: a version
	/// is the workload's.
	fn into_application_detail(self, application: Uuid) -> ReportedDetail {
		ReportedDetail {
			application_id: application,
			source: self.source,
			extra: self.extra,
			version: None,
			reported_at: self.reported_at,
			version_reported_at: None,
		}
	}
}
