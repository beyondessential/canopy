use commons_errors::{AppError, Result};
use commons_types::{
	server::app_type::ApplicationType, source::ReachabilityMode, status::CheckResult,
	version::VersionStr,
};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use node_semver::Version;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::applications::Application;
use crate::issues::Issue;

/// Source value canopy uses when it files reachability issues on behalf of a
/// server. Combined with [`REACHABILITY_REF`] to dedupe / find-or-create.
///
/// Defined with the check namespace, since being curated by canopy is what
/// makes this source's names unqualified.
pub use commons_types::namespace::CANOPY_SOURCE;

/// Ref value canopy uses for the one reachability issue per server. Stable so
/// the find-or-create in [`NewEvent::save`] coalesces every cycle into the
/// same issue row.
pub const REACHABILITY_REF: &str = "reachability";

pub const REACHABILITY_DOC: &str = "## Description

Tracks whether the sources canopy expects to report on this server are actually reporting. A source going quiet degrades the server rather than silently dropping its checks, so a dead reporter is never mistaken for health.

## Results

- **pass** — every source canopy expects to report on this server is reporting within the server's down threshold.
- **warn** — a source in reachability mode `on` has gone quiet past the server's down threshold while others still report; the quiet sources are listed in the detail.
- **fail** — every expected source is stale, or the server has never reported: nothing is reaching canopy, and the server is unreachable.

## Solve

Check whether the server is down, its network/VPN path to canopy, and whether its reporting agents are running. A source that was deliberately retired should be set to `quiet` or `off` in the source list.";

/// How far back a "last value this server ever reported" read may look.
///
/// `statuses` is range-partitioned by week and prod already carries ~100
/// partitions with the better part of a million rows *per server*. A
/// predicate on `server_id` alone cannot be partition-pruned, so a query
/// with no lower bound on `created_at` degrades into a scan of every
/// partition — measured at 217s in production for a single row. Worse, the
/// pruning-free plan reads far more than `shared_buffers` holds, so it
/// evicts the buffer pool and drags every unrelated query down with it.
///
/// Every lookback here is therefore capped. The cost is that a server quiet
/// for longer than the window reads as "never reported"; that is preferable
/// to an unbounded scan, and the windows are generous next to how long a
/// server can be down before someone notices.
const GRACE_LOOKBACK_SQL: &str = "NOW() - INTERVAL '30 days'";

/// [`GRACE_LOOKBACK_SQL`] as a span, for bounding a lookback relative to a
/// caller-supplied point in time rather than to `NOW()`.
const GRACE_LOOKBACK: SignedDuration = SignedDuration::from_hours(24 * 30);

fn machine_label(m: &crate::machines::Machine) -> String {
	m.name.clone().unwrap_or_else(|| m.id.to_string())
}

/// The sources counting toward a set of targets' reachability, with how long
/// each has been silent: reporting, not switched off, and actually ingested.
/// An ignored or denied source has no fresh data to judge, so its silence says
/// nothing.
// spec: CHK#reachability
async fn expected_sources(
	db: &mut AsyncPgConnection,
	freshness: Vec<(Uuid, String, Timestamp)>,
) -> Result<std::collections::HashMap<Uuid, Vec<(String, SignedDuration, ReachabilityMode)>>> {
	use std::collections::HashMap;

	let modes = crate::source_policies::SourcePolicy::modes(db).await?;
	let ingest = crate::source_policies::SourcePolicy::ingest_modes(db).await?;
	let now = Timestamp::now();

	let mut by_target: HashMap<Uuid, Vec<(String, SignedDuration, ReachabilityMode)>> =
		HashMap::new();
	for (target, source, last_seen) in freshness {
		let mode = modes.get(&source).copied().unwrap_or_default();
		if mode == ReachabilityMode::Off
			|| ingest.get(&source).copied().unwrap_or_default()
				!= commons_types::source::IngestMode::Allow
		{
			continue;
		}
		let elapsed = now.duration_since(last_seen).abs();
		by_target
			.entry(target)
			.or_default()
			.push((source, elapsed, mode));
	}
	Ok(by_target)
}

