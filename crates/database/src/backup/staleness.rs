//! Staleness scan over reported runs + maintenance-staleness, DB-driven.
//!
//! Application-centric, per `(server, type)`: the subject is the server being
//! protected; the device is the actor recorded in `backup_runs`.
//!
//! The scanned set is every enabled `(server, type)` capability whose
//! effective schedule has a non-NULL `expected_interval` and whose group's
//! `server_group_backup_config.status = 'ready'`. The effective schedule is
//! the group's override row if it has one, else the type's canopy-wide
//! `default_interval` — the same precedence the schedulers resolve with
//! ([`crate::backups::effective_interval`]), so every pair that is commanded
//! to back up is also monitored. Disabled / manual-only (an override row with
//! a NULL interval) / non-ready configs are simply not in the set, so
//! unauthorized or un-set-up devices never alert.

use std::collections::HashMap;

use commons_errors::Result;
use commons_types::{
	backup::{BackupType, RunOutcome},
	status::CheckResult,
};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Span, SpanRelativeTo, SpanRound, Timestamp, Unit};
use uuid::Uuid;

use crate::{
	applications::Application,
	backup::refs,
	issues::{
		CheckFiling, CheckInstance, GradedInstance, InstancedCheckFiling, Scope, file_check,
		file_check_instances,
	},
};

/// Maintenance cadence threshold. Independent of any backup `expected_interval`
/// — maintenance runs on a full-weekly cadence, so a group whose last
/// successful maintenance is older than this is stale regardless of how often
/// its backups run. ~8 days gives a one-day grace over the weekly cadence.
pub const MAINTENANCE_STALE_AFTER: SignedDuration = SignedDuration::from_hours(8 * 24);

/// One `(machine, type)` to classify. Assembled by [`scan_rows`] from the
/// joined config / schedule / capability / runs / snapshots.
///
/// A box carrying two workloads is one row per type, not two: what a run
/// captures is the box's data, and scanning per application would raise the
/// same late backup twice.
// spec: BAK
#[derive(Debug, Clone)]
pub struct ScanRow {
	pub machine_id: Uuid,
	pub group_id: Uuid,
	/// Latest-associated device, recorded on the `NewEvent` actor field.
	pub device_id: Option<Uuid>,
	pub r#type: BackupType,
	pub is_monitored: bool,
	pub expected_interval: SignedDuration,
	pub config_created_at: Timestamp,
	/// When this machine was enrolled. `None` for one that never has.
	pub machine_registered_at: Option<Timestamp>,
	/// Latest `purpose='backup' AND outcome='success'` for this
	/// `(machine, type)`.
	pub last_success_at: Option<Timestamp>,
}

/// The staleness verdict for one `(server, type)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StalenessVerdict {
	/// A prior success exists but none newer than `now - grace`.
	Stale,
	/// No success ever, and `now - anchor > grace`.
	Never,
	/// Was stale, reporting success again (clears the issue).
	Recovered,
	/// Fresh enough, or not yet past the grace from its anchor — no alert.
	Ok,
}

impl ScanRow {
	/// `grace = expected_interval * 2`.
	fn grace(&self) -> SignedDuration {
		self.expected_interval.saturating_mul(2)
	}

	/// `anchor = max(machine_registered_at, config_created_at)`, so a box
	/// onboarded into a backup configuration that predates it is not stale on
	/// arrival. A machine that has never enrolled has no enrolment moment, and
	/// the anchor degenerates to `config_created_at` alone.
	///
	/// Anchored on the MACHINE rather than the application, because the thing
	/// being backed up is the box. Anchoring on an application's registration
	/// would restart a machine's backup deadline every time a workload was
	/// added to it, so a box that had been failing to back up for a month
	/// would read as freshly onboarded the moment someone deployed a second
	/// application onto it.
	// spec: BKJ
	fn anchor(&self) -> Timestamp {
		match self.machine_registered_at {
			Some(fs) if fs > self.config_created_at => fs,
			_ => self.config_created_at,
		}
	}

