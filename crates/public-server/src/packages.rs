//! Device package reporting.
//!
//! Spec: `.workhorse/specs/public-server/packages.md` (id `PKG`).
//!
//! - `POST /machine-packages` — bestool reports the Linux packages installed on
//!   the machine it runs on, so Canopy can match them against published
//!   vulnerabilities.

use axum::{Json, http::StatusCode};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	audit::{Audit, device_actor},
	device_auth::ServerDevice,
};
use database::{
	audit::{Action, Record, RecordKind},
	machines::Machine,
	packages::{MachinePackageReport, PackageReport, Reported, ReportedPackage},
};
use serde::Deserialize;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(public, audit_optional(MachinePackagesReported): report))
}

/// One installed package, by its source package.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MachinePackage {
	/// The source package's name, as `dpkg-query` gives it in `${source:Package}`.
	pub name: String,
	/// The source package's version, as `dpkg-query` gives it in `${source:Version}`.
	pub version: String,
}

/// The packages installed on a machine.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MachinePackagesArgs {
	/// The distribution's identifier, as `/etc/os-release` gives it in `ID`
	/// (e.g. `ubuntu`).
	pub distribution: String,
	/// The distribution's release, as `/etc/os-release` gives it in
	/// `VERSION_ID` (e.g. `24.04`).
	pub release: String,
	/// Every installed package, by source package. The whole list: it replaces
	/// the one before.
	pub packages: Vec<MachinePackage>,
}

/// Report the packages installed on this machine.
///
/// The machine's whole package list, replacing the one before. A report
/// identical to the last one changes nothing. A machine reports when its list
/// changes, not on every status push.
///
/// Canopy places the report in the vulnerability database's ecosystem for the
/// distribution and release. A distribution or release it cannot place is kept
/// and matches nothing.
///
/// Errors: 400 for a report no real system would send (an empty or malformed
/// name, version, distribution or release, or an implausible number of
/// packages); 412 when the calling device is not bound to a live machine.
#[utoipa::path(
	post,
	path = "/machine-packages",
	operation_id = "report_machine_packages",
	tag = "packages",
	security(("server-device" = [])),
	request_body = MachinePackagesArgs,
	responses(
		(status = 204, description = "Packages recorded."),
		(status = 412, description = "Device is not bound to a live machine.", body = ProblemDetailsSchema),
	),
)]
async fn report(
	audit: Audit,
	device: ServerDevice,
	Json(args): Json<MachinePackagesArgs>,
) -> Result<StatusCode> {
	audit.acting_as(device_actor(&device.0.0, None));
	let mut conn = audit.begin().await?;

	let machine = Machine::get_by_device_id(&mut conn, device.0.0.id)
		.await?
		.ok_or(AppError::DeviceHasNoServer)?;

	let report = PackageReport {
		distribution: args.distribution,
		release: args.release,
		packages: args
			.packages
			.into_iter()
			.map(|package| ReportedPackage {
				name: package.name,
				version: package.version,
			})
			.collect(),
	};
	report.validate()?;

	match MachinePackageReport::store(&mut conn, machine.id, &report).await? {
		Reported::Unchanged => audit.unchanged(),
		Reported::Changed { package_count, .. } => {
			conn.record(Action::MachinePackagesReported {
				machine: Record::new(RecordKind::Machine, machine.id, machine.name.clone()),
				distribution: report.distribution,
				release: report.release,
				package_count,
			})
			.await?;
		}
	}
	Ok(StatusCode::NO_CONTENT)
}
