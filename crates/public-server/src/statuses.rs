use std::collections::{BTreeMap, BTreeSet};
use std::str::FromStr as _;

use axum::{
	Json,
	extract::{Path, State},
};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	backup_jobs::backups_due_now_for_machine, device_auth::ServerDevice, headers::VersionHeader,
};
use commons_types::{
	backup::BackupType,
	device::DeviceRole,
	namespace::{Namespace, RESERVED_SOURCES, is_reserved},
	server::{TagMap, app_type::ApplicationType},
	status::{CheckResult, CheckSeverity},
	subject::CheckSubject,
	version::VersionStr,
};
use database::{
	Db,
	applications::Application,
	check_policies::{CheckPolicy, FilingScope},
	devices::Device,
	diesel_async::{AsyncConnection, AsyncPgConnection},
	issues::{
		CheckGrading, CheckOutcome, CheckStateStamp, GradedCheck, GradingContext, Issue, NewEvent,
		ReportedCheck, Scope, grade_instances, is_health_structure,
	},
	machines::Machine,
	silenced_refs::{silenced_health_checks_of_application, silenced_health_checks_of_machine},
	statuses::{NewStatus, Status},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

/// A status push: a server's periodic heartbeat carrying its self-reported
/// health.
///
/// Besides the reserved `healthy` and `health` keys described here, any
/// additional top-level fields are accepted and stored verbatim as extra
/// status data.
#[derive(Debug, Deserialize, ToSchema)]
pub struct StatusPayload {
	/// The name of the source pushing this status: the reporting agent, e.g.
	/// `alertd`. Multiple sources may report on one server, each with its own
	/// set of checks; a source's push only opens and recovers its own checks.
	///
	/// **Transitionally optional: this field will become mandatory.** A push
	/// without a `source` is attributed to `alertd`; new reporters must send
	/// their own name. Must be a non-empty string; the names `canopy` and
	/// `manual` are reserved for canopy itself and are rejected.
	pub source: Option<String>,

	/// Overall self-reported health of the server. **Absent means `true`**,
	/// so senders that predate this field are never treated as unhealthy by
	/// omission. Recorded for historical analysis and display, but **not
	/// consulted for incident or severity decisions** — those are derived
	/// from the per-check results in `health`, with each check's severity
	/// controlled by an operator-managed catalog.
	pub healthy: Option<bool>,

	/// Per-check breakdown. A push without a `health` array is the legacy
	/// Tamanu direct-report format: it is treated as the `tamanu` source
	/// reporting a single always-passing `tasks` heartbeat check. May be
	/// empty (`[]`) for a source that genuinely runs no checks — which
	/// recovers every check it previously reported. Each entry must
	/// include a non-empty `check` name and exactly one of `result` /
	/// `healthy` / `instances`; a check's own fields (latency, free disk %,
	/// certificate expiry, etc.) go in its `detail`, or flat beside a single
	/// result, and are recorded verbatim and shown in the status UI.
	///
	/// Every check name seen — whatever its result — is added to the
	/// operator-facing check catalog, where the policy grading its results
	/// can be reviewed and adjusted. A check whose effective result is
	/// failed or warning opens (or keeps open) its issue; a broken check
	/// keeps the same issue open, retaining a known failure's contribution
	/// while warning the check itself is broken; effective passed and
	/// skipped results open nothing and close prior issues.
	pub health: Vec<HealthCheck>,

	/// The machine's own health checks and detail: what the box is, rather
	/// than what runs on it.
	///
	/// Sending this puts the push in the current format, and Canopy takes the
	/// separation as given. A push without it is a transitional unified push,
	/// which Canopy separates into the two grains itself from `health` and the
	/// flat body.
	pub machine: Option<TargetReport>,

	/// The applications the reporter found on the machine, each with its own
	/// health checks and detail, keyed by a key the reporter chooses.
	///
	/// The key must be unique among the applications on that machine and must
	/// identify the same application across this reporter's pushes; what it is
	/// derived from is the reporter's own business. Canopy correlates on the
	/// machine, the key and the type together, and never discloses its own
	/// identifier for an application.
	///
	/// Only read alongside `machine`. An application named here that Canopy
	/// does not already hold is created.
	pub applications: Option<BTreeMap<String, ApplicationReport>>,

	/// Free-form additional data (uptime, database version, timezone,
	/// hostname, etc.). Stored verbatim and surfaced as raw JSON in the
	/// status view.
	///
	/// Transitional, and read only on a push with no `machine` section: the
	/// current format carries every reported field inside a `detail` object on
	/// the target it belongs to.
	///
	/// A `tamanuVersion` field here is used as the server's tracked version
	/// (compared against the published version catalog), superseding the
	/// legacy `X-Version` request header. If both are present,
	/// `tamanuVersion` wins; if neither is, the status is recorded without a
	/// version.
	#[serde(flatten)]
	#[schema(additional_properties = true, value_type = Object)]
	pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One target's material within a push: its health checks and its detail.
///
/// A machine and an application are described the same way, so the two grains
/// read alike and a reporter builds one shape for both.
#[derive(Debug, Deserialize, ToSchema)]
pub struct TargetReport {
	/// This target's checks. Absent and empty mean the same thing — the source
	/// currently has no checks for this target — which recovers every check it
	/// previously reported for it.
	pub health: Option<Vec<HealthCheck>>,
	/// Everything the reporter has to say about this target beyond its checks.
	/// Recorded verbatim against the target it was attached to.
	#[schema(additional_properties = true, value_type = Object)]
	pub detail: Option<serde_json::Map<String, serde_json::Value>>,
}

/// One application within a push, as the reporter found it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ApplicationReport {
	/// What this application is: the software and the role it plays together,
	/// for example `tamanu-central`. Required, and part of how Canopy
	/// correlates the report to its own record — a different type under a key
	/// already in use means the reporter has stopped reporting one application
	/// and started reporting another.
	pub r#type: String,
	/// This application's checks. Reported bare; Canopy qualifies them with
	/// the application's type when cataloguing them.
	pub health: Option<Vec<HealthCheck>>,
	/// Everything the reporter has to say about this application beyond its
	/// checks, including its `tamanuVersion`.
	#[schema(additional_properties = true, value_type = Object)]
	pub detail: Option<serde_json::Map<String, serde_json::Value>>,
}

/// One health check within a status push: its name, its own detail, and either
/// a single result or a set of instances.
///
/// Exactly one of `result`, `healthy` and `instances` must be present.
#[derive(Debug, Deserialize, ToSchema)]
pub struct HealthCheck {
	/// Name of the check. Must be a non-empty string, and should stay stable
	/// across pushes: results for the same name are correlated over time, so
	/// successive failures and the eventual recovery land on the same issue.
	pub check: String,
	/// Outcome of the check: `passed`, `warning`, `failed`, `broken`, or
	/// `skipped`. Exactly one of `result` / `healthy` / `instances` must be
	/// present per entry. `warning` and `failed` open the check's issue as
	/// graded by its policy; `broken` (the check itself errored, not the
	/// system under test) neither confirms nor clears a known failure — the
	/// issue stays open, retaining its contribution; `skipped` (a precondition
	/// was not met) and `passed` open nothing and close prior issues.
	pub result: Option<CheckResult>,
	/// Legacy pass/fail form: `true` means `passed`, `false` means `failed`.
	/// Mutually exclusive with `result` and `instances`.
	pub healthy: Option<bool>,
	/// The check's own fields (shown in the status UI as a key/value block,
	/// and read by operator-defined policy rules as `check.<field>`). For a
	/// check with `instances`, the fields its instances share; a rule reads an
	/// instance's own field of the same name over it.
	///
	/// An object. A check with a single result may instead carry its fields
	/// flat beside `check` and `result` (see `extra`), but not both ways at
	/// once; a check with `instances` carries its fields here only. A `detail`
	/// holding anything but an object is one of those flat fields.
	#[schema(additional_properties = true, value_type = Object, required = false)]
	pub detail: Option<serde_json::Map<String, serde_json::Value>>,
	/// The check's instances, in place of a `result`: one entry per instance
	/// of the check's condition (one per device, per resource, per mount),
	/// keyed by an instance key the reporter chooses. A key must not be empty,
	/// must be unique within the check, and must identify the same instance
	/// across this reporter's pushes.
	///
	/// The set is the check's complete one: an instance left out of a push
	/// has recovered, so passing instances are sent too, and an empty object
	/// recovers every instance the check held. Each instance is graded
	/// through the check's policy on its own, and the check takes the most
	/// urgent result among the instances that were not skipped. Canopy writes
	/// the check's message from its graded instances, naming the degraded ones
	/// by label.
	///
	/// A check that could not run reports `result: broken` without instances:
	/// brokenness belongs to the whole check, never to one instance.
	///
	/// Only an object is a set of instances. An `instances` holding anything
	/// else beside a single result is one of the check's flat fields.
	pub instances: Option<BTreeMap<String, HealthCheckInstance>>,
	/// A check with a single result may carry its fields flat beside `check`
	/// and `result`, outside `detail`, and they are read as its detail. Not
	/// accepted together with `detail` or beside `instances`.
	#[serde(flatten)]
	#[schema(additional_properties = true, value_type = Object)]
	pub extra: serde_json::Map<String, serde_json::Value>,
}

/// One instance of a health check.
#[derive(Debug, Deserialize, ToSchema)]
pub struct HealthCheckInstance {
	/// What this instance observed: `passed`, `warning`, `failed`, or
	/// `skipped`. Required. `broken` is refused, being the whole check's
	/// result (report it as the check's own `result`, without instances), and
	/// the legacy `healthy` boolean is not accepted on an instance.
	pub result: CheckResult,
	/// How the instance is named to an operator, in the check's message and
	/// wherever it is listed. Without one, the instance is named by its key.
	pub label: Option<String>,
	/// This instance's own fields. Must be an object. A rule reads them as
	/// `check.<field>`, over the check's shared `detail`.
	#[schema(additional_properties = true, value_type = Object, required = false)]
	pub detail: Option<serde_json::Map<String, serde_json::Value>>,
}

/// The source a push is attributed to when it names none: the reporter
/// deployed before the `source` field existed. Also the migration value for
/// pre-source history. Transitional — the field will become mandatory.
const DEFAULT_SOURCE: &str = "alertd";
/// The source legacy-format pushes (no `health` array) are attributed to:
/// they come from Tamanu's own direct reporting, not from alertd.
const LEGACY_SOURCE: &str = "tamanu";
/// The synthetic check a legacy push reports: a liveness heartbeat,
/// always passing on receipt. Its value is that it stops — a Tamanu
/// server that goes quiet trips the source-staleness net.
const LEGACY_CHECK: &str = "tasks";
/// Prefix for per-check refs. Each check is filed at
/// `(<source>, health/<check_name>)` — one thread per check, brokenness
/// included (a broken check retains the previous definite result's
/// contribution while additionally warning that the check is broken).
const HEALTH_REF: &str = "health";
/// The most instances one check may carry.
///
/// Instance count is the one dimension of a push a reporter can multiply
/// freely, and its cost outlives the push: every instance is graded through
/// the whole policy chain, kept in the check's state, and graded again on
/// each later push of that check and on each silence of one of its
/// instances — which a group-scoped silence fans out across the group. The
/// bound is well clear of what a real check reports (a central's facilities,
/// a machine's backup types) so that it only ever catches a reporter that
/// has gone wrong.
const MAX_INSTANCES_PER_CHECK: usize = 1000;
/// The longest an instance's key or label may be. Both name one instance to
/// an operator or to a silence, so they are identifiers rather than prose.
const MAX_INSTANCE_NAMING_CHARS: usize = 256;

/// The status-push response: only the return-path instructions the device
/// can act on. The stored status record is deliberately not echoed back —
/// the device already has everything it sent.
#[derive(Debug, Serialize, ToSchema)]
pub struct StatusResponse {
	/// Backup types the server should back up now: operator-requested
	/// one-offs plus scheduled backups that are due. Each serializes as a
	/// plain string (e.g. `"tamanu-postgres"`). The device should run each
	/// listed type, then report via `POST /backup-report`; an empty list
	/// means nothing to do. Only sent to `alertd` pushes (the agent that
	/// runs backups); other sources always receive an empty list.
	#[schema(value_type = Vec<String>)]
	pub backup_now: Vec<BackupType>,
	/// The effective handling of every healthcheck canopy knows about, keyed
	/// by check name (as reported in `health[].check`): `skip` (silenced for
	/// this server, or classified below warning), `warn` (warning), or `fail`
	/// (error or critical). Only the static severity baseline is reflected —
	/// operator-defined conditional rules are evaluated per push and not
	/// included. Checks absent from the map are new to canopy and default to
	/// `warn`. Clients that predate this field can safely ignore it; the
	/// same mapping is served on demand at `GET /status/{server_id}/check-severities`.
	pub check_severities: BTreeMap<String, CheckSeverity>,
	/// What this server is entitled to do with names: the domains its group
	/// controls, the grants it holds, whether it is paused, and the names and
	/// certificates it already has. A server-wide fact, so returned to every
	/// source — an agent already reporting status learns of a new domain or a
	/// newly granted permission without asking separately. Identical to what
	/// `GET /names/entitlements` returns. Clients that predate this field can
	/// safely ignore it.
	// spec: CRT#what-an-application-may-act-on
	pub names: crate::names::Entitlements,
	/// The server's effective tags: its own tags overlaid on its group's,
	/// plus the synthetic read-only `canopy:` tags and effective `billing.*`
	/// labels. Identical to what the standalone `GET /tags` endpoint
	/// returns — see that endpoint for the full contract. Clients that
	/// predate this field can safely ignore it.
	///
	/// On a push in the current format this is the machine's, the push being
	/// the machine's; each application's own are under `applications`.
	pub tags: TagMap,

	/// Canopy's answer about the machine. Present only for a push in the
	/// current format: a transitional unified push is answered by the flat
	/// fields above and nothing else, so the response a fielded reporter sees
	/// is the one it already saw.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub machine: Option<TargetResponse>,

	/// Canopy's answer about each application the push described, keyed by the
	/// key the reporter named it with. Present only for a push in the current
	/// format.
	///
	/// A key Canopy holds no application for is absent rather than empty,
	/// which is what a source whose pushes are ignored sees: nothing was
	/// created for it to be told about.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub applications: Option<BTreeMap<String, TargetResponse>>,
}

/// What Canopy answers about one target the push described.
// spec: STA#response
#[derive(Debug, Serialize, ToSchema)]
pub struct TargetResponse {
	/// This target's effective tags, on the same terms as the top-level
	/// `tags`.
	pub tags: TagMap,
	/// How every check this reporter can file against this target is graded,
	/// on the same terms as the top-level `check_severities`. Keyed by bare
	/// check name, so a machine check and an application check of the same
	/// name are each answered under the target they belong to.
	pub check_severities: BTreeMap<String, CheckSeverity>,
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(public: create))
		.routes(routes!(public: check_severities))
}