/// Grade one target's reachability from its expected sources and its own
/// threshold. `grain` names what the target is, for the operator reading the
/// message.
// spec: CHK#reachability
fn grade_reachability(
	grain: &str,
	label: &str,
	threshold: SignedDuration,
	expected: &[(String, SignedDuration, ReachabilityMode)],
	last_reported: Option<Timestamp>,
	now: Timestamp,
) -> (CheckResult, String, serde_json::Value) {
	if expected.is_empty() {
		// No counted source: never reported, or every source excluded. Fall
		// back to whether anything at all has reached Canopy about it.
		let elapsed = last_reported.map(|at| now.duration_since(at).abs());
		let down = elapsed.map(|e| e >= threshold).unwrap_or(true);
		if !down {
			return (
				CheckResult::Passed,
				format!("{grain} {label} is reachable"),
				serde_json::json!({ "threshold_secs": threshold.as_secs() }),
			);
		}
		let message = match elapsed {
			Some(e) => format!(
				"{grain} {label} has not reported for {} (threshold {})",
				format_secs(e.as_secs()),
				format_secs(threshold.as_secs()),
			),
			None => format!(
				"{grain} {label} has never reported (threshold {})",
				format_secs(threshold.as_secs()),
			),
		};
		return (
			CheckResult::Failed,
			message,
			serde_json::json!({
				"elapsed_secs": elapsed.map(|e| e.as_secs()),
				"threshold_secs": threshold.as_secs(),
			}),
		);
	}

	let stale: Vec<&(String, SignedDuration, ReachabilityMode)> = expected
		.iter()
		.filter(|(_, e, _)| *e >= threshold)
		.collect();
	let stale_names = stale
		.iter()
		.map(|(s, _, _)| s.as_str())
		.collect::<Vec<_>>()
		.join(", ");
	let detail = serde_json::json!({
		"stale_sources": stale
			.iter()
			.map(|(source, e, _)| serde_json::json!({ "source": source, "stale_secs": e.as_secs() }))
			.collect::<Vec<_>>(),
		"threshold_secs": threshold.as_secs(),
	});

	if stale.len() == expected.len() {
		(
			CheckResult::Failed,
			format!("{grain} {label} is unreachable: every source is stale ({stale_names})"),
			detail,
		)
	} else if stale
		.iter()
		.any(|(_, _, mode)| *mode == ReachabilityMode::On)
	{
		(
			CheckResult::Warning,
			format!(
				"Source(s) on {} {label} have gone quiet: {stale_names}",
				grain.to_lowercase()
			),
			detail,
		)
	} else {
		// Some stale, but every stale source is quiet: no warning.
		(
			CheckResult::Passed,
			format!("{grain} {label} is reachable"),
			serde_json::json!({ "threshold_secs": threshold.as_secs() }),
		)
	}
}

/// Whether a grading is worth writing. A passing reachability with nothing
/// open to close would churn the check-state on every sweep.
fn worth_filing(observed: &CheckResult, open: Option<bool>) -> bool {
	*observed != CheckResult::Passed || open == Some(true)
}

fn format_secs(secs: i64) -> String {
	let s = secs.max(0);
	if s >= 86_400 {
		format!("{}d", s / 86_400)
	} else if s >= 3_600 {
		format!("{}h", s / 3_600)
	} else if s >= 60 {
		format!("{}m", s / 60)
	} else {
		format!("{s}s")
	}
}

/// A stored status report from a server: one heartbeat's worth of
/// self-reported health, version, and extra data.
#[derive(
	Debug,
	Clone,
	Serialize,
	Deserialize,
	Queryable,
	Selectable,
	Insertable,
	Associations,
	QueryableByName,
	utoipa::ToSchema,
)]
#[diesel(belongs_to(Application, foreign_key = server_id))]
#[diesel(table_name = crate::schema::statuses)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Status {
	/// Unique identifier of this status record.
	pub id: Uuid,
	/// When this status was received.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// The machine this status was reported for.
	///
	/// A push describes a box and the applications on it, so this is what it
	/// is recorded against. `None` on rows written before the split, whose
	/// machine is their application's.
	pub machine_id: Option<Uuid>,
	/// The application this status was reported for, where the push named
	/// one.
	///
	/// `None` for a push from a box that runs no application Canopy holds:
	/// the whole report is the box's, and there is nothing to attribute an
	/// application's half of it to.
	pub server_id: Option<Uuid>,
	/// The device that submitted this status, or `null` for a status not
	/// attributable to one.
	pub device_id: Option<Uuid>,
	/// The software version the server reported, if it reported one.
	pub version: Option<VersionStr>,
	/// Free-form extra data from the report (uptime, database version,
	/// timezone, etc.), stored verbatim as a JSON object.
	pub extra: serde_json::Value,
	/// Application's overall self-reported health. A report that omits this is
	/// recorded as healthy.
	pub healthy: bool,
	/// Per-check breakdown, as an array of objects each with at least a
	/// `check` name and a result; any extra per-check fields are passed
	/// through verbatim.
	pub health: serde_json::Value,
	/// The source that pushed this status: the named reporter (e.g.
	/// `alertd`). Rows retained from before canopy stopped generating
	/// status rows of its own also carry `canopy`.
	pub source: String,
}

