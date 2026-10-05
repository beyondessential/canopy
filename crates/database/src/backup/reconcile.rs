//! Reconcile device **reports** against repo **inventory**: cross-check what
//! devices reported (`backup_runs`) against what inspection found in the repo
//! (`backup_repo_observed_snapshots` for the individual snapshots,
//! `backup_repo_snapshots` for the per-source summary).
//!
//! Per scanned `(server, type)`:
//!
//! - **the reported snapshot is not in the repo** → `backup-reconcile-missing`
//!   (`Warning`, per-server). The run named the snapshot it created and the
//!   repository does not hold it: the case reports alone cannot catch, a device
//!   lying about success or an upload that didn't persist. Detecting it needs
//!   the group's repo inventory, but the finding is about the one server whose
//!   report didn't hold up, so it is filed against that server: two applications in
//!   a group can fail it independently, and one recovering must not clear the
//!   other.
//! - **the repo looks behind the report** → `backup-reconcile-recency`
//!   (recorded only, never alerting). The newest snapshot for the source is
//!   older than the moment the reported run froze its data. It compares
//!   timestamps produced by two independent cadences, so it means "something
//!   looks off" and cannot mean more than that.
//! - **recent snapshot but no report** → `backup-reconcile-report-gap`
//!   (`Warning`, per-server, non-paging). The reporting path is broken, not
//!   the backup.
//! - **neither** → genuinely stale; the staleness scan already owns it, emit
//!   nothing.
//!
//! Independently, per `(server, type)`, the latest run that carries both a
//! device-reported size and an inspection-observed size is compared:
//!
//! - **sizes disagree** (both non-zero) → `backup-reconcile-size-mismatch`
//!   (`Warning`, per-server, non-paging). The device reported a snapshot size
//!   that doesn't match what the repo holds.
//!
//! **Absence is only evidence once someone has looked.** Backups run on one
//! cadence and inspection on a slower, independent one, so "the repo doesn't
//! show it" routinely means "nobody has looked since". Every verdict about a
//! run therefore requires an inspection *newer than that run*; where there
//! isn't one, the instance is [`CheckResult::Skipped`] — neither passed nor
//! failed — and a check with nothing decidable is not filed at all, so an
//! already-open finding is left alone rather than cleared on the strength of a
//! lagging inspector.
//!
//! Two things this cannot see, both because a repository leaves no record of an
//! inspection that found nothing: a group whose repository inspection finds
//! entirely empty, and a group whose snapshots have never been itemised.
//! Corruption and staleness own those.

use std::collections::{HashMap, HashSet};

use commons_errors::Result;
use commons_types::{backup::BackupType, status::CheckResult};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

use crate::{
	backup::{
		refs,
		staleness::{ScanRow, label_list},
	},
	issues::{CheckInstance, CheckOutcome, InstancedCheckFiling, Scope, file_check_instances},
};

/// Slack allowed when comparing a snapshot's recorded time against the moment
/// the run that produced it says it froze its data.
///
/// The two are recorded by the same device from the same clock, and for a
/// backup with nothing to dump beforehand they are the same instant recorded
/// twice — so they can land either side of each other by a rounding or a
/// scheduling hiccup. A real miss is a whole backup interval out, so a few
/// minutes of slack costs nothing and stops a healthy pair being reported on a
/// second's difference.
const RECENCY_TOLERANCE: SignedDuration = SignedDuration::from_mins(5);

/// Snapshot freshness + observed-at for one `(server, type)`, from
/// `backup_repo_snapshots`.
#[derive(Debug, Clone, Copy)]
struct SnapInfo {
	latest_snapshot_at: Option<Timestamp>,
	observed_at: Timestamp,
}