/// The application type a unified push names, if it names one.
///
/// A unified push predates the split, so it has no field that says "this is
/// what I am". `tamanuServerKind` is the one thing in it that does: it named
/// the role a Tamanu application played, and role and software together are
/// what a type is. Anything else falls through to the machine's own record.
// spec: STA#transitional-unified-pushes
fn reported_application_type(extra: &serde_json::Value) -> Option<ApplicationType> {
	// The same mapping the split itself used to collapse product and kind
	// into a type, so a box reports the type it was backfilled with.
	match extra.get("tamanuServerKind")?.as_str()? {
		"facility" => Some(ApplicationType::TamanuFacility),
		_ => Some(ApplicationType::TamanuCentral),
	}
}

/// The application a unified push is about, where it is about one.
///
/// A unified push describes at most one application, the format having no way
/// to say otherwise. Where the push names its type Canopy correlates on it and
/// adopts what it does not already hold; where it does not, the machine's own
/// record answers, which it can as long as there is exactly one to answer
/// with.
///
/// `None` is a real answer, not a failure: the fleet holds boxes that run no
/// application Canopy models, and one reports its disk, memory and uptime like
/// any other. Such a push is the box's in full, so it files at machine scope
/// and records as machine detail, and nothing is attributed to a workload that
/// is not there.
///
/// Refusing rather than guessing is what is left of the last arm. A box
/// running several workloads and a reporter that will not say which one it
/// speaks for is a genuinely new situation, and attributing a box's whole
/// picture to an arbitrary one of its workloads is the failure this card
/// exists to stop.
// spec: STA#transitional-unified-pushes
async fn resolve_unified_application(
	db: &mut AsyncPgConnection,
	machine: &Machine,
	extra: &serde_json::Value,
	create: bool,
) -> Result<Option<Application>> {
	if create && let Some(r#type) = reported_application_type(extra) {
		return Application::from_report(db, machine, &r#type)
			.await
			.map(Some);
	}

	let applications = machine.applications(db).await?;
	if let Some(r#type) = reported_application_type(extra)
		&& let Some(found) = applications.iter().find(|a| a.r#type == r#type)
	{
		return Ok(Some(found.clone()));
	}

	match applications.as_slice() {
		[only] => Ok(Some(only.clone())),
		[] => Ok(None),
		_ => Err(AppError::Conflict(
			"this push names no application Canopy holds, and the machine has several".into(),
		)),
	}
}

/// Submit a status heartbeat for a machine.
///
/// `server_id` in the path is the id the agent was enrolled with, which
/// identifies the machine it runs on. Canopy works out which application on
/// that machine the push describes from the push itself.
///
/// Records a periodic status push against that machine: overall
/// self-reported health, a per-check breakdown, and any free-form extra
/// data. Machine-subject checks and detail file against the machine and the
/// rest against its application. Each failed or warning check opens (or keeps
/// open) an issue at that check's operator-configured severity, and each
/// passed check closes any issue it previously opened; the application's
/// tracked software version is also updated from the payload.
///
/// The calling device must be the one enrolled for this exact machine (or
/// hold the admin role). The response carries only return-path
/// instructions: a `backup_now` list of backup types the server should
/// back up immediately — devices should treat a non-empty list as a
/// prompt to run those backups and report them afterwards — a
/// `check_severities` map describing how canopy classifies each known
/// healthcheck for this server (`skip`/`warn`/`fail`), and the server's
/// effective `tags` (as served by `GET /tags`). The stored status record
/// is not echoed back.
#[utoipa::path(
	post,
	path = "/{server_id}",
	operation_id = "submit_status",
	tag = "statuses",
	security(("server-device" = [])),
	params(
		("server_id" = Uuid, Path),
	),
	request_body(
		content = StatusPayload,
		description = "Status push. A body without a `health` array is the legacy Tamanu direct-report format, treated as the `tamanu` source reporting a single always-passing `tasks` heartbeat check.",
	),
	responses(
		(status = 200, body = StatusResponse),
		(status = 400, body = ProblemDetailsSchema),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
	),
)]
async fn create(
	Path(machine_id): Path<Uuid>,
	State(db): State<Db>,
	State(dns_zones): State<Vec<commons_types::dns::ManagedZone>>,
	device: ServerDevice,
	current_version: Option<VersionHeader>,
	body: Option<Json<serde_json::Value>>,
) -> Result<Json<StatusResponse>> {
	let mut db = db.get().await?;
	let Device { role, id, .. } = device.0.0;

	// The id on the wire is the machine's: an agent identifies the box it runs
	// on, reports that box's disk, memory, load and addresses, and enrols as a
	// device belonging to it. Which workloads that box runs is Canopy's to
	// work out from what the push says, not something the path can carry.
	// spec: STA#push
	let machine = Machine::get_by_id(&mut db, machine_id).await?;
	let is_authorized = role == DeviceRole::Admin || machine.device_id == Some(id);

	if !is_authorized {
		return Err(AppError::AuthInsufficientPermissions {
			required: "the device enrolled for this machine".into(),
		});
	}

	let raw = body.map(|j| j.0).unwrap_or(serde_json::Value::Null);
	let ParsedPush {
		source,
		healthy,
		body,
	} = parse_push(raw)?;

	// Legacy format (no `health` array): Tamanu's direct reporting. It
	// becomes a heartbeat from the `tamanu` source — a single `tasks`
	// check that always passes on receipt — and flows through the unified
	// path from here, so it records state, registers its catalog entry,
	// and participates in source staleness like any source.
	let (source, ingest) = match body {
		PushBody::Split {
			machine,
			applications,
		} => (
			source,
			Ingest::Split {
				machine,
				applications,
			},
		),
		PushBody::Unified { health, extra } => (source, Ingest::Unified { health, extra }),
		PushBody::Legacy { extra } => (
			LEGACY_SOURCE.to_string(),
			Ingest::Unified {
				health: serde_json::json!([{ "check": LEGACY_CHECK, "result": "passed" }]),
				extra,
			},
		),
	};

	// Ingest gating (see CHK "Source policy"): a denied source's push is
	// rejected outright; an ignored source's push is accepted but its data
	// is not recorded. Either way the device still gets the normal response
	// below (backup instructions, tags, severities) — those come from server
	// state, not from this push, so an ignored reporter keeps functioning.
	let record = match database::source_policies::SourcePolicy::ingest_for(&mut db, &source).await?
	{
		commons_types::source::IngestMode::Allow => true,
		commons_types::source::IngestMode::Ignore => false,
		commons_types::source::IngestMode::Deny => return Err(AppError::IngestDenied(source)),
	};

	// Which applications this push is about, resolved under a lock on the
	// machine row: a push that finds no application for what it describes
	// creates one, and two arriving together for one box would otherwise each
	// see nothing and each create. An ignored source records nowhere, so it
	// reads without creating.
	// spec: FLT#applications-come-from-reports
	let resolved = db
		.transaction::<_, AppError, _>(async |conn| {
			if record {
				Machine::get_by_id_for_update(conn, machine.id).await?;
			}
			match ingest {
				Ingest::Split {
					machine: machine_report,
					applications,
				} => {
					let mut resolved = Vec::new();
					for (key, reported) in applications {
						// An ignored source resolves nothing new, so a key it
						// names that Canopy holds nothing for drops out here
						// rather than standing up a record.
						let Some(application) = Application::from_report_key(
							conn,
							&machine,
							&key,
							&reported.r#type,
							record,
						)
						.await?
						else {
							continue;
						};
						resolved.push((key, application, reported.report));
					}
					Ok(ResolvedPush::Split {
						machine: machine_report,
						applications: resolved,
					})
				}
				Ingest::Unified { health, extra } => Ok(ResolvedPush::Unified {
					application: resolve_unified_application(conn, &machine, &extra, record)
						.await?,
					health,
					extra,
				}),
			}
		})
		.await?;

	// The effective tags of each target the push describes: stored tags
	// overlaid on the group's, plus the synthetic `canopy:*` tags and, for an
	// application, the computed `billing.*` labels. Computed once and used for
	// both grading (per CHK, rules evaluate against the effective tags, so a
	// rule can predicate on any of them) and the device response.
	//
	// The flat `tags` and `check_severities` a split push is answered with are
	// the machine's, the push being addressed to the machine and its per-target
	// answers riding in `machine` and `applications`.
	let (effective_tags, check_severities, machine_response, application_responses) = match resolved
	{
		ResolvedPush::Split {
			machine: machine_report,
			applications,
		} => {
			let machine_tags = crate::tags::effective_tags_for_machine(&mut db, &machine).await?;
			let mut apps = Vec::new();
			for (key, application, report) in applications {
				let tags = crate::tags::effective_tags_for_server(&mut db, &application).await?;
				apps.push((key, application, report, tags));
			}

			if record {
				// Insert + file events atomically. NewEvent::save itself opens
				// a transaction; diesel-async nests it as a SAVEPOINT.
				//
				// One status row per target the push described, because that
				// is the grain everything downstream reads a push back at:
				// reachability, the last-seen sample behind a check, and a
				// group's quiet members all ask what a given target last said.
				// spec: STA#push
				db.transaction::<_, AppError, _>(async |conn| {
					let status = NewStatus {
						machine_id: machine.id,
						server_id: None,
						device_id: Some(id),
						// The version Canopy tracks is an application's, so a
						// machine row carries none.
						version: None,
						extra: machine_report.detail,
						healthy,
						health: machine_report.health,
						source: source.clone(),
					}
					.save(conn)
					.await?;
					database::reported_detail::MachineReportedDetail::record(
						conn,
						machine.id,
						&status.source,
						&status.extra,
					)
					.await?;
					file_health_events(
						conn,
						None,
						machine.id,
						machine.group_id,
						None,
						Some(id),
						&status,
						&json_tags(&machine_tags),
						SubjectSplit::AsGiven,
					)
					.await?;

					for (_, application, report, tags) in &apps {
						// Each application's own version, from its own detail.
						// The `X-Version` header names one application and a
						// split push may describe several, so it has no
						// meaning here and is not consulted.
						let version = resolve_version(&report.detail, None);
						let status = NewStatus {
							machine_id: machine.id,
							server_id: Some(application.id),
							device_id: Some(id),
							version,
							extra: report.detail.clone(),
							healthy,
							health: report.health.clone(),
							source: source.clone(),
						}
						.save(conn)
						.await?;
						database::reported_detail::ReportedDetail::record_for_application(
							conn,
							application.id,
							&status.source,
							&status.extra,
							status.version.as_ref(),
						)
						.await?;
						file_health_events(
							conn,
							Some(application.id),
							machine.id,
							application.group_id,
							Some(&application.r#type),
							Some(id),
							&status,
							&json_tags(tags),
							SubjectSplit::AsGiven,
						)
						.await?;
					}

					Ok(())
				})
				.await?;
			}

			// Computed after the transaction so checks first seen on this very
			// push (upserted into the catalog above) are already in the map.
			let machine_severities = machine_check_severities(&mut db, &machine, &source).await?;
			let mut responses = BTreeMap::new();
			for (key, application, _, tags) in apps {
				let check_severities =
					application_check_severities(&mut db, &application, &source).await?;
				responses.insert(
					key,
					TargetResponse {
						tags,
						check_severities,
					},
				);
			}

			(
				machine_tags.clone(),
				machine_severities.clone(),
				Some(TargetResponse {
					tags: machine_tags,
					check_severities: machine_severities,
				}),
				Some(responses),
			)
		}
		ResolvedPush::Unified {
			application,
			health,
			extra,
		} => {
			// The server version canopy tracks (and compares against the
			// published version catalog) is the Tamanu version. Prefer the
			// payload's `tamanuVersion` extra; fall back to the legacy
			// `X-Version` header for reporters that predate carrying it in the
			// body. Either may be absent.
			let version = resolve_version(&extra, current_version.map(|v| v.0));
			let server_id = application.as_ref().map(|s| s.id);
			let group_id = application
				.as_ref()
				.map_or(machine.group_id, |s| s.group_id);

			// A push with no application is the box's, so it grades and
			// answers against the box's own tags rather than borrowing a
			// workload's.
			let effective_tags = match &application {
				Some(application) => {
					crate::tags::effective_tags_for_server(&mut db, application).await?
				}
				None => crate::tags::effective_tags_for_machine(&mut db, &machine).await?,
			};
			let tags = json_tags(&effective_tags);

			// Only the recording is conditional on ingest mode; everything
			// else — backup instructions, tags, severities computed below — is
			// returned regardless, so an ignored reporter keeps working.
			if record {
				db.transaction::<_, AppError, _>(async |conn| {
					let status = NewStatus {
						machine_id: machine.id,
						server_id,
						device_id: Some(id),
						version,
						extra,
						healthy,
						health,
						source: source.clone(),
					}
					.save(conn)
					.await?;

					// This source's current server-wide detail, replacing what
					// it last reported. The push is the source's whole truth,
					// the same rule its checks follow just below.
					// spec: FIG#sourcing
					database::reported_detail::ReportedDetail::record(
						conn,
						server_id,
						machine.id,
						&status.source,
						&status.extra,
						status.version.as_ref(),
					)
					.await?;

					file_health_events(
						conn,
						server_id,
						machine.id,
						group_id,
						application.as_ref().map(|s| &s.r#type),
						Some(id),
						&status,
						&tags,
						SubjectSplit::BySubject,
					)
					.await?;

					Ok(())
				})
				.await?;
			}

			let check_severities =
				unified_check_severities(&mut db, application.as_ref(), &machine, &source).await?;

			(effective_tags, check_severities, None, None)
		}
	};

	// Tell the device which backup types to run now (operator one-offs +
	// schedule-due), riding the heartbeat response. Only alertd runs
	// backups — other sources (the tamanu heartbeat, seedling) would
	// treat an instruction they can't act on as noise at best. Empty for
	// an ungrouped server or one whose group has no `ready` backup config.
	let backup_now = match machine.group_id {
		Some(group_id) if source == DEFAULT_SOURCE => {
			backups_due_now_for_machine(&mut db, machine.id, group_id, Timestamp::now()).await?
		}
		_ => Vec::new(),
	};

	// A server-wide fact, like the tags: every source gets it, so an agent
	// reporting status learns of a new domain or grant without a second call.
	// spec: CRT#what-an-application-may-act-on
	let names = crate::names::entitlements_for(&mut db, &machine, &dns_zones).await?;

	Ok(Json(StatusResponse {
		backup_now,
		check_severities,
		names,
		tags: effective_tags,
		machine: machine_response,
		applications: application_responses,
	}))
}

/// What the push carries once its shape is known and a legacy body has been
/// turned into the heartbeat it stands for.
enum Ingest {
	Split {
		machine: TargetReportBody,
		applications: BTreeMap<String, ApplicationReportBody>,
	},
	Unified {
		health: serde_json::Value,
		extra: serde_json::Value,
	},
}

/// The push with each target it names resolved to the record Canopy holds.
enum ResolvedPush {
	Split {
		machine: TargetReportBody,
		applications: Vec<(String, Application, TargetReportBody)>,
	},
	/// A unified push, and whichever single application Canopy worked out it
	/// was about. `None` is a box Canopy holds no application for, whose push
	/// is the machine's in full.
	Unified {
		application: Option<Application>,
		health: serde_json::Value,
		extra: serde_json::Value,
	},
}

/// Effective tags in the form the policy rule evaluator compares against:
/// JSON-wrapped, so a rule compares them uniformly with reported detail.
fn json_tags(tags: &TagMap) -> std::collections::HashMap<String, serde_json::Value> {
	tags.0
		.iter()
		.map(|(k, v)| (k.clone(), serde_json::Value::String(v.clone())))
		.collect()
}

/// Fetch the effective healthcheck severity mapping for a server.
///
/// Returns, for every healthcheck the `alertd` source reports, how that
/// check is handled for this server: `skip` (the check is silenced for
/// this server — at server or group scope — or its policy ceiling means it
/// never alerts), `warn` (graded at most a warning), or `fail` (failures
/// count as failures). Keys are check names as reported in
/// `health[].check` on status pushes. Only the static policy ceiling is
/// reflected; operator-defined conditional rules are evaluated per push
/// and not included here. The same mapping also rides along every
/// status-push response as `check_severities`, scoped to the pushing
/// source.
///
/// `server_id` in the path is the id the agent was enrolled with, which
/// identifies the machine it runs on.
///
/// The calling device must be the one enrolled for this exact machine (or
/// hold the admin role).
#[utoipa::path(
	get,
	path = "/{server_id}/check-severities",
	operation_id = "check_severities",
	tag = "statuses",
	security(("server-device" = [])),
	params(
		("server_id" = Uuid, Path),
	),
	responses(
		(status = 200, description = "Effective handling for each known check, keyed by check name.", body = BTreeMap<String, CheckSeverity>),
		(status = 401, body = ProblemDetailsSchema),
		(status = 403, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
async fn check_severities(
	Path(machine_id): Path<Uuid>,
	State(db): State<Db>,
	device: ServerDevice,
) -> Result<Json<BTreeMap<String, CheckSeverity>>> {
	let mut db = db.get().await?;
	let Device { role, id, .. } = device.0.0;

	let machine = Machine::get_by_id(&mut db, machine_id).await?;
	if role != DeviceRole::Admin && machine.device_id != Some(id) {
		return Err(AppError::AuthInsufficientPermissions {
			required: "the device enrolled for this machine".into(),
		});
	}

	// The read has no payload to name an application with, so it answers for
	// the machine's own, exactly as a unified push with no type does. A
	// response shaped for one application cannot answer for a box running
	// several, so it says so rather than picking one; a box with no
	// application answers for itself.
	let server =
		resolve_unified_application(&mut db, &machine, &serde_json::Value::Null, false).await?;

	let map = unified_check_severities(&mut db, server.as_ref(), &machine, DEFAULT_SOURCE).await?;
	Ok(Json(map))
}

/// Build the effective per-check map for one target and source: every check
/// in the source's catalog in the target's namespace, mapped from its static
/// policy ceiling (`failed` → `fail`, `warning`/`broken` → `warn`,
/// `passed`/`skipped` → `skip`), then the target's `silenced` checks forced to
/// `skip`. Conditional rules are deliberately not consulted — they depend on
/// each push's contents, so only the static ceiling can be mapped ahead of
/// time.
async fn effective_check_severities(
	db: &mut AsyncPgConnection,
	source: &str,
	namespace: &Namespace,
	silenced: BTreeSet<String>,
) -> Result<BTreeMap<String, CheckSeverity>> {
	// Keyed by bare check name, because that is what the reporter sends and
	// reads back. The catalog is narrowed to the one namespace this target's
	// checks file into, so the box's `memory` and an application's own
	// `memory` do not collide here.
	let mut map: BTreeMap<String, CheckSeverity> =
		CheckPolicy::ceiling_map_for_source(db, source, namespace)
			.await?
			.into_iter()
			.map(|(name, ceiling)| (name, ceiling.into()))
			.collect();

	// Silences are keyed per (source, check): only this source's own
	// silences force its checks to skip.
	for check in silenced {
		map.insert(check, CheckSeverity::Skip);
	}

	Ok(map)
}

/// The effective check map for a machine's own checks under one source.
async fn machine_check_severities(
	db: &mut AsyncPgConnection,
	machine: &Machine,
	source: &str,
) -> Result<BTreeMap<String, CheckSeverity>> {
	let silenced = silenced_health_checks_of_machine(db, machine, source).await?;
	effective_check_severities(db, source, &Namespace::of(source, None), silenced).await
}

/// The effective check map for an application's own checks under one source.
async fn application_check_severities(
	db: &mut AsyncPgConnection,
	application: &Application,
	source: &str,
) -> Result<BTreeMap<String, CheckSeverity>> {
	let silenced = silenced_health_checks_of_application(db, application, source).await?;
	let namespace = Namespace::of(source, Some(&application.r#type));
	effective_check_severities(db, source, &namespace, silenced).await
}

/// The effective check map for a unified push, which gets one answer for both
/// grains: each name is answered from the grain ingest files it at, the
/// machine's for a machine-subject name and the application's for the rest.
/// A push naming no application is the box's in full.
// spec: STA#transitional-unified-pushes
async fn unified_check_severities(
	db: &mut AsyncPgConnection,
	application: Option<&Application>,
	machine: &Machine,
	source: &str,
) -> Result<BTreeMap<String, CheckSeverity>> {
	let on_machine = machine_check_severities(db, machine, source).await?;
	let Some(application) = application else {
		return Ok(on_machine);
	};
	let own = application_check_severities(db, application, source).await?;
	Ok(on_machine
		.into_iter()
		.filter(|(name, _)| CheckSubject::of(name).is_machine())
		.chain(
			own.into_iter()
				.filter(|(name, _)| !CheckSubject::of(name).is_machine()),
		)
		.collect())
}

/// Resolve the server version to record on this status. Prefers the payload's
/// `tamanuVersion` extra (the version bestool now carries in the body), parsed
/// as a semver; falls back to the `X-Version` header for reporters that still
/// send it there. Returns `None` when neither is present or parseable — the
/// `statuses.version` column is nullable and every consumer already handles a
/// versionless row.
fn resolve_version(extra: &serde_json::Value, header: Option<VersionStr>) -> Option<VersionStr> {
	extra
		.get("tamanuVersion")
		.and_then(|v| v.as_str())
		.and_then(|s| VersionStr::from_str(s).ok())
		.or(header)
}

/// How a push's checks map onto the two grains.
// spec: STA#transitional-unified-pushes
#[derive(Debug, Clone, Copy)]
enum SubjectSplit {
	/// A unified push, carrying both grains' checks in one set: each check's
	/// own subject says which grain it belongs to.
	BySubject,
	/// A split push, where the reporter separated the grains itself: every
	/// check in this filing belongs to the target it was filed under.
	AsGiven,
}

/// Per-push event filing. Warning/failed checks land at
/// `(status, health/<check>)`; recoveries close those issues. Broken
/// checks (`result: broken` — the check itself errored, not the
/// system under test) neither confirm nor clear a known failure: the
/// check's issue stays open, retaining its contribution. Skipped checks
/// (`result: skipped` — precondition not met) file nothing and close
/// the check's issue.
///
/// Every check is graded through [`grade_instances`]: each of its instances
/// through the operator-owned `check_policies` catalog entry for
/// `(source, check)` (see [`CheckPolicy::grade`] for the rules/ceiling
/// contract) and the scoped chain covering the target, a check with a single
/// result as its own single instance. Every check seen on a push — whatever
/// its result — upserts a default catalog row so new checks are visible to
/// operators immediately at the default warning ceiling. `status.healthy` is
/// intentionally not consulted: the catalog is canopy's single source of
/// truth for per-check grading.
///
/// Until issues themselves carry results, the effective result maps to
/// the issue severity: failed → error (critical when the policy
/// escalates), warning and broken → warning; passed and skipped file
/// nothing and close prior issues.
// spec: CHK#checks-with-instances
#[allow(clippy::too_many_arguments)]
async fn file_health_events(
	conn: &mut AsyncPgConnection,
	server_id: Option<Uuid>,
	machine_id: Uuid,
	group_id: Option<Uuid>,
	application_type: Option<&ApplicationType>,
	device_id: Option<Uuid>,
	status: &Status,
	tags: &std::collections::HashMap<String, serde_json::Value>,
	subject: SubjectSplit,
) -> Result<()> {
	let reported = ReportedCheck::all_in(&status.health);
	let occurred_at = Some(status.created_at);

	// Which grain a check on this push belongs to.
	//
	// On a unified push the check's own subject decides, Canopy having to
	// separate the grains itself. Where such a push names no application there
	// is no second grain to belong to: a box Canopy holds no application for
	// reports about itself, so the whole push is the box's, down to a check
	// whose name is not in the machine set.
	//
	// On a split push the reporter already separated them, so the target this
	// filing is for is the answer whatever the check is named. A machine check
	// name reported under an application is that application's check, which is
	// what lets a reporter carry a check Canopy does not recognise as
	// machine-subject.
	// spec: STA#transitional-unified-pushes
	let on_machine = |check: &str| match subject {
		SubjectSplit::BySubject => server_id.is_none() || CheckSubject::of(check).is_machine(),
		SubjectSplit::AsGiven => server_id.is_none(),
	};
	// The namespace follows the grain the check files at, never its name: a
	// check filed against an application is that application type's entry.
	// spec: CHK#names
	let namespace_of = |check: &str| {
		Namespace::of(
			&status.source,
			application_type.filter(|_| !on_machine(check)),
		)
	};
	// The scope a check files at. A check off the machine always has an
	// application to file against: with no application, every check is the
	// machine's.
	let scope_of = |check: &str| match server_id {
		Some(server_id) if !on_machine(check) => Scope::Application(server_id),
		_ => Scope::Machine(machine_id),
	};

	// Upsert a catalog row for every check name seen on this push,
	// whatever its result. New checks land at the default warning
	// ceiling; operators can review and adjust from the /healthchecks
	// page. A name resolves to the namespace of the grain it files at,
	// so a machine check on a Tamanu push is the box's entry and not one
	// Tamanu owns.
	for check_name in reported.keys() {
		let namespace = namespace_of(check_name);
		CheckPolicy::upsert_default(conn, &status.source, &namespace, check_name).await?;
	}

	// Status-level extras are shared across every per-check evaluation, and
	// rules read them for every instance of every check.
	let empty_map = serde_json::Map::new();
	let status_extra = status.extra.as_object().unwrap_or(&empty_map);
	fn context<'a>(
		status: &'a Status,
		status_extra: &'a serde_json::Map<String, serde_json::Value>,
		tags: &'a std::collections::HashMap<String, serde_json::Value>,
		check: &'a str,
	) -> GradingContext<'a> {
		GradingContext {
			source: &status.source,
			check,
			status_extra,
			tags,
		}
	}

	// Grade every check in the push through its policy.
	let mut effective: BTreeMap<&String, GradedCheck> = BTreeMap::new();
	for (check, reported) in &reported {
		let on_machine = on_machine(check);
		let namespace = namespace_of(check);
		let grading = CheckGrading::load(
			conn,
			&status.source,
			&namespace,
			check,
			FilingScope {
				// A unified push carries both grains' checks. Grade each at
				// the grain its subject belongs to, so a machine check is
				// graded against the box's tags and silenced by the box's
				// policy.
				// spec: STA
				application_id: (!on_machine).then_some(server_id).flatten(),
				machine_id: on_machine.then_some(machine_id),
				group_id,
				// A device push is a machine's; a cluster's checks arrive over
				// the relay path, never here.
				kubernetes_cluster_id: None,
			},
		)
		.await?;
		// A broken check holds the instances its state held, and keeps an open
		// failure, so only then is its state read before it is written. Read
		// at the grain the check files at: a machine check's open failure is
		// the box's issue, and looking for it among the application's would
		// never find it.
		let prior = match reported.outcome {
			CheckOutcome::Broken => {
				Issue::check_state_at(
					conn,
					scope_of(check),
					&status.source,
					&format!("{HEALTH_REF}/{check}"),
				)
				.await?
			}
			CheckOutcome::Instances(_) => None,
		};
		let mut graded = grade_instances(
			&grading,
			&context(status, status_extra, tags, check),
			Some(&reported.detail),
			&reported.outcome,
			prior.as_ref(),
		);
		// A check that ran but that policy graded broken keeps an open failure
		// as a reported-broken one does, so its state is read once that is
		// known.
		// spec: CHK#stability
		if prior.is_none() && graded.effective == CheckResult::Broken {
			let prior = Issue::check_state_at(
				conn,
				scope_of(check),
				&status.source,
				&format!("{HEALTH_REF}/{check}"),
			)
			.await?;
			graded.retain_through_brokenness(prior.as_ref());
		}
		effective.insert(check, graded);
	}

	// The pushing source's previously-open issues: consulted for close
	// messages ("recovered" vs "was never trouble") and for the
	// unmentioned-check closes below.
	//
	// One set per grain. Sharing a single set would make a check that moves
	// grain read as unmentioned on the grain it left, closing and reopening it
	// as a new issue on every push.
	let health_prefix = format!("{HEALTH_REF}/");
	let strip = |refs: Vec<String>| -> BTreeSet<String> {
		refs.into_iter()
			.filter_map(|r| {
				r.strip_prefix(&health_prefix)
					.map(|check| check.to_string())
			})
			.collect()
	};
	//
	// Only the grains this filing speaks for are consulted. A split push's
	// application filing says nothing about the machine's checks, so reading
	// the machine's open issues here would close every one of them as
	// unmentioned.
	let handles_machine = server_id.is_none() || matches!(subject, SubjectSplit::BySubject);
	let previously_active = match server_id {
		Some(server_id) => strip(
			Issue::active_refs_with_prefix(conn, server_id, &status.source, &health_prefix).await?,
		),
		None => Default::default(),
	};
	let previously_active_on_machine = if handles_machine {
		strip(
			Issue::active_refs_with_prefix_for_machine(
				conn,
				machine_id,
				&status.source,
				&health_prefix,
			)
			.await?,
		)
	} else {
		Default::default()
	};

	// File every check in the push — passing ones included, so the state
	// row records the current result and when it was last reported. An
	// effective broken result neither confirms nor clears the previous
	// definite result: the grading retains an open effective failure's
	// contribution, or counts as a warning when there was nothing to
	// retain (broken contributes as a warning in the rollups).
	//
	// Degraded checks file before recoveries: when one failure swaps for
	// another in a single push, the incoming failure must join the open
	// incident before the outgoing one leaves, or the incident closes
	// and reopens as two.
	let degraded = |g: &GradedCheck| {
		matches!(
			g.effective,
			CheckResult::Warning | CheckResult::Failed | CheckResult::Broken
		)
	};
	let filing_order = effective
		.iter()
		.filter(|(_, g)| degraded(g))
		.chain(effective.iter().filter(|(_, g)| !degraded(g)));
	for (check, graded) in filing_order {
		let on_machine = on_machine(check);
		let was_active = if on_machine {
			previously_active_on_machine.contains(*check)
		} else {
			previously_active.contains(*check)
		};
		let active = degraded(graded);
		let (description, title, message) = if graded.is_plain() {
			// A check with a single result is headlined by what policy made of
			// it. A broken check retaining a failure is still broken, so the
			// headline reads its instance's grade rather than the retained
			// contribution.
			let description = match graded.instances[0].effective {
				_ if !active => None,
				CheckResult::Broken => Some(format!("Health check '{check}' is broken")),
				CheckResult::Failed => Some(format!("Health check '{check}' failed")),
				_ => Some(format!("Health check '{check}' warned")),
			};
			let message = match graded.effective {
				_ if active => per_check_description(&reported[*check].detail),
				CheckResult::Skipped if was_active => {
					Some(format!("Health check '{check}' is now skipped"))
				}
				CheckResult::Skipped => Some(format!("Health check '{check}' skipped")),
				_ if was_active => Some(format!("Health check '{check}' recovered")),
				_ => Some(format!("Health check '{check}' passing")),
			};
			(
				description.clone(),
				description,
				message.unwrap_or_default(),
			)
		} else {
			// A check with instances is headlined the same whatever its
			// instances come to, and the title is kept whatever the result:
			// lifting an instance silence can bring the state back into
			// trouble at any grade, and it presents this title then. Canopy
			// writes the message from the graded instances, so an instance
			// a silence has taken out is never counted in it.
			// spec: CHK#checks-with-instances
			let title = if graded.broken {
				format!("Health check '{check}' is broken")
			} else {
				format!("Health check '{check}' is degraded")
			};
			(
				active.then(|| title.clone()),
				Some(title),
				graded.message(check),
			)
		};
		let stamp = CheckStateStamp::of_graded(
			check,
			graded,
			&context(status, status_extra, tags, check),
			title.as_deref(),
		);
		let r#ref = format!("{HEALTH_REF}/{check}");
		if on_machine {
			// A degraded machine check is one issue at machine scope however
			// many applications run on the box. Incident evaluation happens
			// inside this call rather than through the deferred queue, which
			// is keyed by application.
			database::issues::raise_machine_event_with_state(
				conn,
				machine_id,
				&status.source,
				device_id,
				&r#ref,
				description.as_deref(),
				&message,
				active,
				Some(&stamp),
			)
			.await?;
		} else if let Some(server_id) = server_id {
			// Always bound here: a push with no application has no check that
			// is not the machine's.
			NewEvent {
				source: status.source.clone(),
				r#ref,
				description,
				message,
				active: Some(active),
				occurred_at,
			}
			.save_with_state(conn, server_id, device_id, Some(&stamp), true)
			.await?;
		}
	}

	// Unmentioned closes: a check the source previously reported but
	// omits from this push has recovered ("trust the reporter"). Scoped
	// to the pushing source: one source's push says nothing about
	// another's checks.
	//
	// Walked per grain, against that grain's own previously-active set.
	for (check, on_machine) in previously_active
		.iter()
		.map(|c| (c, false))
		.chain(previously_active_on_machine.iter().map(|c| (c, true)))
	{
		if reported.contains_key(check) {
			continue;
		}
		let stamp = CheckStateStamp {
			check: check.clone(),
			observed: CheckResult::Passed,
			effective: CheckResult::Passed,
			escalates: false,
			detail: None,
			title: None,
			instanced: None,
		};
		let r#ref = format!("{HEALTH_REF}/{check}");
		let message = format!("Health check '{check}' recovered");
		if on_machine {
			database::issues::raise_machine_event_with_state(
				conn,
				machine_id,
				&status.source,
				device_id,
				&r#ref,
				None,
				&message,
				false,
				Some(&stamp),
			)
			.await?;
		} else if let Some(server_id) = server_id {
			NewEvent {
				source: status.source.clone(),
				r#ref,
				description: None,
				message,
				active: Some(false),
				occurred_at,
			}
			.save_with_state(conn, server_id, device_id, Some(&stamp), true)
			.await?;
		}
	}

	// Incident (re-)evaluation is deferred off this request: the issue state
	// above is recorded synchronously, but the incident work — which takes
	// the per-group `server_groups` lock — is handed to the reeval worker so
	// concurrent check-ins never convoy on that lock. Only grouped applications
	// participate in incidents.
	if let Some(server_id) = server_id
		&& group_id.is_some()
	{
		database::issues::enqueue_incident_reeval(conn, server_id).await?;
	}

	Ok(())
}

/// A degraded check's message: its fields, one per line.
fn per_check_description(detail: &serde_json::Map<String, serde_json::Value>) -> Option<String> {
	let mut lines = Vec::new();
	for (k, v) in detail {
		let rendered = match v {
			serde_json::Value::String(s) => s.clone(),
			other => other.to_string(),
		};
		lines.push(format!("- **{k}**: `{rendered}`"));
	}
	(!lines.is_empty()).then(|| lines.join("\n"))
}

/// One target's material, as parsed off the wire: its checks and its detail.
struct TargetReportBody {
	health: serde_json::Value,
	detail: serde_json::Value,
}

/// One reported application: what the reporter says it is, and its material.
struct ApplicationReportBody {
	r#type: ApplicationType,
	report: TargetReportBody,
}

/// What a push carries, once its shape is known.
// spec: STA#transitional-unified-pushes
enum PushBody {
	/// The current format: the reporter separated the machine's material from
	/// each application's, and named each application by a key of its own.
	Split {
		machine: TargetReportBody,
		applications: BTreeMap<String, ApplicationReportBody>,
	},
	/// The transitional unified format: one set of checks and one flat body,
	/// describing a box assumed to run a single application. Canopy separates
	/// the two grains itself.
	Unified {
		health: serde_json::Value,
		extra: serde_json::Value,
	},
	/// A legacy Tamanu direct report: no health field at all.
	Legacy { extra: serde_json::Value },
}

/// A parsed push: the fields common to every shape, and the shape itself.
struct ParsedPush {
	source: String,
	healthy: bool,
	body: PushBody,
}

/// Pulls the reserved keys out of the incoming status body and decides which
/// shape it is in. Validates types per the contract:
///
/// - missing or `null` body → `source = alertd`, `healthy = true`, a legacy
///   report with `extra = {}`
/// - `source` absent ⇒ `alertd` (transitional — the field will become
///   mandatory); present must be a non-empty string and not one of the
///   reserved names (`canopy`, `manual`)
/// - `healthy` absent ⇒ `true` (legacy compat — non-negotiable, this is
///   what stops every legacy server from false-positiving unhealthy on
///   the day we deploy)
/// - `healthy` present must be a bool
/// - a `machine` key puts the push in the current format: `machine` and each
///   value of `applications` must be an object with an optional `health` array
///   and an optional `detail` object, and each application must name a
///   `type`. Any other top-level key is ignored, the format having no place
///   for one.
/// - otherwise a `health` key makes it a unified push and its absence makes it
///   a legacy one; the rest of the body is that push's flat detail.
///
/// A `health` array, wherever it appears, must be an array of objects, each
/// with `check: non-empty string` and **exactly one** of
/// `result: "passed" | "warning" | "failed" | "broken" | "skipped"`
/// (current bestool), `healthy: bool` (legacy), or an `instances` object. An
/// unrecognised `result` string is a 400 — canopy ships before any bestool
/// that adds enum values. A check's fields sit in a `detail` object or, for a
/// check with a single result, flat beside it, never both; see
/// [`parse_health`] for every refusal.
fn parse_push(raw: serde_json::Value) -> Result<ParsedPush> {
	let mut obj = match raw {
		serde_json::Value::Null => serde_json::Map::new(),
		serde_json::Value::Object(m) => m,
		_ => {
			return Err(AppError::BadRequest(
				"status body must be a JSON object (or null/empty)".into(),
			));
		}
	};

	let source = match obj.remove("source") {
		None => DEFAULT_SOURCE.to_string(),
		Some(serde_json::Value::String(s)) if !s.is_empty() => {
			if is_reserved(&s) {
				return Err(AppError::BadRequest(format!(
					"`source` must not be a reserved name ({})",
					RESERVED_SOURCES.join(", "),
				)));
			}
			s
		}
		Some(_) => {
			return Err(AppError::BadRequest(
				"`source` must be a non-empty string".into(),
			));
		}
	};

	let healthy = match obj.remove("healthy") {
		None => true,
		Some(serde_json::Value::Bool(b)) => b,
		Some(_) => return Err(AppError::BadRequest("`healthy` must be a boolean".into())),
	};

	// A `machine` section is what says the reporter separated the grains
	// itself. Its presence, not its contents: a box with nothing to say about
	// itself still says which shape it speaks.
	let body = if let Some(machine) = obj.remove("machine") {
		let machine = parse_target_report(machine, "machine")?;
		let applications = match obj.remove("applications") {
			None => BTreeMap::new(),
			Some(serde_json::Value::Object(map)) => {
				let mut out = BTreeMap::new();
				for (key, value) in map {
					if key.is_empty() {
						return Err(AppError::BadRequest(
							"`applications` keys must be non-empty strings".into(),
						));
					}
					let path = format!("applications.{key}");
					let mut value = match value {
						serde_json::Value::Object(v) => v,
						_ => {
							return Err(AppError::BadRequest(format!(
								"`{path}` must be an object"
							)));
						}
					};
					let r#type = match value.remove("type") {
						Some(serde_json::Value::String(t)) => {
							t.parse::<ApplicationType>().map_err(|_| {
								AppError::BadRequest(format!(
									"`{path}.type` must be a lowercase dashed slug",
								))
							})?
						}
						_ => {
							return Err(AppError::BadRequest(format!(
								"`{path}.type` must be a non-empty string",
							)));
						}
					};
					let report = parse_target_report(serde_json::Value::Object(value), &path)?;
					out.insert(key, ApplicationReportBody { r#type, report });
				}
				out
			}
			Some(_) => {
				return Err(AppError::BadRequest(
					"`applications` must be an object".into(),
				));
			}
		};
		PushBody::Split {
			machine,
			applications,
		}
	} else {
		match obj.remove("health") {
			Some(health) => PushBody::Unified {
				health: parse_health(health, "health")?,
				extra: serde_json::Value::Object(obj),
			},
			// A push without a `health` key is the legacy Tamanu
			// direct-report format; the caller transforms it into the
			// tamanu/tasks heartbeat.
			None => PushBody::Legacy {
				extra: serde_json::Value::Object(obj),
			},
		}
	};

	Ok(ParsedPush {
		source,
		healthy,
		body,
	})
}

/// One target's `health` and `detail`, each optional and each defaulting to
/// the empty form: a reporter with nothing to say about a target still says so
/// by naming it, and an absent `health` recovers what it last reported exactly
/// as an empty one does.
fn parse_target_report(value: serde_json::Value, path: &str) -> Result<TargetReportBody> {
	let mut obj = match value {
		serde_json::Value::Object(m) => m,
		_ => return Err(AppError::BadRequest(format!("`{path}` must be an object"))),
	};
	let health = match obj.remove("health") {
		None => serde_json::Value::Array(Vec::new()),
		Some(health) => parse_health(health, &format!("{path}.health"))?,
	};
	let detail = match obj.remove("detail") {
		None => serde_json::Value::Object(serde_json::Map::new()),
		Some(detail @ serde_json::Value::Object(_)) => detail,
		Some(_) => {
			return Err(AppError::BadRequest(format!(
				"`{path}.detail` must be an object",
			)));
		}
	};
	Ok(TargetReportBody { health, detail })
}

/// Validate one `health` array, wherever in the payload it sits. `path` names
/// it for the error message, so a reporter is told which target it got wrong.
///
/// The array is returned as sent: status history records the push verbatim,
/// and [`ReportedCheck`] reads it back in one form whichever form each check
/// took.
// spec: STA#health-and-detail
// spec: STA#instances
fn parse_health(value: serde_json::Value, path: &str) -> Result<serde_json::Value> {
	let health_arr = match value {
		serde_json::Value::Array(a) => a,
		_ => return Err(AppError::BadRequest(format!("`{path}` must be an array"))),
	};
	for (idx, entry) in health_arr.iter().enumerate() {
		parse_health_entry(entry, &format!("{path}[{idx}]"))?;
	}
	Ok(serde_json::Value::Array(health_arr))
}

/// Validate one check of a `health` array.
fn parse_health_entry(entry: &serde_json::Value, path: &str) -> Result<()> {
	let bad = |message: String| Err(AppError::BadRequest(message));
	let Some(entry) = entry.as_object() else {
		return bad(format!("`{path}` must be an object"));
	};
	match entry.get("check") {
		Some(serde_json::Value::String(s)) if !s.is_empty() => {}
		Some(_) | None => return bad(format!("`{path}.check` must be a non-empty string")),
	}

	let has_detail = entry.get("detail").is_some_and(|d| d.is_object());
	// Fields beside the structure are the flat form of a check's detail, which
	// only a check with a single result may use, and only in place of `detail`.
	let flat = entry
		.iter()
		.filter(|(k, v)| !is_health_structure(k, v))
		.map(|(k, _)| format!("`{k}`"))
		.collect::<Vec<_>>();

	match (
		entry.get("result"),
		entry.get("healthy"),
		entry.get("instances").and_then(|i| i.as_object()),
	) {
		(Some(_), Some(_), None) => {
			return bad(format!(
				"`{path}` must not have both `result` and `healthy`"
			));
		}
		(Some(_), _, Some(_)) | (_, Some(_), Some(_)) => {
			return bad(format!(
				"`{path}` must have exactly one of `result`, `healthy` or `instances`",
			));
		}
		(Some(serde_json::Value::String(s)), None, None) => {
			if s.parse::<CheckResult>().is_err() {
				return bad(format!(
					"`{path}.result` must be one of passed, warning, failed, broken, skipped",
				));
			}
		}
		(Some(_), None, None) => return bad(format!("`{path}.result` must be a string")),
		(None, Some(serde_json::Value::Bool(_)), None) => {}
		(None, Some(_), None) => return bad(format!("`{path}.healthy` must be a boolean")),
		(None, None, Some(instances)) => {
			if !flat.is_empty() {
				return bad(format!(
					"`{path}` has instances, so its fields belong in `detail`, not beside them: {}",
					flat.join(", "),
				));
			}
			if instances.len() > MAX_INSTANCES_PER_CHECK {
				return bad(format!(
					"`{path}.instances` has {} instances, more than the {MAX_INSTANCES_PER_CHECK} a check may carry",
					instances.len(),
				));
			}
			for (key, instance) in instances {
				if key.is_empty() {
					return bad(format!("`{path}.instances` keys must be non-empty strings"));
				}
				if key.chars().count() > MAX_INSTANCE_NAMING_CHARS {
					return bad(format!(
						"`{path}.instances` keys must be at most {MAX_INSTANCE_NAMING_CHARS} characters",
					));
				}
				parse_health_instance(instance, &format!("{path}.instances.{key}"))?;
			}
			return Ok(());
		}
		(None, None, None) => {
			return bad(format!(
				"`{path}` must have a `result` (or legacy `healthy`), or `instances`",
			));
		}
	}

	if has_detail && !flat.is_empty() {
		return bad(format!(
			"`{path}` has its fields both in `detail` and beside it ({}); send them one way",
			flat.join(", "),
		));
	}
	Ok(())
}

/// Validate one instance of a check.
fn parse_health_instance(instance: &serde_json::Value, path: &str) -> Result<()> {
	let bad = |message: String| Err(AppError::BadRequest(message));
	let Some(instance) = instance.as_object() else {
		return bad(format!("`{path}` must be an object"));
	};
	if instance.contains_key("healthy") {
		return bad(format!(
			"`{path}` must report a `result`; the legacy `healthy` is not accepted on an instance",
		));
	}
	match instance.get("result") {
		Some(serde_json::Value::String(s)) => match s.parse::<CheckResult>() {
			Ok(CheckResult::Broken) => {
				return bad(format!(
					"`{path}.result` must not be broken: brokenness is the whole check's, so report `result: broken` on the check, without instances",
				));
			}
			Ok(_) => {}
			Err(_) => {
				return bad(format!(
					"`{path}.result` must be one of passed, warning, failed, skipped",
				));
			}
		},
		Some(_) => return bad(format!("`{path}.result` must be a string")),
		None => return bad(format!("`{path}` must have a `result`")),
	}
	match instance.get("label") {
		None => {}
		Some(serde_json::Value::String(label)) => {
			if label.chars().count() > MAX_INSTANCE_NAMING_CHARS {
				return bad(format!(
					"`{path}.label` must be at most {MAX_INSTANCE_NAMING_CHARS} characters",
				));
			}
		}
		Some(_) => return bad(format!("`{path}.label` must be a string")),
	}
	if let Some(detail) = instance.get("detail")
		&& !detail.is_object()
	{
		return bad(format!("`{path}.detail` must be an object"));
	}
	Ok(())
}
