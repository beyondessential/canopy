use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use database::migration_tests::GroupVerdict;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(for_group))
}

/// Request body for reading a group's migration-test verdicts.
#[derive(Deserialize, ToSchema)]
pub struct ForGroupArgs {
	/// The group to report on.
	pub group_id: Uuid,
}

/// Where each of a group's applications stands against the version it would take
/// next.
///
/// One entry per server that has a candidate version, which is the version its
/// group's open plan names. A group with no plan, and a server running another
/// product, have nothing to be tested against and are absent.
// spec: RST#verdicts
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "migration_tests_for_group",
	tag = "migration_tests",
	security(("tailscale-admin" = [])),
	request_body = ForGroupArgs,
	responses(
		(status = 200, description = "Verdicts, one per server with a candidate.", body = Vec<GroupVerdict>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ForGroupArgs>,
) -> Result<Json<Vec<GroupVerdict>>> {
	let mut conn = state.db.get().await?;
	let verdicts = database::migration_tests::verdicts_for_group(&mut conn, args.group_id).await?;
	Ok(Json(verdicts))
}

/// Whether a restore attempt is under way for a group, for showing beside a
/// verdict.
///
/// Group-level, because credentials are issued per `(group, type)`: RST scopes
/// them that way since one repo holds all of a group's snapshots, so this is the
/// finest granularity the signal honestly has.
// spec: RST#verdicts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AttemptState {
	/// Credentials were issued and have not expired with no report yet.
	InFlight,
	/// Credentials expired with no report: it ran and never said how it went.
	EndedWithoutReport,
}

/// The state of the newest unreported restore attempt for `group_id`, if any.
///
/// A restore takes hours, so without this a group mid-test reads as untested for
/// the whole window and a consumer that has stopped looks the same as one that
/// never started.
// spec: RST#verdicts
pub async fn attempt_state(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Uuid,
	now: jiff::Timestamp,
) -> Result<Option<AttemptState>> {
	use database::backups::BackupCredentialIssuance;

	let reports =
		database::restore::BackupRestoreCheck::list_recent_for_group(conn, group_id, 50).await?;
	// Member-box devices restore for their own purposes (clone refreshes,
	// manual restores) and never report; only consumer issuances speak for the
	// pipeline. Same filter as the restore-activity view. An identity belongs to
	// a box, so the members are the group's machines.
	let member_devices: Vec<Uuid> = database::machines::Machine::list_for_group(conn, group_id)
		.await?
		.into_iter()
		.filter_map(|m| m.device_id)
		.collect();
	let since =
		crate::run_pairing::issuance_since(now, reports.iter().map(|c| c.reported_at).min());
	let issuances: Vec<_> =
		BackupCredentialIssuance::list_for_group_since(conn, group_id, since, 200)
			.await?
			.into_iter()
			.filter(|i| {
				i.purpose == commons_types::backup::BackupPurpose::Restore
					&& !member_devices.contains(&i.device_id)
			})
			.collect();

	let report_refs: Vec<crate::run_pairing::ReportRef> = reports
		.iter()
		.map(|c| crate::run_pairing::ReportRef {
			run_id: c.run_id,
			key: crate::run_pairing::run_key(
				c.consumer_device_id,
				&c.r#type,
				commons_types::backup::BackupPurpose::Restore,
			),
			reported_at: c.reported_at,
		})
		.collect();

	Ok(state_from_issuances(issuances, &report_refs, now))
}

/// The state of the newest unreported attempt among `issuances`, given the
/// reports that have arrived.
///
/// A chain whose first issuance predates the newest report is history: something
/// has reported since, so the chain says nothing about where the pipeline is
/// now, however long its credentials were re-minted for.
fn state_from_issuances(
	issuances: Vec<database::backups::BackupCredentialIssuance>,
	report_refs: &[crate::run_pairing::ReportRef],
	now: jiff::Timestamp,
) -> Option<AttemptState> {
	let (_starts, attempts) = crate::run_pairing::pair_issuances(issuances, report_refs);

	let newest_report = report_refs.iter().map(|r| r.reported_at).max();
	let attempts: Vec<_> = attempts
		.into_iter()
		.filter(|a| newest_report.is_none_or(|newest| a.first.issued_at > newest))
		.collect();

	// An in-flight attempt is the more useful of the two to report, since it
	// says the pipeline is working right now.
	let in_flight = attempts
		.iter()
		.any(|a| matches!(a.status(now), crate::run_pairing::RunStatus::InProgress));
	if in_flight {
		return Some(AttemptState::InFlight);
	}
	if attempts.is_empty() {
		return None;
	}
	Some(AttemptState::EndedWithoutReport)
}

#[cfg(test)]
mod tests {
	use commons_types::backup::{BackupPurpose, BackupType};
	use database::backups::BackupCredentialIssuance;
	use jiff::Timestamp;
	use uuid::Uuid;

	use super::*;

	fn ts(secs: i64) -> Timestamp {
		Timestamp::from_second(secs).unwrap()
	}

	fn issuance(id: i64, issued: i64, expires: i64) -> BackupCredentialIssuance {
		BackupCredentialIssuance {
			id,
			device_id: Uuid::from_u128(1),
			group_id: Uuid::from_u128(2),
			r#type: BackupType::TamanuPostgres,
			issued_at: ts(issued),
			expires_at: ts(expires),
			purpose: BackupPurpose::Restore,
			sts_assumed_role: String::new(),
			sts_request_id: None,
			access_key_id: None,
			bucket: String::new(),
			prefix: String::new(),
			run_id: None,
		}
	}

	fn report(reported: i64) -> crate::run_pairing::ReportRef {
		crate::run_pairing::ReportRef {
			run_id: Some(Uuid::from_u128(9)),
			key: crate::run_pairing::run_key(
				Uuid::from_u128(3),
				&BackupType::TamanuPostgres,
				BackupPurpose::Restore,
			),
			reported_at: ts(reported),
		}
	}

	#[test]
	fn chain_started_before_the_newest_report_is_history() {
		// Re-minting keeps pushing the chain's expiry past the report that landed.
		let issuances = vec![
			issuance(1, 1_000, 4_600),
			issuance(2, 1_030, 4_630),
			issuance(3, 1_060, 4_660),
		];
		assert_eq!(
			state_from_issuances(issuances, &[report(2_000)], ts(3_000)),
			None
		);
	}

	#[test]
	fn chain_started_after_the_newest_report_and_still_valid_is_in_flight() {
		let issuances = vec![issuance(1, 2_500, 6_100)];
		assert_eq!(
			state_from_issuances(issuances, &[report(2_000)], ts(3_000)),
			Some(AttemptState::InFlight)
		);
	}

	#[test]
	fn chain_started_after_the_newest_report_and_expired_ended_without_report() {
		let issuances = vec![issuance(1, 2_500, 2_900)];
		assert_eq!(
			state_from_issuances(issuances, &[report(2_000)], ts(3_000)),
			Some(AttemptState::EndedWithoutReport)
		);
	}
}