#[derive(Debug, Insertable)]
#[diesel(belongs_to(Application, foreign_key = server_id))]
#[diesel(table_name = crate::schema::statuses)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewStatus {
	pub machine_id: Uuid,
	pub server_id: Option<Uuid>,
	pub device_id: Option<Uuid>,
	pub version: Option<VersionStr>,
	pub extra: serde_json::Value,
	pub healthy: bool,
	pub health: serde_json::Value,
	pub source: String,
}

impl NewStatus {
	pub async fn save(self, db: &mut AsyncPgConnection) -> Result<Status> {
		diesel::insert_into(crate::schema::statuses::table)
			.values(self)
			.returning(Status::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}
}

impl Status {
	pub fn extra(&self, key: &str) -> Option<&serde_json::Value> {
		self.extra.as_object().and_then(|obj| obj.get(key))
	}

	/// File (or update) each server's single `reachability` check
	/// from the freshness of the sources it is expected to report, against
	/// its per-server `alert_when_down_for` threshold. Passed when every
	/// expected source is fresh; warning when an `on`-mode source has gone
	/// quiet while others still report (the quiet sources are named in the
	/// detail); failed when every expected source is stale, or the server
	/// has never reported — unreachable. Each source's reachability mode
	/// (`on`/`quiet`/`off`) governs whether its silence warns, only counts
	/// toward unreachable, or is ignored. Servers with no counted source
	/// fall back to whether anything at all has reached canopy.
	///
	/// Unmonitored applications are swept alongside the rest: their reachability
	/// is recorded and presented, and the monitoring gate in
	/// [`crate::issues::NewEvent::save_with_state`] is what keeps it out of
	/// incidents. The UI marks them so an operator reads "unreachable and
	/// nobody is being paged" rather than "unreachable".
	///
	/// Returns the number of events filed in this pass.
	// spec: CHK#monitoring-gate
	pub async fn sweep_staleness(db: &mut AsyncPgConnection) -> Result<usize> {
		let applications = Self::sweep_application_staleness(db).await?;
		let machines = Self::sweep_machine_staleness(db).await?;
		Ok(applications + machines)
	}

