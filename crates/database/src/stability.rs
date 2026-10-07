//! Per-state stability records (CHK "Stability"): fixed-size summaries of
//! a check's *observed* behaviour, updated on every filing and never fed
//! by policy — so operator grading and any noise damping built on top of
//! these numbers can't feed back into them.
//!
//! One row per check state (1:1 with the `issues` row), holding lifetime
//! observation counters, a bounded ring of healthy↔degraded transitions,
//! and an hour-of-week degradation profile that leans towards recent
//! weeks. Everything else (flap counts, typical run lengths) is derived
//! at read time.

use commons_errors::{AppError, Result};
use commons_types::status::CheckResult;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// How many healthy↔degraded transitions each state remembers. A stable
/// check's ring reaches far into the past; a flapping check's ring covers
/// only its recent churn — which is exactly the signal.
pub const TRANSITION_RING_CAP: usize = 32;

/// Hour-of-week buckets: 7 days × 24 hours, UTC, Monday 00:00 first.
pub const DUTY_BUCKETS: usize = 168;

/// When a duty-cycle bucket's observation count crosses this cap, both of
/// its counters are halved: the ratio is preserved while old behaviour
/// gradually decays, keeping the profile bounded and recent-leaning. At a
/// minute report cadence a bucket sees ~60 observations per week, so the
/// cap corresponds to roughly two months of history.
pub const DUTY_BUCKET_CAP: i64 = 512;

/// One healthy↔degraded transition: the state became (or was first
/// observed) `degraded`/healthy at `at`.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct Transition {
	/// When the state changed.
	pub at: Timestamp,
	/// Whether it became degraded (true) or healthy (false).
	pub degraded: bool,
}

/// One hour-of-week bucket of the degradation profile.
#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, utoipa::ToSchema)]
pub struct DutyBucket {
	/// Observations landing in this hour-of-week.
	pub observations: i64,
	/// How many of them were degraded.
	pub degraded: i64,
}

/// A stability row as stored: counters plus the ring and profile as JSONB.
#[derive(Debug, Clone, Queryable, Selectable)]
#[diesel(table_name = crate::schema::check_stability)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct CheckStability {
	pub issue_id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	pub observations: i64,
	pub degraded_observations: i64,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub last_observed_at: Option<Timestamp>,
	pub last_observed_degraded: Option<bool>,
	pub transitions: serde_json::Value,
	pub duty_cycle: serde_json::Value,
}

impl CheckStability {
	/// The transition ring, oldest first. Unparseable content reads as
	/// empty rather than failing the caller.
	pub fn transition_ring(&self) -> Vec<Transition> {
		serde_json::from_value(self.transitions.clone()).unwrap_or_default()
	}

	/// The duty-cycle profile, padded to [`DUTY_BUCKETS`]. Stored as bare
	/// `[observations, degraded]` pairs to keep the JSONB compact.
	pub fn duty_profile(&self) -> Vec<DutyBucket> {
		let mut buckets: Vec<DutyBucket> =
			serde_json::from_value::<Vec<(i64, i64)>>(self.duty_cycle.clone())
				.unwrap_or_default()
				.into_iter()
				.map(|(observations, degraded)| DutyBucket {
					observations,
					degraded,
				})
				.collect();
		buckets.resize(DUTY_BUCKETS, DutyBucket::default());
		buckets
	}

	/// Stability rows for a set of states, keyed by issue id.
	pub async fn for_issue_ids(
		conn: &mut AsyncPgConnection,
		issue_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Self>> {
		use crate::schema::check_stability::dsl;
		if issue_ids.is_empty() {
			return Ok(Default::default());
		}
		let rows: Vec<Self> = dsl::check_stability
			.select(Self::as_select())
			.filter(dsl::issue_id.eq_any(issue_ids))
			.load(conn)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().map(|r| (r.issue_id, r)).collect())
	}
}