	/// Classify against `now`. `was_active` is whether a `backup-staleness`
	/// (or `backup-never`) issue is currently open for this `(server, type)` —
	/// used to distinguish a recovery from steady-state OK.
	pub fn classify(&self, now: Timestamp, was_active: bool) -> StalenessVerdict {
		let grace = self.grace();
		match self.last_success_at {
			Some(last) => {
				let stale = now.duration_since(last) > grace;
				if stale {
					StalenessVerdict::Stale
				} else if was_active {
					StalenessVerdict::Recovered
				} else {
					StalenessVerdict::Ok
				}
			}
			None => {
				// Never backed up: only alert once past the anchor + grace.
				if now.duration_since(self.anchor()) > grace {
					StalenessVerdict::Never
				} else {
					StalenessVerdict::Ok
				}
			}
		}
	}
}

/// Raw scan-set row: `(machine_id, group_id, is_monitored, device_id, type,
/// has_schedule_override, override_interval, default_interval,
/// config_created_at, registered_at)`.
type ScanBaseRow = (
	Uuid,
	Uuid,
	bool,
	Option<Uuid>,
	String,
	bool,
	Option<crate::pg_duration::PgDuration>,
	Option<crate::pg_duration::PgDuration>,
	jiff_diesel::Timestamp,
	Option<jiff_diesel::Timestamp>,
);

/// A scan-set row with its effective interval resolved, before the success and
/// anchor lookups are attached.
struct ScanRowBase {
	machine_id: Uuid,
	group_id: Uuid,
	is_monitored: bool,
	device_id: Option<Uuid>,
	r#type: BackupType,
	expected_interval: SignedDuration,
	config_created_at: Timestamp,
	machine_registered_at: Option<Timestamp>,
}

