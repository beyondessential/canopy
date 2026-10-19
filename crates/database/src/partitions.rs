//! Provisioning the weekly ranges the high-volume histories are written into.
//!
//! Spec: `.workhorse/specs/platform/history-storage.md` (id `HST`).
//!
//! `statuses` and `device_connections` are range-partitioned by week, and a
//! write only lands if a partition covering its timestamp exists — there is no
//! catch-all, so a week with no partition is a week the history cannot be
//! written at all. Canopy keeps the ranges provisioned ahead of itself from the
//! monitor loop rather than relying on an external schedule, and reports the
//! runway so the shortfall is alerted on long before it bites (see
//! [`crate::self_alerts::sweep_partition_runway`]).
//!
//! The DDL lives in SQL (`ensure_weekly_partitions`) because it has to: it
//! builds each partition detached and attaches it, which is what keeps it from
//! taking an exclusive lock on the history it is extending.

use commons_errors::Result;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};

/// The histories whose ranges Canopy provisions. `partition_runway` discovers
/// partitioned tables on its own; provisioning has to be told, because creating
/// a partition is not something to do to a table by inference.
pub const HISTORIES: [&str; 3] = ["statuses", "device_connections", "audit_entries"];

/// Weeks of future range kept provisioned beyond the current week. Four weeks
/// leaves the runway alert (below) a fortnight of warning before anything is at
/// risk, which is ample for a condition that only needs one successful pass to
/// clear.
pub const WEEKS_AHEAD: i32 = 4;

/// Runway below which the self-alert warns.
pub const WARN_DAYS: i32 = 14;

/// Runway below which the self-alert fails: a week's notice before the history
/// stops accepting writes.
pub const FAIL_DAYS: i32 = 7;

/// One week's outcome from a provisioning pass.
#[derive(Debug, Clone)]
pub struct Provisioned {
	pub partition: String,
	/// `created`, `attached`, `already_exists`, or `failed: <error>` — the SQL
	/// function isolates each week, so one failure doesn't stop the rest.
	pub action: String,
}

impl Provisioned {
	/// Whether this week failed to provision. The pass as a whole still
	/// succeeds: it runs again shortly, and the runway alert is what catches a
	/// failure that persists.
	pub fn failed(&self) -> bool {
		self.action.starts_with("failed")
	}
}

/// How much future range a history has left.
#[derive(Debug, Clone)]
pub struct Runway {
	pub parent: String,
	pub partitions: i64,
	/// Exclusive upper bound of the last partition, as an ISO date. `None` when
	/// the history has no ranges at all.
	pub covered_to: Option<String>,
	/// Days from today to that bound. Zero or negative means the history is
	/// already unwritable.
	pub days_remaining: i32,
}

impl Runway {
	pub fn short(&self) -> bool {
		self.days_remaining < WARN_DAYS
	}

	pub fn critical(&self) -> bool {
		self.days_remaining < FAIL_DAYS
	}
}

/// Provision the current week plus `weeks` future weeks for every history.
///
/// Idempotent and safe to run concurrently with itself, with the external
/// schedule, and with ingestion. Returns only the weeks that were acted on or
/// failed — a steady state returns nothing.
pub async fn ensure_runway(db: &mut AsyncPgConnection, weeks: i32) -> Result<Vec<Provisioned>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Text)]
		partition_name: String,
		#[diesel(sql_type = sql_types::Text)]
		action: String,
	}

	let mut acted = Vec::new();
	for history in HISTORIES {
		let rows: Vec<Row> =
			sql_query("SELECT partition_name, action FROM ensure_weekly_partitions($1, $2)")
				.bind::<sql_types::Text, _>(history)
				.bind::<sql_types::Integer, _>(weeks)
				.load(db)
				.await?;
		acted.extend(
			rows.into_iter()
				.filter(|r| r.action != "already_exists")
				.map(|r| Provisioned {
					partition: r.partition_name,
					action: r.action,
				}),
		);
	}
	Ok(acted)
}

/// Future range remaining per partitioned history, worst first.
pub async fn runway(db: &mut AsyncPgConnection) -> Result<Vec<Runway>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = sql_types::Text)]
		parent: String,
		#[diesel(sql_type = sql_types::BigInt)]
		partitions: i64,
		#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
		covered_to: Option<String>,
		#[diesel(sql_type = sql_types::Integer)]
		days_remaining: i32,
	}

	let rows: Vec<Row> = sql_query(
		"SELECT parent, partitions, covered_to::text AS covered_to, days_remaining \
		 FROM partition_runway() \
		 ORDER BY days_remaining, parent",
	)
	.load(db)
	.await?;

	Ok(rows
		.into_iter()
		.map(|r| Runway {
			parent: r.parent,
			partitions: r.partitions,
			covered_to: r.covered_to,
			days_remaining: r.days_remaining,
		})
		.collect())
}
