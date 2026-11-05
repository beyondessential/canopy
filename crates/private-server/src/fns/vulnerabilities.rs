//! The vulnerabilities page: what the production fleet is exposed to, and the
//! triage decisions taken on it.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`) and
//! `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).

use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use database::{
	audit::Actor,
	osv::Severity,
	packages::MachinePackageReport,
	vulnerabilities::{
		Decision, ItemDetail, ItemSummary, ListFilter, Listing, NewDecision, Outcome, Source,
		Status, Via, today,
	},
};
use jiff::civil::Date;
use serde::Deserialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: list))
		.routes(routes!(read_only, silent: get))
		.routes(routes!(read_only, silent: machine_packages))
		.routes(routes!(write, audit(VulnerabilityDecided): decide))
}

/// Which items to list.
#[derive(Debug, Default, Deserialize, ToSchema)]
pub struct VulnerabilityListArgs {
	/// Only items of this source.
	pub source: Option<Source>,
	/// Only items with this status. Omitted, every status.
	pub status: Option<Status>,
	/// Only items at or above this severity.
	pub min_severity: Option<Severity>,
	/// Include items nothing counts for any more.
	#[serde(default)]
	pub include_uncounted: bool,
}

/// List vulnerabilities.
///
/// One entry per advisory within a source, sorted by severity and then by how
/// much each exposes. Items nothing counts for any more are left out unless
/// asked for.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "vulnerabilities_list",
	tag = "vulnerabilities",
	security(("tailscale-user" = [])),
	request_body = VulnerabilityListArgs,
	responses(
		(status = 200, body = Listing),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<VulnerabilityListArgs>,
) -> Result<Json<Listing>> {
	let mut conn = state.db.get().await?;
	let listing = ItemSummary::list(
		&mut conn,
		&ListFilter {
			source: args.source,
			status: args.status,
			min_severity: args.min_severity,
			include_uncounted: args.include_uncounted,
			limit: None,
		},
		today(),
	)
	.await?;
	Ok(Json(listing))
}

/// Which item.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VulnerabilityItemArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The source the item belongs to.
	pub source: Source,
}

/// One vulnerability in full.
///
/// The advisory, what it exposes, and the item's history of decisions,
/// reopenings and the model's suggestion.
#[utoipa::path(
	post,
	path = "/get",
	operation_id = "vulnerabilities_get",
	tag = "vulnerabilities",
	security(("tailscale-user" = [])),
	request_body = VulnerabilityItemArgs,
	responses(
		(status = 200, body = ItemDetail),
		(status = 401, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<VulnerabilityItemArgs>,
) -> Result<Json<ItemDetail>> {
	let mut conn = state.db.get().await?;
	Ok(Json(
		ItemDetail::load(&mut conn, &args.id, args.source, today()).await?,
	))
}

/// A decision to record.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VulnerabilityDecideArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The source the decision covers.
	pub source: Source,
	/// What it decides.
	pub outcome: Outcome,
	/// Why. Required for not affected and accepted risk.
	pub reason: Option<String>,
	/// When to look at an accepted risk again, `YYYY-MM-DD`. Required for
	/// accepted risk, and in the future.
	pub review_by: Option<String>,
	/// A link to the issue tracking a fix, for will fix.
	pub issue_url: Option<String>,
}

/// Record a triage decision.
///
/// Becomes the item's current decision, covering its whole source. The one it
/// replaces stays in the history.
#[utoipa::path(
	post,
	path = "/decide",
	operation_id = "vulnerabilities_decide",
	tag = "vulnerabilities",
	security(("tailscale-admin" = [])),
	request_body = VulnerabilityDecideArgs,
	responses(
		(status = 200, body = Decision),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn decide(
	audit: Audit,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<VulnerabilityDecideArgs>,
) -> Result<Json<Decision>> {
	let review_by: Option<Date> = args
		.review_by
		.as_deref()
		.filter(|date| !date.trim().is_empty())
		.map(|date| {
			date.trim()
				.parse()
				.map_err(|_| AppError::BadRequest(format!("{date:?} is not a date (YYYY-MM-DD)")))
		})
		.transpose()?;

	let mut conn = audit.begin().await?;
	let decision = Decision::record(
		&mut conn,
		NewDecision {
			vulnerability_id: args.id.clone(),
			source: args.source,
			outcome: args.outcome,
			reason: args.reason,
			review_by,
			issue_url: args.issue_url,
			ground: None,
			actor: Actor::operator(admin.login, Some(admin.name)),
			via: Via::Ui,
		},
		today(),
	)
	.await?;
	conn.record(decision.audited()).await?;
	Ok(Json(decision))
}

/// Which machine.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VulnerabilityMachineArgs {
	/// The machine to read.
	pub machine_id: Uuid,
}

/// When a machine last reported its packages, and how many it reported.
///
/// `null` for a machine that has never reported. A report whose ecosystem is
/// `null` could not be placed, so nothing on that machine is being checked.
#[utoipa::path(
	post,
	path = "/machine_packages",
	operation_id = "vulnerabilities_machine_packages",
	tag = "vulnerabilities",
	security(("tailscale-user" = [])),
	request_body = VulnerabilityMachineArgs,
	responses(
		(status = 200, body = Option<MachinePackageReport>),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn machine_packages(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<VulnerabilityMachineArgs>,
) -> Result<Json<Option<MachinePackageReport>>> {
	let mut conn = state.db.get().await?;
	Ok(Json(
		MachinePackageReport::get(&mut conn, args.machine_id).await?,
	))
}