/// Reconcile the scanned set against repo snapshots. Reuses the same
/// [`ScanRow`]s that [`crate::backup::staleness::scan_rows`] produced (the
/// caller passes them in so the staleness scan and reconciliation share one
/// pass). Returns the number of events filed.
// spec: BKJ#detection
pub async fn sweep(db: &mut AsyncPgConnection, rows: &[ScanRow]) -> Result<usize> {
	if rows.is_empty() {
		return Ok(0);
	}
	let now = Timestamp::now();

	// Snapshot info per (machine_id, type) across the scanned groups.
	let group_ids: Vec<Uuid> = {
		let mut s: std::collections::HashSet<Uuid> = rows.iter().map(|r| r.group_id).collect();
		s.drain().collect()
	};
	let snaps = snapshot_info(db, &group_ids).await?;

	// When the group was last inspected, which is what makes a snapshot's
	// absence mean anything. It is a property of the *group's* inspection, not
	// of any one pair's snapshot row: the inspection Job only writes a row for
	// sources it actually finds, so the pair with nothing in the repo — exactly
	// the one a "missing" verdict is about — has no row of its own to date.
	let mut inspected_at: HashMap<Uuid, Timestamp> = HashMap::new();
	for gid in &group_ids {
		if let Some(at) =
			crate::backups::BackupRepoSnapshot::last_inspected_at_for_group(db, *gid).await?
		{
			inspected_at.insert(*gid, at);
		}
	}

	// The itemised snapshots each group's repo holds, with when they were
	// observed. Both halves come from the same table so the evidence and its
	// freshness can never disagree: a group with no rows has no observation to
	// reason from, whatever the per-source summaries say.
	let mut observed_snapshots: HashMap<Uuid, (HashSet<String>, Timestamp)> = HashMap::new();
	for gid in &group_ids {
		let (ids, at) =
			crate::backups::BackupRepoObservedSnapshot::observed_ids_for_group(db, *gid).await?;
		if let Some(at) = at {
			observed_snapshots.insert(*gid, (ids, at));
		}
	}

	// The run behind each pair's `last_success_at`, for the fields the scan
	// doesn't carry: the snapshot id the device named, when it reported, and
	// when it froze its data.
	let mut latest_success: HashMap<(Uuid, BackupType), crate::backups::BackupRun> = HashMap::new();
	for gid in &group_ids {
		latest_success.extend(
			crate::backups::BackupRun::latest_success_by_machine_type_for_group(db, *gid).await?,
		);
	}

	// How each scanned server is named in alert text, resolved in one query
	// rather than per finding.
	let labels: HashMap<Uuid, String> = {
		let ids: Vec<Uuid> = {
			let mut s: std::collections::HashSet<Uuid> =
				rows.iter().map(|r| r.machine_id).collect();
			s.drain().collect()
		};
		crate::machines::Machine::get_many(db, &ids)
			.await?
			.iter()
			.map(|m| (m.id, m.name.clone()))
			.collect()
	};

	// Latest comparable (reported, observed) sizes per (server, type). A server
	// belongs to one group, so keys don't collide across groups.
	let mut sized: HashMap<(Uuid, BackupType), (i64, i64)> = HashMap::new();
	for gid in &group_ids {
		sized.extend(
			crate::backups::BackupRun::latest_sized_by_machine_type_for_group(db, *gid).await?,
		);
	}

	// One check per server, not one per backup type: group the scan by server
	// and grade each type as an instance (see the Names section of the CHK
	// spec).
	let mut by_server: HashMap<Uuid, Vec<&ScanRow>> = HashMap::new();
	let mut order: Vec<Uuid> = Vec::new();
	for row in rows {
		by_server
			.entry(row.machine_id)
			.or_insert_with(|| {
				order.push(row.machine_id);
				Vec::new()
			})
			.push(row);
	}

	let mut filed = 0usize;
	for machine_id in order {
		let server_rows = &by_server[&machine_id];
		let device_id = server_rows.iter().find_map(|r| r.device_id);
		// A scanned server always exists (the scan joins applications), so the
		// placeholder is unreachable in practice. A missed lookup still files:
		// it is only the wording that suffers, never the alert.
		let label = labels
			.get(&machine_id)
			.cloned()
			.unwrap_or_else(|| "Unknown machine".to_string());

		let missing_open = crate::backup::staleness::open_machine_issue_active(
			db,
			machine_id,
			refs::RECONCILE_MISSING,
		)
		.await?;
		let gap_open = crate::backup::staleness::open_machine_issue_active(
			db,
			machine_id,
			refs::RECONCILE_REPORT_GAP,
		)
		.await?;
		let size_open = crate::backup::staleness::open_machine_issue_active(
			db,
			machine_id,
			refs::RECONCILE_SIZE_MISMATCH,
		)
		.await?;
		// The recency check's ceiling holds its effective result at passed, so
		// it is never "active" and there is no open finding to key off. What it
		// leaves behind instead is an observation, which has to be brought up to
		// date once the repo catches up.
		let recency_recorded = crate::backup::staleness::machine_check_observed_degraded(
			db,
			machine_id,
			refs::RECONCILE_RECENCY,
		)
		.await?;

		let mut missing_instances: Vec<CheckInstance> = Vec::with_capacity(server_rows.len());
		let mut recency_instances: Vec<CheckInstance> = Vec::with_capacity(server_rows.len());
		let mut gap_instances: Vec<CheckInstance> = Vec::with_capacity(server_rows.len());
		let mut size_instances: Vec<CheckInstance> = Vec::with_capacity(server_rows.len());
		let mut any_missing = false;
		let mut any_missing_decidable = false;
		let mut any_recency_decidable = false;
		let mut any_behind = false;
		let mut any_gap = false;
		let mut any_size = false;

		for row in server_rows {
			let grace = row.expected_interval.saturating_mul(2);
			let report_fresh = row
				.last_success_at
				.is_some_and(|t| now.duration_since(t) <= grace);
			let snap = snaps.get(&(row.machine_id, row.r#type.clone()));
			let snapshot_fresh = snap
				.and_then(|s| s.latest_snapshot_at)
				.is_some_and(|t| now.duration_since(t) <= grace);
			let run = latest_success.get(&(row.machine_id, row.r#type.clone()));

			// The device named the snapshot it created and the repo doesn't hold
			// it. Skipped unless every condition for the absence to *mean*
			// something holds: the run named a snapshot, it is recent enough
			// that retention cannot have expired that snapshot since, and the
			// repo's snapshots were itemised after the run was reported. The
			// report time is the bound there rather than the moment the data was
			// frozen, because a snapshot is in the repo by the time its run is
			// reported, so an inspection after that point would have seen it.
			//
			// A type with no reported success is skipped too: there is no claim
			// to hold the repo to, so the check didn't run. Staleness and
			// `backup-never` own the server that reports nothing.
			let missing = match run.and_then(|r| r.snapshot_id.as_deref().map(|id| (r, id))) {
				Some((run, id)) if report_fresh => observed_snapshots
					.get(&row.group_id)
					.filter(|(_, at)| *at > run.reported_at)
					.map(|(ids, _)| !ids.contains(id)),
				_ => None,
			};
			any_missing |= missing == Some(true);
			any_missing_decidable |= missing.is_some();
			let mut missing_detail = serde_json::Map::new();
			missing_detail.insert("type".into(), row.r#type.to_string().into());
			if let Some(id) = run.and_then(|r| r.snapshot_id.as_deref()) {
				missing_detail.insert("snapshot_id".into(), id.into());
			}
			missing_instances.push(CheckInstance {
				key: row.r#type.to_string(),
				label: None,
				observed: match missing {
					Some(true) => CheckResult::Warning,
					Some(false) => CheckResult::Passed,
					None => CheckResult::Skipped,
				},
				detail: Some(serde_json::Value::Object(missing_detail)),
			});

			// The repo's newest snapshot for the source is older than the moment
			// the reported run froze its data. Decidable only where the run
			// reported that moment and the source was inspected since the run:
			// the report time is *after* the snapshot was written, so it is no
			// lower bound on how new the repo's newest snapshot should be, and
			// judging against it would call every healthy server behind.
			let behind = run.and_then(|run| {
				run.snapshot_taken_at
					.filter(|_| {
						inspected_at
							.get(&row.group_id)
							.is_some_and(|at| *at > run.reported_at)
					})
					.map(|taken| {
						snap.and_then(|s| s.latest_snapshot_at)
							.is_none_or(|newest| newest < taken - RECENCY_TOLERANCE)
					})
			});
			any_behind |= behind == Some(true);
			any_recency_decidable |= behind.is_some();
			let mut recency_detail = serde_json::Map::new();
			recency_detail.insert("type".into(), row.r#type.to_string().into());
			if let Some(at) = snap.and_then(|s| s.latest_snapshot_at) {
				recency_detail.insert("latest_snapshot_at".into(), at.to_string().into());
			}
			recency_instances.push(CheckInstance {
				key: row.r#type.to_string(),
				label: None,
				observed: match behind {
					Some(true) => CheckResult::Warning,
					Some(false) => CheckResult::Passed,
					None => CheckResult::Skipped,
				},
				detail: Some(serde_json::Value::Object(recency_detail)),
			});

			// Snapshot landed but no recent report → the reporting path is
			// broken, not the backup.
			let gap = !report_fresh && snapshot_fresh;
			any_gap |= gap;
			gap_instances.push(CheckInstance {
				key: row.r#type.to_string(),
				label: None,
				observed: if gap {
					CheckResult::Warning
				} else {
					CheckResult::Passed
				},
				detail: Some(serde_json::json!({
					"type": row.r#type.to_string(),
				})),
			});

			// Size discrepancy: orthogonal to freshness. Compare the latest run
			// that has both a reported and an observed size.
			let sizes = sized.get(&(row.machine_id, row.r#type.clone()));
			let mismatch = sizes.is_some_and(|(reported, observed)| reported != observed);
			any_size |= mismatch;
			let mut size_detail = serde_json::Map::new();
			size_detail.insert("type".into(), row.r#type.to_string().into());
			if let Some(&(reported, observed)) = sizes {
				size_detail.insert("reported_bytes".into(), reported.into());
				size_detail.insert("observed_bytes".into(), observed.into());
			}
			size_instances.push(CheckInstance {
				key: row.r#type.to_string(),
				label: None,
				observed: if mismatch {
					CheckResult::Warning
				} else {
					CheckResult::Passed
				},
				detail: Some(serde_json::Value::Object(size_detail)),
			});
		}

		// Nothing decidable means nothing to conclude: leave whatever state is
		// already there rather than clearing an open finding on the strength of
		// an inspection that predates the runs it would be clearing.
		if any_missing || (missing_open && any_missing_decidable) {
			let total = missing_instances.len();
			file_check_instances(
				db,
				InstancedCheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Machine(machine_id),
					device_id,
					check: refs::RECONCILE_MISSING,
					title: None,
					detail: None,
					outcome: CheckOutcome::Instances(missing_instances),
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::RECONCILE_MISSING_DOC),
				},
				&|graded| match graded.degraded().as_slice() {
					[] => format!("Application {label} backup reports and repo snapshots agree again"),
					[one] => format!(
						"Application {label} reported a successful {} backup but its snapshot is not in the repo",
						one.name()
					),
					many => format!(
						"Application {label} reported {} of its {total} backups successful with snapshots the repo doesn't hold: {}",
						many.len(),
						label_list(many),
					),
				},
			)
			.await?;
			filed += 1;
		}

		// Filed when there is something to say, or something already said that
		// this pass can retract — the same shape as the arms above, keyed on
		// the last observation rather than on an open finding, because this
		// check never has one.
		if any_behind || (recency_recorded && any_recency_decidable) {
			let total = recency_instances.len();
			file_check_instances(
				db,
				InstancedCheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Machine(machine_id),
					device_id,
					check: refs::RECONCILE_RECENCY,
					title: None,
					detail: None,
					outcome: CheckOutcome::Instances(recency_instances),
					// Recorded and visible, never alerting: this compares
					// timestamps from two independent cadences, which supports
					// "the repo looks behind" and not "a backup is missing".
					default_ceiling: CheckResult::Passed,
					default_escalates: false,
					documentation: Some(refs::RECONCILE_RECENCY_DOC),
				},
				&|graded| match graded.degraded().as_slice() {
					[] => format!("Repo snapshots for {label} are as new as the runs it reported"),
					[one] => format!(
						"The repo holds no {} snapshot for {label} as new as the run it reported",
						one.name()
					),
					many => format!(
						"The repo holds no snapshot as new as the reported run for {} of {label}'s {total} backups: {}",
						many.len(),
						label_list(many),
					),
				},
			)
			.await?;
			filed += 1;
		}

		if any_gap || gap_open {
			let total = gap_instances.len();
			file_check_instances(
				db,
				InstancedCheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Machine(machine_id),
					device_id,
					check: refs::RECONCILE_REPORT_GAP,
					title: None,
					detail: None,
					outcome: CheckOutcome::Instances(gap_instances),
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::RECONCILE_REPORT_GAP_DOC),
				},
				&|graded| match graded.degraded().as_slice() {
					[] => format!("Backup reporting for {label} recovered"),
					[one] => format!(
						"A fresh {} repo snapshot exists for {label} but no backup run was reported",
						one.name()
					),
					many => format!(
						"Fresh repo snapshots exist for {} of {label}'s {total} types with no backup run reported: {}",
						many.len(),
						label_list(many),
					),
				},
			)
			.await?;
			filed += 1;
		}

		if any_size || size_open {
			let total = size_instances.len();
			file_check_instances(
				db,
				InstancedCheckFiling {
					source: crate::statuses::CANOPY_SOURCE,
					scope: Scope::Machine(machine_id),
					device_id,
					check: refs::RECONCILE_SIZE_MISMATCH,
					title: None,
					detail: None,
					outcome: CheckOutcome::Instances(size_instances),
					default_ceiling: CheckResult::Warning,
					default_escalates: false,
					documentation: Some(refs::RECONCILE_SIZE_MISMATCH_DOC),
				},
				&|graded| match graded.degraded().as_slice() {
					[] => format!("Reported and repo snapshot sizes for {label} agree again"),
					[one] => format!(
						"Application {label} reported a {} snapshot size that disagrees with the repo",
						one.name()
					),
					many => format!(
						"Application {label} reported snapshot sizes disagreeing with the repo for {} of its {total} types: {}",
						many.len(),
						label_list(many),
					),
				},
			)
			.await?;
			filed += 1;
		}
	}
	Ok(filed)
}

/// Raw snapshot row: `(machine_id, type, latest_snapshot_at, observed_at)`.
type SnapshotRow = (
	Option<Uuid>,
	Option<String>,
	Option<jiff_diesel::Timestamp>,
	jiff_diesel::Timestamp,
);

/// Latest snapshot + observed-at per `(machine_id, type)` for the given groups.
/// Rows with a NULL `machine_id` (sources we can't attribute to a server) are
/// skipped.
async fn snapshot_info(
	db: &mut AsyncPgConnection,
	group_ids: &[Uuid],
) -> Result<HashMap<(Uuid, BackupType), SnapInfo>> {
	use crate::schema::backup_repo_snapshots as s;

	if group_ids.is_empty() {
		return Ok(HashMap::new());
	}

	let rows: Vec<SnapshotRow> = s::table
		.filter(s::group_id.eq_any(group_ids))
		.filter(s::machine_id.is_not_null())
		.select((
			s::machine_id,
			s::type_,
			s::latest_snapshot_at,
			s::observed_at,
		))
		.load(db)
		.await?;

	let mut out: HashMap<(Uuid, BackupType), SnapInfo> = HashMap::new();
	for (machine_id, ty, latest, observed) in rows {
		let (Some(machine_id), Some(ty)) = (machine_id, ty) else {
			continue;
		};
		let info = SnapInfo {
			latest_snapshot_at: latest.map(Timestamp::from),
			observed_at: Timestamp::from(observed),
		};
		out.entry((machine_id, BackupType::from(ty)))
			.and_modify(|e| {
				// Keep the freshest snapshot row for the pair.
				if info.observed_at > e.observed_at {
					*e = info;
				}
			})
			.or_insert(info);
	}
	Ok(out)
}