/// Build the scan set: every enabled `(machine, type)` capability in a
/// `status='ready'` group whose effective schedule has a non-NULL
/// `expected_interval`. Per `(machine, type)`, attach the latest backup success
/// and the machine's enrolment moment as the anchor.
///
/// Rooted at the machine rather than at the applications on it: a capability is
/// a box's, and a box shared by two workloads owes one backup rather than two.
// spec: BAK
pub async fn scan_rows(db: &mut AsyncPgConnection) -> Result<Vec<ScanRow>> {
	use crate::schema::{
		backup_type_defaults as defaults, machine_backup_capabilities as cap, machines,
		server_group_backup_config as cfg, server_group_backup_schedule as sched,
	};

	// Join: machines -> their group's ready config -> enabled capability, with
	// both interval sources left-joined. The effective interval is resolved in
	// Rust below rather than in SQL, because "override row present but NULL"
	// (manual-only) and "no override row" (inherit the default) have to stay
	// distinguishable — a COALESCE would flatten them together.
	let base: Vec<ScanBaseRow> = machines::table
		.inner_join(cfg::table.on(cfg::group_id.nullable().eq(machines::group_id)))
		.inner_join(cap::table.on(cap::machine_id.eq(machines::id)))
		.left_join(
			sched::table.on(sched::group_id
				.eq(cfg::group_id)
				.and(sched::type_.eq(cap::type_))),
		)
		.left_join(defaults::table.on(defaults::type_.eq(cap::type_)))
		.filter(machines::deleted_at.is_null())
		.filter(cfg::status.eq("ready"))
		.filter(cap::enabled.eq(true))
		.select((
			machines::id,
			cfg::group_id,
			machines::is_monitored,
			machines::device_id,
			cap::type_,
			sched::group_id.nullable().is_not_null(),
			sched::expected_interval.nullable(),
			defaults::default_interval.nullable(),
			cfg::created_at,
			machines::registered_at.nullable(),
		))
		.load(db)
		.await?;

	// Resolve each pair's effective interval, dropping the ones with none: an
	// override row answers on its own (NULL = manual-only), otherwise the type
	// default applies. Mirrors [`crate::backups::effective_interval`].
	let base: Vec<ScanRowBase> = base
		.into_iter()
		.filter_map(
			|(
				machine_id,
				group_id,
				is_monitored,
				device_id,
				ty,
				has_override,
				override_interval,
				default_interval,
				created_at,
				machine_registered_at,
			)| {
				let interval = if has_override {
					override_interval
				} else {
					default_interval
				}?;
				Some(ScanRowBase {
					machine_id,
					group_id,
					is_monitored,
					device_id,
					r#type: BackupType::from(ty),
					expected_interval: interval.0,
					config_created_at: Timestamp::from(created_at),
					machine_registered_at: machine_registered_at.map(Timestamp::from),
				})
			},
		)
		.collect();

	if base.is_empty() {
		return Ok(Vec::new());
	}

	// Latest success per (server, type), fetched per distinct group.
	let group_ids: Vec<Uuid> = {
		let mut s: std::collections::HashSet<Uuid> = base.iter().map(|r| r.group_id).collect();
		s.drain().collect()
	};
	let mut last_success: HashMap<(Uuid, BackupType), Timestamp> = HashMap::new();
	for gid in group_ids {
		let runs =
			crate::backups::BackupRun::latest_success_by_machine_type_for_group(db, gid).await?;
		for ((sid, ty), run) in runs {
			// The data's own moment, not when its upload finished: a backup that
			// took hours to transfer is as old as what it captured. `anchor` falls
			// back to the report time for a client that reports no freeze moment,
			// so this is a no-op for those. See `BackupRun::ANCHOR_SQL` for why the
			// query above orders by the same expression.
			last_success.insert((sid, ty), run.anchor());
		}
	}

	Ok(base
		.into_iter()
		.map(|row| {
			let last_success_at = last_success
				.get(&(row.machine_id, row.r#type.clone()))
				.copied();
			ScanRow {
				machine_id: row.machine_id,
				group_id: row.group_id,
				device_id: row.device_id,
				r#type: row.r#type,
				is_monitored: row.is_monitored,
				expected_interval: row.expected_interval,
				config_created_at: row.config_created_at,
				machine_registered_at: row.machine_registered_at,
				last_success_at,
			}
		})
		.collect())
}

/// Read an instance's `last_success_at` back out of its detail for the
/// message. The detail is the one place the specifics live now that the
/// message summarises across types.
fn last_success_of(instance: &GradedInstance) -> String {
	instance
		.detail
		.as_ref()
		.and_then(|d| d.get("last_success_at"))
		.and_then(|v| v.as_str())
		.unwrap_or("never")
		.to_owned()
}

/// Join instance labels for a message, in the order they were graded (most
/// urgent first). Shared with [`crate::backup::reconcile`] so every backup
/// check names its degraded instances the same way.
pub(super) fn label_list(instances: &[GradedInstance]) -> String {
	instances
		.iter()
		.map(|i| i.label.as_str())
		.collect::<Vec<_>>()
		.join(", ")
}

/// Run the full staleness sweep over a pre-computed scan: classify every
/// scanned `(server, type)`, then file one `backup-staleness` and one
/// `backup-never` check *per server* with the types as instances, and the
/// group-level `backup-maintenance-stale`. Returns the number of events
/// filed.
///
/// A server backing up four things has one staleness check, not four: the
/// types it is stale for are instances of that check (see the Names section
/// of the CHK spec), so an operator configures staleness once and writes a
/// rule on `check.type` where a particular type warrants different handling.
pub async fn sweep(db: &mut AsyncPgConnection, rows: &[ScanRow]) -> Result<usize> {
	let now = Timestamp::now();
	let mut filed = 0usize;

	// Group the scan by server, preserving first-seen order so the sweep stays
	// deterministic.
	let mut by_machine: HashMap<Uuid, Vec<&ScanRow>> = HashMap::new();
	let mut order: Vec<Uuid> = Vec::new();
	for row in rows {
		by_machine
			.entry(row.machine_id)
			.or_insert_with(|| {
				order.push(row.machine_id);
				Vec::new()
			})
			.push(row);
	}

	for machine_id in order {
		let server_rows = &by_machine[&machine_id];
		let device_id = server_rows.iter().find_map(|r| r.device_id);
		let machine = crate::machines::Machine::get_by_id(db, machine_id).await?;
		let label = machine_label(&machine);

		// Both flags are per-box now that the check is: whether this box's
		// staleness (or never) check is currently degraded at all.
		let stale_open = open_machine_issue_active(db, machine_id, refs::STALENESS).await?;
		let never_open = open_machine_issue_active(db, machine_id, refs::NEVER).await?;

		let mut stale_instances: Vec<CheckInstance> = Vec::with_capacity(server_rows.len());
		let mut never_instances: Vec<CheckInstance> = Vec::with_capacity(server_rows.len());
		let mut any_stale = false;
		let mut any_never = false;

		for row in server_rows {
			let verdict = row.classify(now, stale_open);
			let grace = row.grace();

			let stale = verdict == StalenessVerdict::Stale;
			any_stale |= stale;
			stale_instances.push(CheckInstance {
				label: row.r#type.to_string(),
				observed: if stale {
					CheckResult::Failed
				} else {
					CheckResult::Passed
				},
				detail: Some(serde_json::json!({
					"type": row.r#type.to_string(),
					"grace_secs": grace.as_secs(),
					"last_success_at": row.last_success_at.map(|t| t.to_string()),
				})),
			});

			// A server that has *never* backed up (freshly set up, or blocked
			// upstream) is a different condition from one that was backing up
			// and stopped, so it is its own check rather than an instance of
			// staleness.
			let never = verdict == StalenessVerdict::Never;
			any_never |= never;
			never_instances.push(CheckInstance {
				label: row.r#type.to_string(),
				observed: if never {
					CheckResult::Warning
				} else {
					CheckResult::Passed
				},
				detail: Some(serde_json::json!({
					"type": row.r#type.to_string(),
					"expected_since": row.anchor().to_string(),
				})),
			});
		}

		// File when something is degraded, or when the check is open and needs
		// clearing. A healthy server with nothing open has nothing to say.
		if any_stale || stale_open {
			let total = stale_instances.len();
			file_check_instances(
				db,
				InstancedCheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Machine(machine_id),
					device_id,
					check: refs::STALENESS,
					title: None,
					instances: stale_instances,
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::STALENESS_DOC),
				},
				&|degraded| match degraded {
					[] => format!("Application {label} is backing up on schedule again"),
					[one] => format!(
						"Application {label} has no recent {} backup (last success {})",
						one.label,
						last_success_of(one),
					),
					many => format!(
						"Application {label} has no recent backup for {} of its {total} types: {}",
						many.len(),
						label_list(many),
					),
				},
			)
			.await?;
			filed += 1;
		}

		if any_never || never_open {
			let total = never_instances.len();
			file_check_instances(
				db,
				InstancedCheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Machine(machine_id),
					device_id,
					check: refs::NEVER,
					title: None,
					instances: never_instances,
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::NEVER_DOC),
				},
				&|degraded| match degraded {
					[] => {
						format!("Application {label} has now backed up everything expected of it")
					}
					[one] => format!(
						"Application {label} has never reported a successful {} backup",
						one.label
					),
					many => format!(
						"Application {label} has never backed up {} of its {total} types: {}",
						many.len(),
						label_list(many),
					),
				},
			)
			.await?;
			filed += 1;
		}
	}

	filed += sweep_maintenance(db, now).await?;
	Ok(filed)
}

