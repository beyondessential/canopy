//! Operator-owned catalog of check policies per (source, check). Read
//! and edit endpoints for the catalog page at /healthchecks. Ingestion
//! (in the public-server status handler) maintains the rows; this
//! module exposes them to admins.

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::namespace::{Namespace, NamespaceRef, is_reserved};
use commons_types::source::{IngestMode, ReachabilityMode};
use commons_types::status::CheckResult;
use database::applications::Application;
use database::check_policies::{CheckPolicy, IfLadder};
use database::source_policies::SourcePolicy;
use database::statuses::Status;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use std::collections::HashMap;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: list))
		.routes(routes!(read_only: sources))
		.routes(routes!(write: set_source_reachability))
		.routes(routes!(danger: set_source_ingest))
		.routes(routes!(write: update))
		.routes(routes!(danger: decommission))
		.routes(routes!(write: update_rules))
		.routes(routes!(write: update_documentation))
		.routes(routes!(read_only: sample))
		.routes(routes!(read_only: tag_keys))
}

/// One (source, check)'s policy: the ceiling capping its effective
/// result, the escalation flag, plus optional conditional rules that can
/// grade a given report differently.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CheckPolicyData {
	/// The source that reports this check.
	pub source: String,
	/// The healthcheck's name, exactly as reported by monitored applications.
	/// Two application types reporting this name are two entries, told apart
	/// by `namespace`.
	pub check_name: String,
	/// Which catalog entry this is: the box's, one application type's, or a
	/// curated source's unqualified one. Send it back verbatim to name this
	/// entry in an edit.
	pub namespace: NamespaceRef,
	/// How the entry reads to an operator: `<type>.<check>` where it is one
	/// application type's, the bare name otherwise.
	pub qualified_name: String,
	/// The maximum effective result for this check when no conditional
	/// rule (see `rules`) overrides it: an observed result more urgent
	/// than the ceiling grades down to it. One of `failed`, `warning`,
	/// `passed`, or `skipped` (`skipped` also tells the reporting source
	/// it may stop running the check).
	#[schema(value_type = String)]
	pub ceiling: CheckResult,
	/// Whether an effective failure of this check notifies immediately,
	/// bypassing the incident grace period.
	pub escalates: bool,
	/// When this check was first reported and this policy entry was
	/// created.
	pub first_seen: Timestamp,
	/// When an operator last reviewed or updated this policy. `null` if
	/// it has never been reviewed.
	pub reviewed_at: Option<Timestamp>,
	/// The operator who last reviewed this policy. `null` if it has
	/// never been reviewed.
	pub reviewed_by: Option<String>,
	/// Free-form operator notes about this check.
	pub notes: Option<String>,
	/// When this policy was last modified.
	pub updated_at: Timestamp,
	/// Operator-authored documentation for this check: a single markdown
	/// document. By convention it covers what the check observes, what
	/// each result means, and hints for solving a failure, but no
	/// structure is enforced. `null` when nobody has documented it yet.
	pub documentation: Option<String>,
	/// `true` if no operator has reviewed this policy yet.
	pub pending_review: bool,
	/// When this check was most recently reported on any server across the
	/// fleet. `null` until liveness has been reconciled since it first
	/// appeared. A check quiet for long enough is a decommissioning
	/// candidate.
	pub last_seen: Option<Timestamp>,
	/// When this check was decommissioned, if it has been. A decommissioned
	/// check contributes to nothing until it is reported again. `null` while
	/// live.
	pub decommissioned_at: Option<Timestamp>,
	/// Conditional rules that can grade a report to a different result
	/// than the ceiling would — in any direction — depending on the
	/// check's own fields, the surrounding status report, or the
	/// reporting server's tags. `null` means no conditional rules are
	/// configured, and the ceiling always applies.
	///
	/// When present, this is a single-key object shaped like
	/// `{"if": [condition_1, result_1, condition_2, result_2, ...]}`.
	/// Conditions are tried in order, and the result paired with the
	/// first matching condition is used; if none match, the observed
	/// result capped at the ceiling is used instead. There's no explicit
	/// "else" branch — the ceiling fallback plays that role — so the
	/// array must have an even number of entries and at least one pair.
	///
	/// Each condition is a single-key object naming a comparison
	/// operator — one of `==`, `!=`, `<`, `<=`, `>`, `>=`, or `in_range`
	/// — whose value is a two-element array: a variable reference and a
	/// value to compare it against. A variable reference has the shape
	/// `{"var": "<namespace>.<field>"}`, where `<namespace>` is one of
	/// `check` (a field on the failing check itself), `status` (a
	/// top-level field on the status report that contained it), or
	/// `tag` (a tag on the reporting server, merged with its group's
	/// tags). `in_range` compares a version-like string against a
	/// semantic version range (e.g. `">=1.2.0 <2.0.0"`). If the named
	/// variable isn't present in the data being evaluated, the condition
	/// doesn't match. A `rules` value that doesn't parse into this shape
	/// is treated the same as `null` (no conditional rules).
	#[schema(value_type = Option<serde_json::Value>)]
	pub rules: Option<JsonValue>,
	/// Number of condition/result branches in `rules`; `0` when
	/// `rules` is `null` or couldn't be parsed. Lets a caller tell
	/// whether conditional rules exist without parsing `rules` itself.
	pub rule_count: u32,
}

