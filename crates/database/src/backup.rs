//! Backup-credentials detection / alerting (DB-driven half of the control
//! plane). The persistent models live in [`crate::backups`]; this module owns
//! the periodic *logic*: staleness and report-vs-inventory reconciliation.
//!
//! - [`refs`] — the stable `(source, ref)` alert keys (a contract).
//! - [`staleness`] — staleness scan over reported runs + maintenance staleness.
//! - [`reconcile`] — reconcile device reports against repo inventory.
//!
//! Alerting goes through [`crate::issues::file_check`], which grades
//! each observation through the operator's check policy and bypasses the
//! per-server `is_monitored` gate for group-scoped filings. The preflight
//! (AWS-touching) lives in the `jobs` crate binary, not here — the
//! `database` crate must not gain an AWS dependency — but its alerting uses
//! the same path.

pub mod reconcile;
pub mod refs;
pub mod staleness;

use commons_errors::Result;
use diesel_async::AsyncPgConnection;

/// Run both DB-driven checks in one pass off a single scan: staleness (+
/// maintenance staleness) then report-vs-inventory reconciliation. Called each
/// tick from the `reachability` loop (it already runs minute-cadence DB-only
/// sweeps). Returns the total number of events filed.
pub async fn sweep(db: &mut AsyncPgConnection) -> Result<usize> {
	let rows = staleness::scan_rows(db).await?;
	let mut filed = staleness::sweep(db, &rows).await?;
	filed += reconcile::sweep(db, &rows).await?;
	filed += crate::restore::sweep_restore_checks(db).await?;
	// Not counted in `filed`: this one files per group rather than per event,
	// and its own instances are pairs.
	crate::reporting_schemas::sweep(db).await?;
	// Not an event, but the same cadence: a plan closes once its group reports
	// the target, and this sweep is what notices.
	crate::upgrade_plans::close_met_plans(db).await?;
	Ok(filed)
}