/// Group-level maintenance health, per `status='ready'` group:
///
/// - **Staleness** ([`refs::MAINTENANCE_STALE`]): latest *successful* run (any
///   kind) older than [`MAINTENANCE_STALE_AFTER`] — or none at all, past the
///   threshold from config creation. A fresh success clears it.
/// - **Failure** ([`refs::MAINTENANCE_ERROR`]): the most recently *finished*
///   run failed. Distinct from staleness — maintenance can run on cadence yet
///   error every time. A newer successful run clears it.
///
/// Both are `Error` severity (open an incident + page). A group that is both
/// stale and erroring files both, independently keyed.
async fn sweep_maintenance(db: &mut AsyncPgConnection, now: Timestamp) -> Result<usize> {
	use crate::schema::{backup_maintenance_runs as mr, server_group_backup_config as cfg};

	let groups: Vec<(Uuid, jiff_diesel::Timestamp)> = cfg::table
		.filter(cfg::status.eq("ready"))
		.select((cfg::group_id, cfg::created_at))
		.load(db)
		.await?;

	let mut filed = 0usize;
	for (group_id, created_at) in groups {
		let latest_success: Option<jiff_diesel::Timestamp> = mr::table
			.filter(mr::group_id.eq(group_id))
			.filter(mr::outcome.eq("success"))
			.select(diesel::dsl::max(mr::finished_at))
			.first::<Option<jiff_diesel::Timestamp>>(db)
			.await?;

		let reference = latest_success
			.map(Timestamp::from)
			.unwrap_or_else(|| Timestamp::from(created_at));
		let stale = now.duration_since(reference) > MAINTENANCE_STALE_AFTER;
		let was_active = open_group_issue_active(db, group_id, refs::MAINTENANCE_STALE).await?;

		if stale {
			file_check(
				db,
				CheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Group(group_id),
					device_id: None,
					check: refs::MAINTENANCE_STALE,
					observed: CheckResult::Failed,
					title: None,
					message: &format!(
						"No successful repo maintenance for {} (last {})",
						fmt_dur(MAINTENANCE_STALE_AFTER),
						latest_success
							.map(|t| Timestamp::from(t).to_string())
							.unwrap_or_else(|| "never".into()),
					),
					detail: Some(serde_json::json!({
						"threshold_secs": MAINTENANCE_STALE_AFTER.as_secs(),
						"last_success_at": latest_success.map(|t| Timestamp::from(t).to_string()),
					})),
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::MAINTENANCE_STALE_DOC),
				},
			)
			.await?;
			filed += 1;
		} else if !stale && was_active {
			file_check(
				db,
				CheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Group(group_id),
					device_id: None,
					check: refs::MAINTENANCE_STALE,
					observed: CheckResult::Passed,
					title: None,
					message: "Repo maintenance completed successfully again",
					detail: None,
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::MAINTENANCE_STALE_DOC),
				},
			)
			.await?;
			filed += 1;
		}

		// Failure leg: a group can run maintenance on cadence yet error every
		// time, which staleness (absence-of-success) never catches. Key off the
		// most recently *finished* run.
		let latest_completed =
			crate::backups::BackupMaintenanceRun::latest_completed_for_group(db, group_id).await?;
		let err_active = open_group_issue_active(db, group_id, refs::MAINTENANCE_ERROR).await?;
		match latest_completed {
			Some(run) if run.outcome == Some(RunOutcome::Failure) => {
				file_check(
					db,
					CheckFiling {
						source: crate::statuses::CANOPY_SOURCE,
						scope: Scope::Group(group_id),
						device_id: None,
						check: refs::MAINTENANCE_ERROR,
						observed: CheckResult::Failed,
						title: None,
						message: &format!(
							"Repo maintenance ({}) failed: {}",
							run.kind,
							run.error.as_deref().unwrap_or("(no detail reported)"),
						),
						detail: Some(serde_json::json!({
							"kind": run.kind,
							"error": run.error,
						})),
						default_ceiling: CheckResult::Warning,
						default_escalates: false,
						documentation: Some(refs::MAINTENANCE_ERROR_DOC),
					},
				)
				.await?;
				filed += 1;
			}
			// Most recent finished run succeeded (or there is none): clear any
			// open failure issue.
			_ if err_active => {
				file_check(
					db,
					CheckFiling {
						source: crate::statuses::CANOPY_SOURCE,
						scope: Scope::Group(group_id),
						device_id: None,
						check: refs::MAINTENANCE_ERROR,
						observed: CheckResult::Passed,
						title: None,
						message: "Repo maintenance completed successfully again",
						detail: None,
						default_ceiling: CheckResult::Warning,
						default_escalates: false,
						documentation: Some(refs::MAINTENANCE_ERROR_DOC),
					},
				)
				.await?;
				filed += 1;
			}
			_ => {}
		}
	}
	Ok(filed)
}