/// Check states matching any of the given (source, check) pairs, each
/// with its stability row (absent for states that predate stability
/// recording), across all scopes: server, group, and canopy-wide.
///
/// `application_id` narrows to that server's states; `group_id` narrows to the
/// group's applications plus the group's own group-scoped states.
pub async fn states_for_checks(
	conn: &mut AsyncPgConnection,
	pairs: &[(String, String)],
	application_id: Option<Uuid>,
	group_id: Option<Uuid>,
) -> Result<Vec<(crate::issues::Issue, Option<CheckStability>)>> {
	use crate::issues::Issue;
	use crate::schema::{applications, issues};
	use diesel::pg::Pg;
	use diesel::sql_types::{Bool, Nullable};

	if pairs.is_empty() {
		return Ok(Vec::new());
	}

	// `check_name` is nullable, so the whole OR-chain is Nullable<Bool>
	// (NULL never matches, which is what we want).
	let mut matches_pair: Box<dyn BoxableExpression<issues::table, Pg, SqlType = Nullable<Bool>>> =
		Box::new(diesel::dsl::sql::<Nullable<Bool>>("FALSE"));
	for (source, check) in pairs {
		matches_pair = Box::new(
			matches_pair.or(issues::source
				.eq(source.clone())
				.and(issues::check_name.eq(check.clone()))),
		);
	}

	let mut q = issues::table
		.select(Issue::as_select())
		.filter(matches_pair)
		.filter(issues::observed_result.is_not_null())
		.into_boxed();
	if let Some(sid) = application_id {
		q = q.filter(issues::application_id.eq(sid));
	}
	if let Some(gid) = group_id {
		let member_ids: Vec<Uuid> = applications::table
			.select(applications::id)
			.filter(applications::group_id.eq(gid))
			.load(conn)
			.await?;
		q = q.filter(
			issues::application_id
				.eq_any(member_ids)
				.or(issues::server_group_id.eq(gid)),
		);
	}
	let states: Vec<Issue> = q.load(conn).await?;

	let ids: Vec<Uuid> = states.iter().map(|st| st.id).collect();
	let mut rows = CheckStability::for_issue_ids(conn, &ids).await?;
	Ok(states
		.into_iter()
		.map(|st| {
			let stability = rows.remove(&st.id);
			(st, stability)
		})
		.collect())
}

/// Is this observed result a degraded observation? Skipped carries no
/// signal at all and is never recorded (see the CHK spec's Stability
/// section).
fn observed_degraded(observed: CheckResult) -> Option<bool> {
	match observed {
		CheckResult::Warning | CheckResult::Failed | CheckResult::Broken => Some(true),
		CheckResult::Passed => Some(false),
		CheckResult::Skipped => None,
	}
}

/// The hour-of-week bucket (UTC, Monday 00:00 = 0) a timestamp lands in.
fn duty_bucket_index(at: Timestamp) -> usize {
	let zoned = at.to_zoned(jiff::tz::TimeZone::UTC);
	let weekday = zoned.weekday().to_monday_zero_offset() as usize;
	weekday * 24 + zoned.hour() as usize
}

/// Record one observation against a state's stability row, creating the
/// row on first sight. Called from the check-state stamping path inside
/// the filing transaction; the caller already holds the issue row lock,
/// which serialises concurrent filings for the same state.
pub(crate) async fn record_observation(
	conn: &mut AsyncPgConnection,
	issue_id: Uuid,
	observed: CheckResult,
	at: Timestamp,
) -> Result<()> {
	use crate::schema::check_stability::dsl;

	let Some(degraded) = observed_degraded(observed) else {
		return Ok(());
	};

	diesel::insert_into(dsl::check_stability)
		.values(dsl::issue_id.eq(issue_id))
		.on_conflict(dsl::issue_id)
		.do_nothing()
		.execute(conn)
		.await?;
	let row: CheckStability = dsl::check_stability
		.select(CheckStability::as_select())
		.filter(dsl::issue_id.eq(issue_id))
		.for_update()
		.first(conn)
		.await?;

	let mut ring = row.transition_ring();
	if row.last_observed_degraded != Some(degraded) {
		ring.push(Transition { at, degraded });
		if ring.len() > TRANSITION_RING_CAP {
			let excess = ring.len() - TRANSITION_RING_CAP;
			ring.drain(..excess);
		}
	}

	let mut duty = row.duty_profile();
	let bucket = &mut duty[duty_bucket_index(at)];
	bucket.observations += 1;
	if degraded {
		bucket.degraded += 1;
	}
	if bucket.observations > DUTY_BUCKET_CAP {
		bucket.observations /= 2;
		bucket.degraded /= 2;
	}
	let duty_compact: Vec<(i64, i64)> = duty.iter().map(|b| (b.observations, b.degraded)).collect();

	// A backdated `occurred_at` must not roll `last_observed_at` backwards.
	let last_observed_at = row.last_observed_at.map_or(at, |prev| prev.max(at));
	diesel::update(dsl::check_stability.filter(dsl::issue_id.eq(issue_id)))
		.set((
			dsl::observations.eq(row.observations + 1),
			dsl::degraded_observations.eq(row.degraded_observations + i64::from(degraded)),
			dsl::last_observed_at.eq(jiff_diesel::Timestamp::from(last_observed_at)),
			dsl::last_observed_degraded.eq(degraded),
			// Plain structs of scalars: serialization cannot fail.
			dsl::transitions.eq(serde_json::to_value(&ring).unwrap_or_default()),
			dsl::duty_cycle.eq(serde_json::to_value(&duty_compact).unwrap_or_default()),
		))
		.execute(conn)
		.await?;
	Ok(())
}