/// The namespace a request names, refused if the pair is not one of the three
/// shapes a catalog entry can have. A request naming a shape the schema will
/// not store is a mistake to report, not one to interpret.
fn namespace_of(r: &NamespaceRef) -> Result<Namespace> {
	Namespace::try_from(r).map_err(|e| AppError::BadRequest(e.to_string()))
}

fn rule_count(rules: &Option<JsonValue>) -> u32 {
	let Some(v) = rules else { return 0 };
	serde_json::from_value::<IfLadder>(v.clone())
		.map(|l| l.branches.len() as u32)
		.unwrap_or(0)
}

impl From<CheckPolicy> for CheckPolicyData {
	fn from(h: CheckPolicy) -> Self {
		let pending_review = h.reviewed_at.is_none();
		let rule_count = rule_count(&h.rules);
		let qualified_name = h.qualified_name();
		let namespace = NamespaceRef {
			subject: h.subject,
			application_type: h.application_type,
		};
		Self {
			source: h.source,
			check_name: h.check_name,
			namespace,
			qualified_name,
			ceiling: h.ceiling,
			escalates: h.escalates,
			first_seen: h.first_seen,
			reviewed_at: h.reviewed_at,
			reviewed_by: h.reviewed_by,
			notes: h.notes,
			updated_at: h.updated_at,
			documentation: h.documentation,
			pending_review,
			last_seen: h.last_seen,
			decommissioned_at: h.decommissioned_at,
			rules: h.rules,
			rule_count,
		}
	}
}

/// List the check policy catalog.
///
/// Returns every known (source, check) together with its current policy,
/// ordered by source then name. An entry exists for every check any
/// source has ever reported; new checks are added automatically the
/// first time they're seen, with a default policy pending review.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "healthcheck_list",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "Catalog rows ordered by source then check_name.", body = Vec<CheckPolicyData>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<CheckPolicyData>>> {
	let mut conn = state.db.get().await?;
	let rows = CheckPolicy::list(&mut conn).await?;
	Ok(Json(rows.into_iter().map(Into::into).collect()))
}

/// One reporting source with its reachability policy and fleet-wide
/// last-seen.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct SourceData {
	/// The source name, as reported by devices.
	pub source: String,
	/// How this source's silence bears on its applications' reachability: `on`
	/// (a stale source warns, all-stale is unreachable), `quiet` (never
	/// warns, still counts toward unreachable), or `off` (excluded).
	pub reachability: ReachabilityMode,
	/// Whether the device API ingests this source's reports: `allow`,
	/// `ignore` (accepted but discarded), or `deny` (rejected). A source
	/// that isn't ingested is excluded from reachability.
	pub ingest: IngestMode,
	/// When any of this source's checks was most recently reported anywhere
	/// in the fleet. `null` until liveness has been reconciled.
	pub last_seen: Option<Timestamp>,
}