/// Whether an application-scoped `(canopy, ref)` issue is currently open and
/// active.
pub(crate) async fn open_server_issue_active(
	db: &mut AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
) -> Result<bool> {
	use crate::schema::issues::dsl;
	let n: i64 = dsl::issues
		.filter(dsl::application_id.eq(server_id))
		.filter(dsl::source.eq(refs::CANOPY_SOURCE))
		.filter(dsl::ref_.eq(r#ref))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.count()
		.get_result(db)
		.await?;
	Ok(n > 0)
}

/// The applications an active `(canopy, ref)` issue is open against, with the
/// group each belongs to.
///
/// One question of the whole fleet: a sweep that files per group otherwise asks
/// it per group to learn whether it has anything to recover.
pub(crate) async fn applications_with_open_issue(
	db: &mut AsyncPgConnection,
	r#ref: &str,
) -> Result<Vec<(Uuid, Option<Uuid>)>> {
	use crate::schema::{applications, issues};

	issues::table
		.inner_join(applications::table.on(applications::id.nullable().eq(issues::application_id)))
		.filter(issues::source.eq(refs::CANOPY_SOURCE))
		.filter(issues::ref_.eq(r#ref))
		.filter(issues::active.eq(true))
		.filter(issues::resolved_at.is_null())
		.select((applications::id, applications::group_id))
		.load(db)
		.await
		.map_err(Into::into)
}

/// Whether a machine-scoped `(canopy, ref)` check last *observed* something
/// other than a pass.
///
/// The active flag follows the effective result, so a check whose policy holds
/// it below alerting is never active however degraded its observations. This is
/// what a sweep asks in its place, to know whether it has a recorded
/// observation to bring up to date.
pub(crate) async fn machine_check_observed_degraded(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#ref: &str,
) -> Result<bool> {
	use crate::schema::issues::dsl;
	let n: i64 = dsl::issues
		.filter(dsl::machine_id.eq(machine_id))
		.filter(dsl::source.eq(refs::CANOPY_SOURCE))
		.filter(dsl::ref_.eq(r#ref))
		.filter(dsl::observed_result.is_not_null())
		.filter(dsl::observed_result.ne(CheckResult::Passed.to_string()))
		.count()
		.get_result(db)
		.await?;
	Ok(n > 0)
}

/// Every server with a currently-open, active `(canopy, ref)` issue for one of
/// these checks.
///
/// A sweep that re-derives its checks from current state has to visit these
/// applications even when it derives nothing for them: a check whose last instance is
/// gone is recovered by being filed as passing, and a server nobody visits is a
/// check left open with nothing that could ever clear it.
pub(crate) async fn servers_with_open_checks(
	db: &mut AsyncPgConnection,
	checks: &[&str],
) -> Result<Vec<Uuid>> {
	use crate::schema::issues::dsl;
	let ids: Vec<Option<Uuid>> = dsl::issues
		.select(dsl::application_id)
		.distinct()
		.filter(dsl::application_id.is_not_null())
		.filter(dsl::source.eq(refs::CANOPY_SOURCE))
		.filter(dsl::ref_.eq_any(checks.to_vec()))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.load(db)
		.await?;
	Ok(ids.into_iter().flatten().collect())
}

/// The machines holding an open, active `(canopy, ref)` issue for any of
/// `checks`. The machine-grain twin of [`servers_with_open_checks`]: a check
/// filed at machine scope leaves its issue on `issues.machine_id`, so looking
/// for it on `application_id` would never find it and the check could never be
/// visited to recover.
pub(crate) async fn machines_with_open_checks(
	db: &mut AsyncPgConnection,
	checks: &[&str],
) -> Result<Vec<Uuid>> {
	use crate::schema::issues::dsl;
	let ids: Vec<Option<Uuid>> = dsl::issues
		.select(dsl::machine_id)
		.distinct()
		.filter(dsl::machine_id.is_not_null())
		.filter(dsl::source.eq(refs::CANOPY_SOURCE))
		.filter(dsl::ref_.eq_any(checks.to_vec()))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.load(db)
		.await?;
	Ok(ids.into_iter().flatten().collect())
}

/// Whether a machine-scoped `(canopy, ref)` check is currently open + active.
/// The machine-grain twin of [`open_server_issue_active`].
pub(crate) async fn open_machine_issue_active(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#ref: &str,
) -> Result<bool> {
	use crate::schema::issues::dsl;
	let n: i64 = dsl::issues
		.filter(dsl::machine_id.eq(machine_id))
		.filter(dsl::source.eq(refs::CANOPY_SOURCE))
		.filter(dsl::ref_.eq(r#ref))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.count()
		.get_result(db)
		.await?;
	Ok(n > 0)
}

/// A machine's label for alert messages: its name, else its id.
pub fn machine_label(machine: &crate::machines::Machine) -> String {
	match &machine.name {
		Some(n) if !n.is_empty() => n.clone(),
		_ => machine.id.to_string(),
	}
}

/// Whether a group-scoped `(canopy, ref)` issue is currently open + active.
pub(crate) async fn open_group_issue_active(
	db: &mut AsyncPgConnection,
	group_id: Uuid,
	r#ref: &str,
) -> Result<bool> {
	use crate::schema::issues::dsl;
	let n: i64 = dsl::issues
		.filter(dsl::server_group_id.eq(group_id))
		.filter(dsl::source.eq(refs::CANOPY_SOURCE))
		.filter(dsl::ref_.eq(r#ref))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.count()
		.get_result(db)
		.await?;
	Ok(n > 0)
}

/// How an alert message names a server: the name an operator knows it by,
/// qualified with its host when both are known, falling back to the host
/// alone and finally to the id. Shared across every canopy-determined check
/// so they all name applications the same way — never interpolate a bare id.
// spec: BKJ#alerting
pub fn server_label(server: &Application) -> String {
	let host = server.host.as_ref().map(|h| h.0.to_string());
	match (&server.name, host) {
		(Some(n), Some(h)) if !n.is_empty() => format!("{n} ({h})"),
		(Some(n), None) if !n.is_empty() => n.clone(),
		(_, Some(h)) => h,
		(_, None) => server.id.to_string(),
	}
}

/// Render a threshold duration for alert messages via jiff's friendly
/// formatter, balanced to whole days. These are elapsed-time thresholds, not
/// calendar spans, so days are treated as invariant 24h
/// (`SpanRelativeTo::days_are_24_hours`) — no calendar anchor needed. Gives
/// "7d" / "12h" / "7d 12h", which reads better than `SignedDuration`'s
/// hours-capped "168h".
fn fmt_dur(d: SignedDuration) -> String {
	let span = Span::try_from(d)
		.and_then(|s| {
			s.round(
				SpanRound::new()
					.largest(Unit::Day)
					.relative(SpanRelativeTo::days_are_24_hours()),
			)
		})
		.unwrap_or_default();
	format!("{span:#}")
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn fmt_dur_is_friendly_and_day_balanced() {
		assert_eq!(fmt_dur(SignedDuration::from_hours(7 * 24)), "7d");
		assert_eq!(fmt_dur(MAINTENANCE_STALE_AFTER), "8d");
		assert_eq!(fmt_dur(SignedDuration::from_hours(12)), "12h");
		assert_eq!(fmt_dur(SignedDuration::from_hours(7 * 24 + 12)), "7d 12h");
		assert_eq!(fmt_dur(SignedDuration::from_mins(30)), "30m");
	}
}