/// Flap statistics derived from a transition ring.
#[derive(Debug, Clone, Default, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StabilityStats {
	/// State changes recorded in the last 24 hours.
	pub flips_24h: u32,
	/// State changes recorded in the last 7 days.
	pub flips_7d: u32,
	/// The oldest remembered transition: the flip counts only see back to
	/// here, so on a heavily flapping state they are lower bounds.
	pub ring_covers_from: Option<Timestamp>,
	/// Median length of the completed degraded runs the ring remembers,
	/// in seconds.
	pub typical_degraded_run_secs: Option<i64>,
	/// Median length of the completed healthy gaps between remembered
	/// degraded runs, in seconds.
	pub typical_healthy_gap_secs: Option<i64>,
}

/// Derive [`StabilityStats`] from a transition ring (oldest first).
pub fn derive_stats(ring: &[Transition], now: Timestamp) -> StabilityStats {
	let day_ago = now - SignedDuration::from_hours(24);
	let week_ago = now - SignedDuration::from_hours(24 * 7);
	let flips_24h = ring.iter().filter(|t| t.at >= day_ago).count() as u32;
	let flips_7d = ring.iter().filter(|t| t.at >= week_ago).count() as u32;

	let mut degraded_runs: Vec<i64> = Vec::new();
	let mut healthy_gaps: Vec<i64> = Vec::new();
	for pair in ring.windows(2) {
		let secs = pair[1].at.duration_since(pair[0].at).as_secs();
		if secs < 0 {
			continue;
		}
		if pair[0].degraded {
			degraded_runs.push(secs);
		} else {
			healthy_gaps.push(secs);
		}
	}

	StabilityStats {
		flips_24h,
		flips_7d,
		ring_covers_from: ring.first().map(|t| t.at),
		typical_degraded_run_secs: median(&mut degraded_runs),
		typical_healthy_gap_secs: median(&mut healthy_gaps),
	}
}

fn median(values: &mut [i64]) -> Option<i64> {
	if values.is_empty() {
		return None;
	}
	values.sort_unstable();
	Some(values[values.len() / 2])
}

/// A state's full stability record on the wire: the stored counters, ring,
/// and profile, plus the derived statistics. Shared by the private API and
/// the MCP interface.
#[derive(Debug, Clone, Serialize, Deserialize, utoipa::ToSchema)]
pub struct StabilityData {
	/// Total observations recorded for this state.
	pub observations: i64,
	/// How many of them were degraded (observed warning/failed/broken).
	pub degraded_observations: i64,
	/// When the state was last observed (skipped observations excluded).
	pub last_observed_at: Option<Timestamp>,
	/// Whether the last observation was degraded.
	pub last_observed_degraded: Option<bool>,
	/// The remembered healthy↔degraded transitions, oldest first.
	pub transitions: Vec<Transition>,
	/// Hour-of-week degradation profile: 168 buckets, UTC, Monday 00:00
	/// first, each with how many observations landed there and how many
	/// were degraded. Recent-leaning: bucket counters halve at a cap.
	pub duty_cycle: Vec<DutyBucket>,
	/// Statistics derived from the transition ring.
	pub stats: StabilityStats,
}

impl StabilityData {
	pub fn from_row(row: &CheckStability, now: Timestamp) -> Self {
		let transitions = row.transition_ring();
		let stats = derive_stats(&transitions, now);
		Self {
			observations: row.observations,
			degraded_observations: row.degraded_observations,
			last_observed_at: row.last_observed_at,
			last_observed_degraded: row.last_observed_degraded,
			transitions,
			duty_cycle: row.duty_profile(),
			stats,
		}
	}
}