	/// The application half of [`Self::sweep_staleness`].
	// spec: CHK#reachability
	async fn sweep_application_staleness(db: &mut AsyncPgConnection) -> Result<usize> {
		use std::collections::HashMap;

		let applications = Application::get_all(db, 0, None).await?;
		let swept: Vec<&Application> = applications
			.iter()
			.filter(|s| s.id != Uuid::nil())
			.collect();
		if swept.is_empty() {
			return Ok(0);
		}
		let server_ids: Vec<Uuid> = swept.iter().map(|s| s.id).collect();

		// Per-source freshness (already excludes canopy/manual and
		// decommissioned checks), grouped by server, plus each source's
		// reachability and ingest modes.
		let freshness = Issue::source_freshness(db, &server_ids).await?;
		let expected = expected_sources(db, freshness).await?;

		// Backstop for applications with no counted source (never reported, or
		// every source excluded): when anything last reported, any source.
		//
		// Read from the current-state projection rather than from status
		// history, which is capped at `GRACE_LOOKBACK_SQL`: an application
		// quiet for longer than that window is absent from the windowed read,
		// so this filed "has never reported" against applications that had,
		// and with no elapsed time on the event to contradict it.
		let last_reported =
			crate::reported_detail::ReportedDetail::last_reported_ats(db, &server_ids).await?;
		let statuses = Self::latest_for_servers(db, &server_ids).await?;
		let status_map: HashMap<Uuid, Timestamp> = statuses
			.into_iter()
			.filter_map(|s| Some((s.server_id?, s.created_at)))
			.chain(last_reported)
			.fold(HashMap::new(), |mut acc, (id, at)| {
				acc.entry(id)
					.and_modify(|held| *held = (*held).max(at))
					.or_insert(at);
				acc
			});

		let existing_issues =
			Issue::list_by_source_ref(db, CANOPY_SOURCE, REACHABILITY_REF, &server_ids).await?;
		let open: HashMap<Uuid, bool> = existing_issues
			.iter()
			.filter_map(|i| i.application_id.map(|sid| (sid, i.active)))
			.collect();

		let now = Timestamp::now();
		let mut filed = 0usize;
		for server in &swept {
			let graded = grade_reachability(
				"Application",
				&server.label(),
				server.alert_when_down_for.0,
				expected.get(&server.id).map(Vec::as_slice).unwrap_or(&[]),
				status_map.get(&server.id).copied(),
				now,
			);
			if !worth_filing(&graded.0, open.get(&server.id).copied()) {
				continue;
			}
			let (observed, message, detail) = graded;

			crate::issues::file_check(
				db,
				crate::issues::CheckFiling {
					source: CANOPY_SOURCE,
					scope: crate::issues::Scope::Application(server.id),
					device_id: None,
					check: REACHABILITY_REF,
					observed,
					title: Some("Application reachability"),
					message: &message,
					detail: Some(detail),
					default_ceiling: CheckResult::Failed,
					default_escalates: false,
					documentation: Some(REACHABILITY_DOC),
				},
			)
			.await?;
			filed += 1;
		}

		Ok(filed)
	}

	/// The machine half of [`Self::sweep_staleness`].
	///
	/// A box's silence is its own fact, graded on its own threshold and its own
	/// reporters. Nothing here reads the applications on it and nothing there
	/// reads this: a machine that goes quiet stops reporting about its
	/// workloads by the same act, so each of them goes unreachable on its own
	/// account under the application sweep's identical rule.
	// spec: CHK#reachability
	async fn sweep_machine_staleness(db: &mut AsyncPgConnection) -> Result<usize> {
		use std::collections::HashMap;

		let live = crate::machines::Machine::list_live(db).await?;
		let machines: Vec<&crate::machines::Machine> =
			live.iter().filter(|m| m.id != Uuid::nil()).collect();
		if machines.is_empty() {
			return Ok(0);
		}
		let machine_ids: Vec<Uuid> = machines.iter().map(|m| m.id).collect();

		let freshness = Issue::source_freshness_for_machines(db, &machine_ids).await?;
		let expected = expected_sources(db, freshness).await?;

		// The same backstop the application sweep uses, over the machine's own
		// status history and current-state projection: a box with no counted
		// source falls back to when anything last reported about it.
		let last_reported =
			crate::reported_detail::MachineReportedDetail::latest_for_machines(db, &machine_ids)
				.await?;
		let statuses = Self::latest_for_machines(db, &machine_ids).await?;
		let last_reported: HashMap<Uuid, Timestamp> = statuses
			.into_iter()
			.filter_map(|s| Some((s.machine_id?, s.created_at)))
			.chain(last_reported)
			.fold(HashMap::new(), |mut acc, (id, at)| {
				acc.entry(id)
					.and_modify(|held| *held = (*held).max(at))
					.or_insert(at);
				acc
			});

		let existing_issues = Issue::list_by_source_ref_for_machines(
			db,
			CANOPY_SOURCE,
			REACHABILITY_REF,
			&machine_ids,
		)
		.await?;
		let open: HashMap<Uuid, bool> = existing_issues
			.iter()
			.filter_map(|i| i.machine_id.map(|mid| (mid, i.active)))
			.collect();

		let now = Timestamp::now();
		let mut filed = 0usize;
		for machine in machines {
			let graded = grade_reachability(
				"Machine",
				&machine_label(machine),
				machine.alert_when_down_for.0,
				expected.get(&machine.id).map(Vec::as_slice).unwrap_or(&[]),
				last_reported.get(&machine.id).copied(),
				now,
			);
			if !worth_filing(&graded.0, open.get(&machine.id).copied()) {
				continue;
			}
			let (observed, message, detail) = graded;

			crate::issues::file_check(
				db,
				crate::issues::CheckFiling {
					source: CANOPY_SOURCE,
					scope: crate::issues::Scope::Machine(machine.id),
					device_id: None,
					check: REACHABILITY_REF,
					observed,
					title: Some("Machine reachability"),
					message: &message,
					detail: Some(detail),
					default_ceiling: CheckResult::Failed,
					default_escalates: false,
					documentation: Some(REACHABILITY_DOC),
				},
			)
			.await?;
			filed += 1;
		}

		Ok(filed)
	}

