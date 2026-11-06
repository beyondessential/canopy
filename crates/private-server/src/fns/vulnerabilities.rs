//! The vulnerabilities page: what the production fleet is exposed to, and the
//! triage decisions taken on it.
//!
//! Spec: `.workhorse/specs/security/vulnerabilities.md` (id `VUL`) and
//! `.workhorse/specs/security/vulnerability-triage.md` (id `VTR`).

use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use database::{
	audit::Actor,
	packages::MachinePackageReport,
	vulnerabilities::{
		Decision, Edit, Entry, Facets, ItemDetail, ItemSummary, ListQuery, NewDecision, Source,
		Via, Withdrawal,
	},
};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: list))
		.routes(routes!(read_only, silent: count))
		.routes(routes!(read_only, silent: get))
		.routes(routes!(read_only, silent: machine_packages))
		.routes(routes!(write, audit(VulnerabilityDecided): decide))
		.routes(routes!(write, audit(VulnerabilityDecisionWithdrawn): withdraw))
		.routes(routes!(write, audit(VulnerabilityDecisionEdited): edit))
}

/// A page of items, with the choices of every filter and how many items each
/// would give.
#[derive(Debug, Serialize, ToSchema)]
pub struct VulnerabilityListResponse {
	/// The items of the page, in order.
	pub items: Vec<ItemSummary>,
	/// How many items match in all.
	pub total: i64,
	/// The cursor of the next page, absent on the last.
	pub next_cursor: Option<String>,
	/// The choices of each filter, each counted without its own filter.
	pub facets: Facets,
}

/// List vulnerabilities.
///
/// One entry per advisory within a source. Filters, search, sort and page are
/// all of the query. Closed items are left out unless asked for.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "vulnerabilities_list",
	tag = "vulnerabilities",
	security(("tailscale-user" = [])),
	request_body = ListQuery,
	responses(
		(status = 200, body = VulnerabilityListResponse),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(query): Json<ListQuery>,
) -> Result<Json<VulnerabilityListResponse>> {
	let mut conn = state.db.get().await?;
	let listing = ItemSummary::list(&mut conn, &query).await?;
	let facets = ItemSummary::facets(&mut conn, &query).await?;
	Ok(Json(VulnerabilityListResponse {
		items: listing.items,
		total: listing.total,
		next_cursor: listing.next_cursor,
		facets,
	}))
}

/// How many items need triage.
#[derive(Debug, Serialize, ToSchema)]
pub struct VulnerabilityCount {
	/// Items that need triage, closed ones left out.
	pub needs_triage: i64,
}

/// How many vulnerabilities need triage.
///
/// For the badge of the sidebar entry.
#[utoipa::path(
	post,
	path = "/count",
	operation_id = "vulnerabilities_count",
	tag = "vulnerabilities",
	security(("tailscale-user" = [])),
	request_body = serde_json::Value,
	responses(
		(status = 200, body = VulnerabilityCount),
		(status = 401, body = ProblemDetailsSchema),
	),
)]
pub async fn count(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<VulnerabilityCount>> {
	let mut conn = state.db.get().await?;
	Ok(Json(VulnerabilityCount {
		needs_triage: ItemSummary::needs_triage_count(&mut conn).await?,
	}))
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
/// The advisory, what it exposes, and the item's history of decisions, edits,
/// withdrawals, lapses and the model's suggestion.
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
	Ok(Json(ItemDetail::load(&mut conn, &args.id, &args.source).await?))
}

/// A decision to record.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VulnerabilityDecideArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The source the decision covers.
	pub source: Source,
	/// What it decides. A whole decision is one entry with no workspace
	/// packages named; a split is entries naming workspace packages, optionally
	/// ending in a catch-all, and covers every workspace package that reaches
	/// the advisory.
	pub entries: Vec<Entry>,
}

/// Record a triage decision.
///
/// Becomes the item's current decision. The one it replaces stays in the
/// history.
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
	let mut conn = audit.begin().await?;
	let decision = Decision::record(
		&mut conn,
		NewDecision {
			vulnerability_id: args.id.clone(),
			source: args.source,
			entries: args.entries,
			actor: Actor::operator(admin.login, Some(admin.name)),
			via: Via::Ui,
		},
	)
	.await?;
	conn.record(decision.audited()).await?;
	Ok(Json(decision))
}

/// A decision to withdraw.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VulnerabilityWithdrawArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The source of the item.
	pub source: Source,
	/// Why the decision is taken away.
	pub reason: String,
}

/// Withdraw an item's current decision.
///
/// The item needs triage again, with the suggestion it had.
#[utoipa::path(
	post,
	path = "/withdraw",
	operation_id = "vulnerabilities_withdraw",
	tag = "vulnerabilities",
	security(("tailscale-admin" = [])),
	request_body = VulnerabilityWithdrawArgs,
	responses(
		(status = 200, body = Withdrawal),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn withdraw(
	audit: Audit,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<VulnerabilityWithdrawArgs>,
) -> Result<Json<Withdrawal>> {
	let mut conn = audit.begin().await?;
	let withdrawal = Decision::withdraw(
		&mut conn,
		&args.id,
		&args.source,
		&args.reason,
		Actor::operator(admin.login, Some(admin.name)),
		Via::Ui,
	)
	.await?;
	conn.record(withdrawal.audited()).await?;
	Ok(Json(withdrawal))
}

/// An edit to the reason or issue link of a decision's entry.
#[derive(Debug, Deserialize, ToSchema)]
pub struct VulnerabilityEditArgs {
	/// The advisory's OSV identifier.
	pub id: String,
	/// The source of the item.
	pub source: Source,
	/// A workspace package the entry covers. Absent for the catch-all.
	pub workspace_package: Option<String>,
	/// The corrected reason. Absent leaves it as it is.
	pub reason: Option<String>,
	/// The corrected issue link, blank to clear it. Absent leaves it as it is.
	pub issue_url: Option<String>,
}

/// Edit an entry of an item's current decision.
///
/// Corrects its reason or issue link, never its outcome or which packages it
/// covers. The history keeps the earlier text.
#[utoipa::path(
	post,
	path = "/edit",
	operation_id = "vulnerabilities_edit",
	tag = "vulnerabilities",
	security(("tailscale-admin" = [])),
	request_body = VulnerabilityEditArgs,
	responses(
		(status = 200, body = Edit),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn edit(
	audit: Audit,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<VulnerabilityEditArgs>,
) -> Result<Json<Edit>> {
	let mut conn = audit.begin().await?;
	let edit = Decision::edit(
		&mut conn,
		&args.id,
		&args.source,
		args.workspace_package.as_deref(),
		args.reason.as_deref(),
		args.issue_url.as_deref(),
		Actor::operator(admin.login, Some(admin.name)),
		Via::Ui,
	)
	.await?;
	conn.record(edit.audited()).await?;
	Ok(Json(edit))
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