/// List the reporting sources and their reachability policy.
///
/// Every non-reserved source that has catalogued checks, with its
/// reachability mode (defaulting to `on`) and most recent fleet-wide
/// report. The reserved sources are excluded.
#[utoipa::path(
	post,
	path = "/sources",
	operation_id = "healthcheck_sources",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "Sources ordered by name.", body = Vec<SourceData>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn sources(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<SourceData>>> {
	let mut conn = state.db.get().await?;
	let rows = SourcePolicy::list_sources(&mut conn).await?;
	Ok(Json(
		rows.into_iter()
			.map(|s| SourceData {
				source: s.source,
				reachability: s.reachability,
				ingest: s.ingest,
				last_seen: s.last_seen,
			})
			.collect(),
	))
}

/// Request body for setting a source's reachability mode.
#[derive(Deserialize, ToSchema)]
pub struct SetSourceReachabilityArgs {
	/// The source to configure. The reserved names (`canopy`, `manual`, `kubernetes`) are
	/// rejected.
	pub source: String,
	/// The reachability mode to apply: `on`, `quiet`, or `off`.
	pub reachability: ReachabilityMode,
}

/// Set a source's reachability mode.
///
/// Governs how the source's silence bears on its applications' reachability:
/// `on` warns, `quiet` never warns but still counts toward unreachable,
/// `off` is excluded. The reserved names (`canopy`, `manual`, `kubernetes`) are rejected.
#[utoipa::path(
	post,
	path = "/set_source_reachability",
	operation_id = "healthcheck_set_source_reachability",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = SetSourceReachabilityArgs,
	responses(
		(status = 200, description = "Reachability mode set."),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn set_source_reachability(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SetSourceReachabilityArgs>,
) -> Result<Json<()>> {
	if is_reserved(&args.source) {
		return Err(AppError::BadRequest(
			"the reserved sources have no reachability policy".into(),
		));
	}
	let mut conn = state.db.get().await?;
	SourcePolicy::set_reachability(&mut conn, &args.source, args.reachability).await?;
	Ok(Json(()))
}

/// Request body for setting a source's ingest mode.
#[derive(Deserialize, ToSchema)]
pub struct SetSourceIngestArgs {
	/// The source to configure. The reserved names (`canopy`, `manual`, `kubernetes`) are
	/// rejected.
	pub source: String,
	/// The ingest mode to apply: `allow`, `ignore`, or `deny`.
	pub ingest: IngestMode,
}

/// Set a source's ingest mode.
///
/// Governs whether the device API accepts the source's reports: `allow`
/// ingests normally, `ignore` accepts but discards them, `deny` rejects
/// the push. The reserved names (`canopy`, `manual`, `kubernetes`) are rejected.
#[utoipa::path(
	post,
	path = "/set_source_ingest",
	operation_id = "healthcheck_set_source_ingest",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = SetSourceIngestArgs,
	responses(
		(status = 200, description = "Ingest mode set."),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn set_source_ingest(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SetSourceIngestArgs>,
) -> Result<Json<()>> {
	if is_reserved(&args.source) {
		return Err(AppError::BadRequest(
			"the reserved sources have no ingest policy".into(),
		));
	}
	let mut conn = state.db.get().await?;
	SourcePolicy::set_ingest(&mut conn, &args.source, args.ingest).await?;
	Ok(Json(()))
}

/// Request body for updating a check's base policy.
#[derive(Deserialize, ToSchema)]
pub struct HealthcheckUpdateArgs {
	/// The source whose check to update.
	pub source: String,
	/// The healthcheck name to update; must already exist in the
	/// catalog.
	pub check_name: String,
	/// Which catalog entry is meant, as returned in the entry's `namespace`.
	/// Omitted is the unqualified namespace a curated source's checks live in.
	#[serde(default)]
	pub namespace: NamespaceRef,
	/// The ceiling to apply to this check's observed results when no
	/// conditional rule overrides it: one of `failed`, `warning`,
	/// `passed`, or `skipped`.
	#[schema(value_type = String)]
	pub ceiling: CheckResult,
	/// Whether an effective failure of this check should notify
	/// immediately, bypassing the incident grace period.
	#[serde(default)]
	pub escalates: bool,
	/// Operator notes to store alongside the new policy. Omitting this
	/// or sending `null` clears any existing notes — there's no way to
	/// leave them unchanged implicitly, so resend the current value to
	/// keep it.
	#[serde(default)]
	pub notes: Option<String>,
}

/// Update a check's policy.
///
/// Sets the ceiling and escalation flag (and optionally notes) for the
/// given (source, check), and marks it as reviewed by the caller. Saving
/// with the same values as before still counts as a review, so an
/// operator can acknowledge a check without changing its policy.
#[utoipa::path(
	post,
	path = "/update",
	operation_id = "healthcheck_update",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = HealthcheckUpdateArgs,
	responses(
		(status = 200, description = "Updated catalog row.", body = CheckPolicyData),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn update(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<HealthcheckUpdateArgs>,
) -> Result<Json<CheckPolicyData>> {
	if !matches!(
		args.ceiling,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Passed | CheckResult::Skipped
	) {
		return Err(AppError::BadRequest(
			"ceiling must be one of failed, warning, passed, skipped".into(),
		));
	}
	let mut conn = state.db.get().await?;
	let row = CheckPolicy::update(
		&mut conn,
		&args.source,
		&namespace_of(&args.namespace)?,
		&args.check_name,
		args.ceiling,
		args.escalates,
		args.notes.as_deref(),
		&admin.0.login,
	)
	.await?;
	Ok(Json(row.into()))
}

/// Request body for decommissioning a check.
#[derive(Deserialize, ToSchema)]
pub struct DecommissionArgs {
	/// The source whose check to decommission.
	pub source: String,
	/// The healthcheck name to decommission; must already exist in the
	/// catalog.
	pub check_name: String,
	/// Which catalog entry is meant, as returned in the entry's `namespace`.
	/// Omitted is the unqualified namespace a curated source's checks live in.
	#[serde(default)]
	pub namespace: NamespaceRef,
}

/// Decommission a check fleet-wide.
///
/// Retires the (source, check): its state on every server is resolved and
/// it stops counting toward health, incidents, and source staleness. When
/// the source has no live checks left, its per-server staleness clears
/// too. If the check is ever reported again it returns pending review at
/// the warning ceiling.
#[utoipa::path(
	post,
	path = "/decommission",
	operation_id = "healthcheck_decommission",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = DecommissionArgs,
	responses(
		(status = 200, description = "Check decommissioned."),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn decommission(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<DecommissionArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	CheckPolicy::decommission(
		&mut conn,
		&args.source,
		&namespace_of(&args.namespace)?,
		&args.check_name,
		&admin.0.login,
	)
	.await?;
	Ok(Json(()))
}

/// Request body for replacing a check's documentation.
#[derive(Deserialize, ToSchema)]
pub struct UpdateDocumentationArgs {
	/// The source whose check to document.
	pub source: String,
	/// The healthcheck name to document; must already exist in the
	/// catalog.
	pub check_name: String,
	/// Which catalog entry is meant, as returned in the entry's `namespace`.
	/// Omitted is the unqualified namespace a curated source's checks live in.
	#[serde(default)]
	pub namespace: NamespaceRef,
	/// The new markdown document, or `null` (or blank) to clear it.
	#[serde(default)]
	pub documentation: Option<String>,
}

/// Replace a check's documentation.
///
/// Stores the markdown document presented alongside the check wherever
/// its state appears, and over MCP. Sending `null` or a blank document
/// clears it. Doesn't mark the policy as reviewed — documenting a check
/// is not the same as reviewing its grading.
#[utoipa::path(
	post,
	path = "/update_documentation",
	operation_id = "healthcheck_update_documentation",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = UpdateDocumentationArgs,
	responses(
		(status = 200, description = "Updated catalog row.", body = CheckPolicyData),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn update_documentation(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<UpdateDocumentationArgs>,
) -> Result<Json<CheckPolicyData>> {
	let documentation = args
		.documentation
		.as_deref()
		.map(str::trim)
		.filter(|d| !d.is_empty());
	let mut conn = state.db.get().await?;
	let row = CheckPolicy::update_documentation(
		&mut conn,
		&args.source,
		&namespace_of(&args.namespace)?,
		&args.check_name,
		documentation,
	)
	.await?;
	Ok(Json(row.into()))
}

/// Request body for replacing a check's conditional rules.
#[derive(Deserialize, ToSchema)]
pub struct UpdateRulesArgs {
	/// The source whose check's rules to replace.
	pub source: String,
	/// The healthcheck name whose rules to replace; must already exist
	/// in the catalog.
	pub check_name: String,
	/// Which catalog entry is meant, as returned in the entry's `namespace`.
	/// Omitted is the unqualified namespace a curated source's checks live in.
	#[serde(default)]
	pub namespace: NamespaceRef,
	/// The new conditional rules to store, or `null` to remove all
	/// conditional rules and rely solely on the ceiling. Same shape as
	/// the `rules` field returned when listing checks. A ladder with no
	/// condition/result pairs is treated the same as `null`.
	#[schema(value_type = Option<serde_json::Value>)]
	#[serde(default)]
	pub rules: Option<JsonValue>,
}

/// Replace a check's conditional rules.
///
/// Stores a new set of conditional rules for the given (source, check)
/// (or removes them, if `rules` is `null`), and marks the check as
/// reviewed by the caller. Returns 400 if `rules` doesn't parse as a
/// valid ladder — for example an unknown comparison operator, a
/// malformed variable reference, or an odd number of entries.
#[utoipa::path(
	post,
	path = "/update_rules",
	operation_id = "healthcheck_update_rules",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = UpdateRulesArgs,
	responses(
		(status = 200, description = "Updated catalog row.", body = CheckPolicyData),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn update_rules(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<UpdateRulesArgs>,
) -> Result<Json<CheckPolicyData>> {
	let ladder: Option<IfLadder> = match args.rules {
		None | Some(JsonValue::Null) => None,
		Some(v) => {
			let parsed: IfLadder = serde_json::from_value(v)
				.map_err(|e| AppError::BadRequest(format!("invalid rules: {e}")))?;
			// An empty ladder is equivalent to "no rules"; normalise so the
			// stored shape is always either NULL or a non-empty if-ladder.
			if parsed.branches.is_empty() {
				None
			} else {
				Some(parsed)
			}
		}
	};
	let mut conn = state.db.get().await?;
	let row = CheckPolicy::update_rules(
		&mut conn,
		&args.source,
		&namespace_of(&args.namespace)?,
		&args.check_name,
		ladder.as_ref(),
		&admin.0.login,
	)
	.await?;
	Ok(Json(row.into()))
}

/// Request body identifying which healthcheck to sample data for.
#[derive(Deserialize, ToSchema)]
pub struct SampleArgs {
	/// The source that reports the check. A check's identity is the
	/// (source, check) pair; another source's same-named check may carry
	/// entirely different fields.
	pub source: String,
	/// The healthcheck name to sample.
	pub check_name: String,
	/// Which catalog entry is meant, as returned in the entry's `namespace`.
	/// Omitted is the unqualified namespace a curated source's checks live in.
	#[serde(default)]
	pub namespace: NamespaceRef,
}

/// A real-world sample of the data a conditional rule can reference for a
/// given healthcheck, taken from the most recent status report (across
/// all applications) from the check's own source that included it.
#[derive(Serialize, ToSchema)]
pub struct HealthcheckSample {
	/// Additional top-level fields submitted with the status report that
	/// contained this check, available to conditional rules under the
	/// `status.<field>` namespace.
	pub status_extra: serde_json::Map<String, JsonValue>,
	/// The sampled check's own reported fields (excluding its name and
	/// pass/fail flag), available to conditional rules under the
	/// `check.<field>` namespace.
	pub check_extra: serde_json::Map<String, JsonValue>,
	/// The reporting server's tags, merged with its group's tags,
	/// available to conditional rules under the `tag.<key>` namespace.
	pub tags: HashMap<String, String>,
	/// Hostname of the server the sample was taken from.
	pub server_host: String,
	/// Friendly name of the server the sample was taken from, if it has
	/// one.
	pub server_name: Option<String>,
	/// When the sampled status report was received.
	pub seen_at: Timestamp,
}

/// Result of sampling real data for a healthcheck's conditional rules.
#[derive(Serialize, ToSchema)]
pub struct HealthcheckSampleResponse {
	/// The healthcheck name that was sampled.
	pub check_name: String,
	/// The sampled data, or `null` if no server has ever reported this
	/// check.
	pub sample: Option<HealthcheckSample>,
}

/// Sample real data available to a healthcheck's conditional rules.
///
/// Fetches the most recent status report from any server that reported
/// the given check, and returns the fields a conditional rule could
/// reference for it. Useful for discovering what data is actually
/// available before writing a rule, and for previewing how a candidate
/// rule would evaluate. Returns a `null` sample if no server has ever
/// reported this check.
#[utoipa::path(
	post,
	path = "/sample",
	operation_id = "healthcheck_sample",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	request_body = SampleArgs,
	responses(
		(status = 200, description = "Sample payload or null if no server has reported this check yet.", body = HealthcheckSampleResponse),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn sample(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SampleArgs>,
) -> Result<Json<HealthcheckSampleResponse>> {
	let mut conn = state.db.get().await?;
	// Sampled from a push by an application of the entry's own type: another
	// type's same-named check carries different fields, and a rule written
	// against those would grade nothing.
	let namespace = namespace_of(&args.namespace)?;
	let Some(status) = Status::latest_for_check_name(
		&mut conn,
		&args.source,
		&args.check_name,
		namespace.application_type(),
	)
	.await?
	else {
		return Ok(Json(HealthcheckSampleResponse {
			check_name: args.check_name,
			sample: None,
		}));
	};
	// A push from a box Canopy holds no application for is the box's in full,
	// so the sample's context is the box's: its tags, its name, and no host.
	let server = match status.server_id {
		Some(id) => Some(Application::get_by_id(&mut conn, id).await?),
		None => None,
	};
	let machine = match status.machine_id {
		Some(id) => Some(database::machines::Machine::get_by_id(&mut conn, id).await?),
		None => None,
	};

	// Top-level extras — the column is always an object after our
	// ingestion path strips reserved keys.
	let status_extra = status.extra.as_object().cloned().unwrap_or_default();

	// Pull the check entry out of the health array (any entry matching
	// by name; we don't require a failing result here so we still
	// surface the check's typical shape even on a passing push). Strip
	// the reserved fields and inject the normalised `result` so the UI
	// sees exactly what the ingestion path passes to the policy —
	// including a `check.result` value on legacy (`healthy: bool`)
	// pushes.
	let check_extra = status
		.health
		.as_array()
		.and_then(|arr| {
			arr.iter().find_map(|e| {
				let obj = e.as_object()?;
				let name = obj.get("check")?.as_str()?;
				if name == args.check_name {
					let result = commons_types::status::CheckResult::from_entry(obj);
					let mut m = obj.clone();
					m.remove("check");
					m.remove("healthy");
					if let Some(result) = result {
						m.insert("result".into(), JsonValue::String(result.to_string()));
					}
					Some(m)
				} else {
					None
				}
			})
		})
		.unwrap_or_default();

	let tag_map = match (&server, &machine) {
		(Some(server), _) => server.tags_merged_with_group(&mut conn).await?,
		(None, Some(machine)) => machine.tags_merged_with_group(&mut conn).await?,
		(None, None) => Default::default(),
	};
	let tags: HashMap<String, String> = tag_map.0.into_iter().collect();

	Ok(Json(HealthcheckSampleResponse {
		check_name: args.check_name,
		sample: Some(HealthcheckSample {
			status_extra,
			check_extra,
			tags,
			server_host: server
				.as_ref()
				.and_then(|s| s.host.as_ref())
				.map(|h| h.0.to_string())
				.unwrap_or_default(),
			server_name: server
				.map(|s| s.display_name())
				.or_else(|| machine.and_then(|m| m.name)),
			seen_at: status.created_at,
		}),
	}))
}

/// List all known tag keys.
///
/// Returns the sorted, deduplicated set of tag keys used across every
/// server and server group in the fleet, including keys not present on
/// the sample returned by the sampling endpoint. Useful for discovering
/// which `tag.<key>` variables are available when writing a conditional
/// rule, even for tags the sampled server doesn't happen to carry.
#[utoipa::path(
	post,
	path = "/tag_keys",
	operation_id = "healthcheck_tag_keys",
	tag = "healthchecks",
	security(("tailscale-admin" = [])),
	responses(
		(status = 200, description = "Sorted, distinct tag keys.", body = Vec<String>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
pub async fn tag_keys(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<Vec<String>>> {
	let mut conn = state.db.get().await?;
	let keys = database::tags::all_known_keys(&mut conn).await?;
	Ok(Json(keys))
}