	/// Most recent status row (across all applications) pushed by `source`
	/// whose `health` array contains an entry for `check_name`. Used by
	/// the rule-editor UI to surface a realistic sample of the variables
	/// an operator can predicate on (the check's extras, the status-level
	/// extras, and the server's tags resolved up the group). Scoped to
	/// the check's own source — another source's same-named check may
	/// carry entirely different fields.
	///
	/// Bounded to [`GRACE_LOOKBACK_SQL`]: a recent sample is the useful one,
	/// and the `health @>` containment is not indexable, so without the
	/// window a check that has stopped reporting scans every partition
	/// across every server.
	/// The most recent push carrying this check.
	///
	/// `application_type` narrows to pushes from applications of that type,
	/// and is how a caller samples the check it means: an application-subject
	/// name is one check per type, and another type's push carries different
	/// fields under the same name. `None` samples any type, which is what a
	/// machine's check or a curated source's wants.
	pub async fn latest_for_check_name(
		db: &mut AsyncPgConnection,
		source: &str,
		check_name: &str,
		application_type: Option<&ApplicationType>,
	) -> Result<Option<Status>> {
		use diesel::sql_types::{Text, Uuid as DUuid};
		use diesel::{QueryableByName, sql_query};

		#[derive(QueryableByName)]
		struct Picked {
			#[diesel(sql_type = DUuid, column_name = "id")]
			row_id: Uuid,
		}

		// Two-step: pick the id via raw SQL (JSONB containment needs a
		// parameterised JSON literal that Diesel's typed DSL doesn't
		// express cleanly), then load the typed Status row by id.
		let type_clause = if application_type.is_some() {
			"AND server_id IN (SELECT id FROM applications WHERE type = $3) "
		} else {
			""
		};
		let query = sql_query(format!(
			"SELECT id FROM statuses \
			 WHERE source = $1 \
			 AND health @> jsonb_build_array(jsonb_build_object('check', $2::text)) \
			 {type_clause}\
			 AND created_at >= {GRACE_LOOKBACK_SQL} \
			 ORDER BY created_at DESC LIMIT 1"
		))
		.bind::<Text, _>(source)
		.bind::<Text, _>(check_name);
		let picked: Option<Picked> = match application_type {
			Some(ty) => query.bind::<Text, _>(ty.to_string()).get_result(db).await,
			None => query.get_result(db).await,
		}
		.optional()
		.map_err(AppError::from)?;
		let Some(Picked { row_id }) = picked else {
			return Ok(None);
		};

		use crate::schema::statuses::dsl;
		dsl::statuses
			.select(Self::as_select())
			.filter(dsl::id.eq(row_id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	pub async fn latest_for_server(
		db: &mut AsyncPgConnection,
		server: Uuid,
	) -> Result<Option<Status>> {
		use crate::schema::statuses::dsl::*;

		statuses
			.select(Status::as_select())
			.filter(
				server_id
					.eq(server)
					.and(created_at.ge(diesel::dsl::sql("NOW() - INTERVAL '7 days'")))
					.and(id.ne(Uuid::nil())),
			)
			.order(created_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Most recent status row for `server` with `created_at <= at`. No
	/// time-window cap — operators reviewing historical issues need
	/// the actual contemporary status, even if it's old.
	pub async fn at_time(
		db: &mut AsyncPgConnection,
		server: Uuid,
		at: Timestamp,
	) -> Result<Option<Status>> {
		use crate::schema::statuses::dsl::*;

		statuses
			.select(Status::as_select())
			.filter(
				server_id
					.eq(server)
					.and(created_at.le(jiff_diesel::Timestamp::from(at)))
					.and(id.ne(Uuid::nil())),
			)
			.order(created_at.desc())
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Each source's most recent status for `server` at or before `at`
	/// (latest overall when `at` is `None`) — one row per source. Unlike
	/// [`Self::at_time`], which collapses to a single row regardless of
	/// source, this keeps every source's contribution, for reconstructing
	/// the consolidated multi-source checks view as of a point in time.
	///
	/// Bounded to [`GRACE_LOOKBACK`] before the cutoff, so a source silent
	/// for the whole window contributes nothing as of `at`. The bound is
	/// load-bearing: `DISTINCT ON` cannot terminate early (it has to see
	/// every candidate row before it knows which is each group's newest), so
	/// an upper bound alone made this read a server's entire status history —
	/// ~864k rows across every partition in prod — to return one row per
	/// source. This is the same trap [`Self::latest_for_servers`] documents.
	pub async fn latest_per_source_at(
		db: &mut AsyncPgConnection,
		server: Uuid,
		at: Option<Timestamp>,
	) -> Result<Vec<Status>> {
		use crate::schema::statuses::dsl::*;

		let cutoff = at.unwrap_or_else(Timestamp::now);
		let floor = cutoff.checked_sub(GRACE_LOOKBACK).unwrap_or(Timestamp::MIN);
		statuses
			.select(Status::as_select())
			.filter(
				server_id
					.eq(server)
					.and(id.ne(Uuid::nil()))
					.and(created_at.le(jiff_diesel::Timestamp::from(cutoff)))
					.and(created_at.ge(jiff_diesel::Timestamp::from(floor))),
			)
			.distinct_on(source)
			.order((source, created_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Each source's most recent machine-scoped status for `machine` at or
	/// before `at`, the machine-grain counterpart of
	/// [`Self::latest_per_source_at`].
	///
	/// Only rows the box filed for itself count, so a workload's own report
	/// carried on the same machine is not read back as the box's. A reporter
	/// still pushing the unified shape files no such row at all; its machine
	/// checks are recognised by subject in the application's rows instead.
	// spec: CHK#a-machines-checks-present-on-its-applications
	pub async fn machine_latest_per_source_at(
		db: &mut AsyncPgConnection,
		machine: Uuid,
		at: Option<Timestamp>,
	) -> Result<Vec<Status>> {
		use crate::schema::statuses::dsl::*;

		let cutoff = at.unwrap_or_else(Timestamp::now);
		let floor = cutoff.checked_sub(GRACE_LOOKBACK).unwrap_or(Timestamp::MIN);
		statuses
			.select(Status::as_select())
			.filter(
				machine_id
					.eq(machine)
					.and(server_id.is_null())
					.and(id.ne(Uuid::nil()))
					.and(created_at.le(jiff_diesel::Timestamp::from(cutoff)))
					.and(created_at.ge(jiff_diesel::Timestamp::from(floor))),
			)
			.distinct_on(source)
			.order((source, created_at.desc()))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The machine-grain equivalent of [`Self::latest_for_servers`]: when each
	/// box last had a status row recorded against it, whatever it described.
	pub async fn latest_for_machines(
		db: &mut AsyncPgConnection,
		machine_ids: &[Uuid],
	) -> Result<Vec<Status>> {
		if machine_ids.is_empty() {
			return Ok(Vec::new());
		}

		let query = diesel::sql_query(
			"SELECT st.* FROM unnest($1) AS m(id) \
			 CROSS JOIN LATERAL ( \
				SELECT * FROM statuses \
				WHERE machine_id = m.id \
				AND created_at >= NOW() - INTERVAL '7 days' \
				AND id != '00000000-0000-0000-0000-000000000000' \
				ORDER BY created_at DESC LIMIT 1 \
			 ) st",
		)
		.bind::<diesel::sql_types::Array<diesel::sql_types::Uuid>, _>(machine_ids);

		query.load::<Status>(db).await.map_err(AppError::from)
	}

	pub async fn latest_for_servers(
		db: &mut AsyncPgConnection,
		server_ids: &[Uuid],
	) -> Result<Vec<Status>> {
		if server_ids.is_empty() {
			return Ok(Vec::new());
		}

		// One LATERAL probe per server rather than DISTINCT ON over the
		// window: DISTINCT ON can't terminate early, so it reads every
		// status row in the 7-day window for each server. The LIMIT 1 under
		// the lateral join reads one row per weekly partition through the
		// (server_id, created_at DESC) composite index.
		let query = diesel::sql_query(
			"SELECT st.* FROM unnest($1) AS s(id) \
			 CROSS JOIN LATERAL ( \
				SELECT * FROM statuses \
				WHERE server_id = s.id \
				AND created_at >= NOW() - INTERVAL '7 days' \
				AND id != '00000000-0000-0000-0000-000000000000' \
				ORDER BY created_at DESC LIMIT 1 \
			 ) st",
		)
		.bind::<diesel::sql_types::Array<diesel::sql_types::Uuid>, _>(server_ids);

		query.load::<Status>(db).await.map_err(AppError::from)
	}

	/// This one status's server-wide detail, for a caller that has a single
	/// status in hand rather than a server's set of sources. Prefer
	/// [`MergedDetail::from_statuses`] where every source is available: a
	/// figure this push doesn't carry is one another source may.
	pub fn detail(&self) -> MergedDetail {
		MergedDetail(self.extra.as_object().cloned().unwrap_or_default())
	}

	pub fn platform(&self) -> Option<String> {
		self.detail().platform()
	}

	pub fn postgres_version(&self) -> Option<String> {
		self.detail().postgres_version()
	}

	/// Node.js runtime version the server reported in its status payload
	/// (`nodeVersion` extra), if present. Preferred over scraping the device
	/// connection's User-Agent (see [`crate::devices::DeviceConnection::nodejs_version`]),
	/// which only reflects whichever transport happened to set that header.
	pub fn node_version(&self) -> Option<String> {
		self.detail().node_version()
	}

	/// Version the server's reporting schema was built for, as the server
	/// reported it (`reportingSchemaVersion` extra). Absent until a server
	/// runs a schema that stamps one.
	pub fn reporting_schema_version(&self) -> Option<String> {
		self.detail().reporting_schema_version()
	}

	/// Identified operators connected to the server as of this status
	/// row, from the `external_users` check. Display fields are unfilled;
	/// see [`commons_types::status::operators_from_health`].
	pub fn operators(&self) -> Vec<commons_types::status::OperatorPresence> {
		commons_types::status::operators_from_health(&self.health)
	}

	pub fn distance_from_version(&self, version: &Version) -> Option<u64> {
		let current = self.version.as_ref().map(|v| &v.0)?;
		Some(version_distance(current, version))
	}
}

/// A server's server-wide detail resolved across all the sources reporting on
/// it: for each key, the value from the most recent source that carries it.
///
/// Sources don't all report the same fields — only bestool reports
/// `bestoolVersion`, while a legacy Tamanu push carries neither that nor
/// `pgVersion` — so reading the figures off whichever source pushed last
/// makes them blink out as sources interleave. Falling through to an older
/// source per key holds each figure at its last reported value instead.
// spec: FIG#sourcing
#[derive(Debug, Clone, Default)]
pub struct MergedDetail(serde_json::Map<String, serde_json::Value>);

impl MergedDetail {
	/// Fold each source's report — `(when it was reported, what it carried)`,
	/// in any order — into the resolved detail. Newer reports win per key; a
	/// key absent from the newest falls through to the newest report that
	/// has it.
	///
	/// Which reports are passed sets what this sees. The live path hands it
	/// every source's current report (see
	/// [`crate::reported_detail::ReportedDetail`]); the point-in-time path
	/// hands it each source's latest push at-or-before a moment.
	pub fn from_reports<'a>(
		reports: impl IntoIterator<Item = (Timestamp, &'a serde_json::Value)>,
	) -> Self {
		// Callers' orderings vary and none of them are chronological —
		// latest_per_source_at orders by source name, the table read by
		// whatever the scan yields. "Newest wins" silently reading as "last
		// row wins" would be wrong in a way nothing would surface.
		let mut ordered: Vec<(Timestamp, &serde_json::Value)> = reports.into_iter().collect();
		ordered.sort_by_key(|(at, _)| *at);

		let mut merged = serde_json::Map::new();
		for (_, extra) in ordered {
			let Some(obj) = extra.as_object() else {
				continue;
			};
			for (key, value) in obj {
				if value.is_null() {
					continue;
				}
				merged.insert(key.clone(), value.clone());
			}
		}
		Self(merged)
	}

	/// [`Self::from_reports`] over one server's statuses.
	pub fn from_statuses(statuses: &[Status]) -> Self {
		Self::from_reports(statuses.iter().map(|st| (st.created_at, &st.extra)))
	}

	pub fn get(&self, key: &str) -> Option<&serde_json::Value> {
		self.0.get(key)
	}

	/// The resolved detail as a JSON object, for handing to a client that
	/// wants to read fields canopy doesn't derive figures from.
	pub fn into_json(self) -> serde_json::Value {
		serde_json::Value::Object(self.0)
	}

	fn string(&self, key: &str) -> Option<String> {
		self.get(key)
			.and_then(|v| v.as_str())
			.map(ToOwned::to_owned)
	}

	/// The operating system the server runs, as it reports it — the name,
	/// qualified by the version when it reports one.
	///
	/// A server that reports neither falls back to what the PostgreSQL
	/// version banner gives away: it names its build toolchain, which
	/// distinguishes a Windows build from any other but says nothing finer.
	/// So an unreported platform degrades to the family rather than to
	/// nothing.
	// spec: FIG#figures
	pub fn platform(&self) -> Option<String> {
		self.os_platform()
			.or_else(|| self.pg_version_banner().map(platform_family_of))
	}

	/// The operating system as this detail reports it, with no fallback: the
	/// name, qualified by the version where one is reported.
	///
	/// A machine's platform falls back to what an application on it gives
	/// away, which is a different set of reports, so the fallback is the
	/// caller's to apply rather than this one's.
	// spec: FIG#machine-figures
	pub fn os_platform(&self) -> Option<String> {
		let name = self.string("osName")?;
		Some(match self.string("osVersion") {
			Some(version) => format!("{name} {version}"),
			None => name,
		})
	}

	/// The database engine version banner as reported, whole.
	pub fn pg_version_banner(&self) -> Option<String> {
		self.string("pgVersion")
	}

	pub fn postgres_version(&self) -> Option<String> {
		self.get("pgVersion")
			.and_then(|pg| pg.as_str())
			.and_then(|pg| pg.split_ascii_whitespace().nth(1))
			.map(|vers| vers.trim_end_matches(',').into())
	}

	pub fn node_version(&self) -> Option<String> {
		self.string("nodeVersion")
	}

	pub fn reporting_schema_version(&self) -> Option<String> {
		self.string("reportingSchemaVersion")
	}

	pub fn timezone(&self) -> Option<String> {
		self.string("timezone")
	}

	/// Whether the server runs Munin. Absent when no source has reported the
	/// flag — which is not the same as reporting that it doesn't.
	// spec: SVC#munin-link
	pub fn munin(&self) -> Option<bool> {
		self.get("munin").and_then(|v| v.as_bool())
	}

	/// Version of bestool itself, which it reports alongside the rest of its
	/// server-wide detail. Absent for a server no bestool reports on.
	// spec: FIG#figures
	pub fn bestool_version(&self) -> Option<String> {
		self.string("bestoolVersion")
	}
}

/// The operating system family a PostgreSQL version banner gives away: the
/// banner names its build toolchain, which distinguishes a Windows build from
/// anything else but says nothing finer.
// spec: FIG#machine-figures
pub fn platform_family_of(pg_version: impl AsRef<str>) -> String {
	let pg = pg_version.as_ref();
	if pg.contains("Visual C++") || pg.contains("windows") {
		"Windows"
	} else {
		"Linux"
	}
	.into()
}

/// How far `current` lags behind `latest`, as `major_distance * 1000 +
/// minor_distance` with saturating subtraction (a newer-than-latest `current`
/// yields 0). Used for the status snapshot and the group card's headline.
pub fn version_distance(current: &Version, latest: &Version) -> u64 {
	let minor_distance = latest.minor.saturating_sub(current.minor);
	let major_distance = latest.major.saturating_sub(current.major);
	major_distance * 1000 + minor_distance
}
