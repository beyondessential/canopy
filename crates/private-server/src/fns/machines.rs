//! Operator endpoints for machines: the hosts applications run on.
//!
//! A machine is what an operator creates and places in a group; the
//! applications on it arrive by report and take the machine's group. So the
//! writes here are the group, the box's location, and its monitoring
//! settings — never an application's type or version.
//!
//! Identity is deliberately absent: an identity is bound by enrolment, not by
//! an operator editing a form.

use axum::Json;
use axum::extract::State;
use base64::Engine;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{backup_jobs::BillingLabels, tailscale_auth::TailscaleAdmin};
use commons_types::{
	Uuid,
	device::DeviceRole,
	geo::GeoPoint,
	server::TagMap,
	status::{HealthState, ShortStatus},
};
use database::applications::Application;
use database::devices::{Device, TailscaleIdentity};
use database::issues::Scope;
use jiff::Timestamp;

use database::machine_enrollment_tokens::MachineEnrollmentToken;
use database::machines::{Machine, MachineUpdate, NewMachine};
use database::maintenance_windows::MaintenanceWindow;
use database::pg_duration::PgDuration;
use database::reported_detail::MachineReportedDetail;
use database::server_groups::ServerGroup;
use database::statuses::MergedDetail;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(list))
		.routes(routes!(get))
		.routes(routes!(get_detail))
		.routes(routes!(create))
		.routes(routes!(update))
		.routes(routes!(archive))
		.routes(routes!(attach_tailscale_device))
		.routes(routes!(mint_enrollment))
		.routes(routes!(revoke_enrollment))
		.routes(routes!(enrollment_status))
}

/// Identifies one machine.
#[derive(Deserialize, ToSchema)]
pub struct MachineIdArgs {
	/// The machine to operate on.
	pub machine_id: Uuid,
}

/// A machine together with the applications running on it.
#[derive(Serialize, ToSchema)]
pub struct MachineDetail {
	#[serde(flatten)]
	pub machine: Machine,
	/// The applications on this machine. Empty for a machine that has been
	/// created but has not yet reported: that is awaiting check-in, not an
	/// error.
	pub applications: Vec<Application>,
}

/// List the live fleet's machines.
///
/// Every machine that has not been archived, ordered by name. A machine with
/// no applications on it is included: one created but not yet reporting is
/// awaiting check-in, not an error. The request body is ignored; send an empty
/// JSON object.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "machines_list",
	tag = "machines",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Vec<Machine>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn list(State(state): State<AppState>) -> Result<Json<Vec<Machine>>> {
	let mut conn = state.db_read.get().await?;
	Ok(Json(Machine::list_live(&mut conn).await?))
}

/// Get one machine and the applications running on it.
///
/// Returns the machine's own facts — its group, where it is, how long it may
/// be silent — together with the applications it hosts. Returns 404 if the
/// machine doesn't exist.
#[utoipa::path(
	post,
	path = "/get",
	operation_id = "machines_get",
	tag = "machines",
	security(("tailscale-user" = [])),
	request_body = MachineIdArgs,
	responses(
		(status = 200, body = MachineDetail),
		(status = 404, body = ProblemDetailsSchema),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get(
	State(state): State<AppState>,
	Json(args): Json<MachineIdArgs>,
) -> Result<Json<MachineDetail>> {
	let mut conn = state.db_read.get().await?;
	let machine = Machine::get_by_id(&mut conn, args.machine_id).await?;
	let applications = machine.applications(&mut conn).await?;
	Ok(Json(MachineDetail {
		machine,
		applications,
	}))
}

/// A machine's effective billing labels.
///
/// A box is not a piece of software, so it carries no product. Its stage is the
/// highest rank among the applications on it — a box shared by a production and
/// a test workload bills as production — and its deployment label comes from its
/// group. An ungrouped machine carries no attribution at all, there being no
/// group to attribute it to.
// spec: APP#billing-attribution
async fn machine_billing_labels(
	machine: &Machine,
	group: Option<&ServerGroup>,
	applications: &[super::applications::ServerInfo],
) -> Vec<super::server_groups::BillingTag> {
	let Some(group) = group else {
		return Vec::new();
	};
	let highest_rank = applications
		.iter()
		.filter_map(|a| a.rank)
		.min_by_key(|r| database::server_groups::rank_priority(Some(*r)));
	BillingLabels::from_group(&machine.tags, &group.name, None, highest_rank)
		.into_tags()
		.into_iter()
		.map(|(key, value)| super::server_groups::BillingTag { key, value })
		.collect()
}

/// Everything a machine's own page presents.
///
/// The machine's record, what the box reports about itself, its own health and
/// checks, the identity it authenticates with, and the applications running on
/// it. An application's version and database engine are not here: those are the
/// workload's, and each application carries its own (see [APP]).
#[derive(Serialize, ToSchema)]
pub struct MachineDetailData {
	/// The machine's own record.
	pub machine: Machine,
	/// The group this machine belongs to, with its notes and tags, so the page
	/// renders its group section without a second fetch.
	pub group: Option<ServerGroup>,
	/// Full detail on the identity bound to this machine, if it has enrolled.
	pub device_info: Option<super::devices::DeviceInfo>,
	/// What the box reports about itself, resolved across every source
	/// reporting on it: platform, hardware, addresses, uptime.
	// spec: FIG#sourcing
	#[schema(value_type = Object)]
	pub figures: serde_json::Value,
	/// When the box last reported anything, across every source.
	pub last_reported_at: Option<jiff::Timestamp>,
	/// Whether the box is known to run Munin, from the most recent source to
	/// report the flag. Munin watches the box rather than anything running on
	/// it, so the flag and the link it drives are the machine's.
	// spec: SVC#munin-link
	pub munin: bool,
	/// Whether the box is currently reporting, on its own threshold.
	pub up: ShortStatus,
	/// The machine's own health, from the checks filed against it. What the
	/// applications on it make of their own checks is each application's.
	pub health: HealthState,
	/// Whether a maintenance window suspends this machine, its own or its
	/// group's.
	// spec: MNT#presentation
	pub maintained: bool,
	/// Whether the suspension is only the settle period.
	pub maintenance_settling: bool,
	/// The machine's own checks across every source, graded and classified.
	pub checks: commons_types::status::ConsolidatedChecks,
	/// The people logged in to this box right now, from its `external_users`
	/// check. Empty unless the box is currently reporting: a stale report
	/// cannot say who is on it now.
	// spec: FLT
	pub operators: Vec<commons_types::status::OperatorPresence>,
	/// The applications running on this box, each carrying its own
	/// reachability and health so the page renders a dot per workload.
	pub applications: Vec<super::applications::ServerInfo>,
	/// Every application in this box's group, for the tree the page ends with.
	/// Empty when the machine is ungrouped. The applications on this box are
	/// among them, the tree being a map of the group rather than of elsewhere.
	// spec: FLT
	pub group_applications: Vec<super::applications::ServerInfo>,
	/// Every machine in this box's group, for the same tree. Empty when the
	/// machine is ungrouped.
	// spec: FLT
	pub group_machines: Vec<super::server_groups::GroupMachine>,
	/// The machine's effective `billing.*` labels — the ones Canopy hands its
	/// device. A machine carries no product, a box not being a piece of
	/// software.
	// spec: APP#billing-attribution
	pub billing_labels: Vec<super::server_groups::BillingTag>,
}

/// Get full detail for one machine.
///
/// Returns the box's record, what it reports about itself, its identity, its
/// own health and checks, and the applications running on it. Returns 404 if
/// the machine doesn't exist.
#[utoipa::path(
	post,
	path = "/get_detail",
	operation_id = "machines_get_detail",
	tag = "machines",
	security(("tailscale-user" = [])),
	request_body = MachineIdArgs,
	responses(
		(status = 200, body = MachineDetailData),
		(status = 404, body = ProblemDetailsSchema),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn get_detail(
	State(state): State<AppState>,
	Json(args): Json<MachineIdArgs>,
) -> Result<Json<MachineDetailData>> {
	let mut conn = state.db.get().await?;
	let machine = Machine::get_by_id(&mut conn, args.machine_id).await?;

	let group = match machine.group_id {
		Some(gid) => Some(ServerGroup::get_by_id(&mut conn, gid).await?),
		None => None,
	};

	// Every source's current report on this box, folded into one view. The
	// same resolution the application grain uses, so a field one source
	// omitted is still here.
	// spec: FIG#sourcing
	let reports = MachineReportedDetail::for_machine(&mut conn, machine.id).await?;
	let last_reported_at = reports.iter().map(|r| r.reported_at).max();
	let merged = MergedDetail::from_reports(reports.iter().map(|r| (r.reported_at, &r.extra)));
	// spec: SVC#munin-link
	let munin = merged.munin().unwrap_or(false);
	let figures = merged.into_json();

	// One consolidated read drives both the headline health and the checks
	// table, so they cannot disagree.
	let checks = database::issues::consolidated_checks_latest_for_machine(
		&mut conn,
		machine.id,
		machine.group_id,
	)
	.await?;
	let health = checks.health_state;
	let up = machine.reachability(last_reported_at);

	// Who is on the box, from the same consolidated read. Withheld unless the
	// box is reporting, since sessions from an old push say nothing about now.
	let mut operators = match up {
		ShortStatus::Up => checks.operators(),
		_ => Vec::new(),
	};
	super::statuses::enrich_operators(&mut conn, operators.iter_mut()).await?;

	let device_info = match machine.device_id {
		Some(did) => {
			let with_info = Device::get_with_info(&mut conn, did).await?;
			Some(super::devices::DeviceInfo::from_db(with_info, &state).await)
		}
		None => None,
	};

	// The box's own window, its group's, or its environment's, the same set an
	// application on it is judged by, since taking the box down stops the
	// workload too.
	let maintained =
		MaintenanceWindow::suspends(&mut conn, Some(machine.id), machine.group_id).await?;
	let maintenance_settling = maintained && {
		let mut open = MaintenanceWindow::open_for(&mut conn, Scope::Machine(machine.id), None)
			.await?
			.is_some();
		if !open && let Some(gid) = machine.group_id {
			open = MaintenanceWindow::open_for(&mut conn, Scope::Group(gid), None)
				.await?
				.is_some();
		}
		if !open
			&& let Some(gid) = machine.group_id
			&& let Some(rank) = database::machines::Machine::rank(&mut conn, machine.id).await?
		{
			open = MaintenanceWindow::open_for(&mut conn, Scope::Group(gid), Some(rank))
				.await?
				.is_some();
		}
		!open
	};

	let mut applications: Vec<super::applications::ServerInfo> = machine
		.applications(&mut conn)
		.await?
		.into_iter()
		.map(super::applications::server_to_info)
		.collect();
	for info in applications.iter_mut() {
		info.group_name = group.as_ref().map(|g| g.name.clone());
	}
	super::applications::decorate_with_status(&mut conn, &mut applications).await?;
	super::applications::fill_display_hosts(&mut conn, &mut applications).await?;

	let billing_labels = machine_billing_labels(&machine, group.as_ref(), &applications).await;

	let (group_applications, group_machines) = match group.as_ref() {
		Some(g) => super::server_groups::tree_members(&mut conn, g).await?,
		None => (Vec::new(), Vec::new()),
	};

	Ok(Json(MachineDetailData {
		machine,
		group,
		device_info,
		figures,
		last_reported_at,
		munin,
		up,
		health,
		maintained,
		maintenance_settling,
		checks,
		operators,
		applications,
		group_applications,
		group_machines,
		billing_labels,
	}))
}

/// What an operator supplies when adding a machine.
///
/// The group is the only field that matters to get right up front: which
/// group a box belongs to is the one thing the box has no way of knowing.
#[derive(Deserialize, ToSchema)]
pub struct MachineCreateArgs {
	/// What to call the box. Distinct from the hostname its operating system
	/// reports, which arrives as a reported figure.
	pub name: Option<String>,
	/// The group this machine belongs to. The applications on it take it.
	pub group_id: Option<Uuid>,
	/// Whether the box is cloud-hosted, if known.
	pub cloud: Option<bool>,
	/// Where the box is, if known.
	pub geolocation: Option<GeoPoint>,
	/// Whether the machine's own checks alert. Defaults to on.
	pub is_monitored: Option<bool>,
	/// How long the box may be silent before it is unreachable, in seconds.
	/// Defaults to the column's own default.
	pub alert_when_down_for: Option<i64>,
	/// Operator notes shown on the machine's page.
	pub notes: Option<String>,
	/// Operator-set tags. Reserved names are refused, as they are on an edit.
	pub tags: Option<TagMap>,
	/// A tailnet node to bind this machine to up front, by address, node key,
	/// or DNS name. Omit to enrol the machine later.
	///
	/// The operator is describing a box they can already see on the tailnet,
	/// so binding it here saves an enrolment round trip. The identity is a
	/// machine's, not an application's: the applications on the box are
	/// reported and carry no identity of their own.
	pub tailscale_identifier: Option<String>,
}

/// Add a machine to the fleet.
///
/// The machine starts with no applications and no identity; enrolment binds an
/// identity, and the applications on it arrive by report.
#[utoipa::path(
	post,
	path = "/create",
	operation_id = "machines_create",
	tag = "machines",
	security(("tailscale-user" = [])),
	request_body = MachineCreateArgs,
	responses(
		(status = 200, body = Uuid),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn create(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineCreateArgs>,
) -> Result<Json<Uuid>> {
	let mut conn = state.db.get().await?;

	// Bind the tailnet node first: a machine created and then failed to bind
	// would leave a record the operator did not ask for.
	let device_id = match args.tailscale_identifier.as_deref() {
		None => None,
		Some(identifier) => Some(resolve_tailnet_device(&state, &mut conn, identifier).await?),
	};

	let machine = Machine::create(
		&mut conn,
		NewMachine {
			name: args.name,
			group_id: args.group_id,
			cloud: args.cloud,
			geolocation: args.geolocation,
		},
	)
	.await?;

	// The rest of the form is an edit applied to the machine just made, so
	// creating and editing cannot disagree about what a field means.
	Machine::update(
		&mut conn,
		machine.id,
		MachineUpdate {
			is_monitored: args.is_monitored,
			alert_when_down_for: args
				.alert_when_down_for
				.map(|s| PgDuration(jiff::SignedDuration::from_secs(s))),
			notes: args.notes,
			tags: args.tags,
			..Default::default()
		},
	)
	.await?;

	if let Some(device_id) = device_id {
		Machine::bind_device(&mut conn, machine.id, device_id).await?;
	}

	Ok(Json(machine.id))
}

/// Fields to change on a machine. Omitted fields are left alone; for the
/// nullable ones an explicit `null` clears the value.
#[derive(Deserialize, ToSchema)]
pub struct MachineUpdateArgs {
	/// The machine to edit.
	pub machine_id: Uuid,
	/// New name for the box, or `null` to clear it.
	pub name: Option<Option<String>>,
	/// New group, or `null` to remove it from its current one. The
	/// applications on this machine move with it.
	pub group_id: Option<Option<Uuid>>,
	/// New value for whether the box is cloud-hosted, or `null` to clear it.
	pub cloud: Option<Option<bool>>,
	/// New location for the box, or `null` to clear it.
	pub geolocation: Option<Option<GeoPoint>>,
	/// New monitored state. Switching it off quiets the machine's own checks
	/// and leaves the applications on it alone.
	pub is_monitored: Option<bool>,
	/// How long the machine may be silent before it is unreachable, in seconds.
	pub alert_when_down_for: Option<i64>,
	/// New free-form operator notes for the box.
	pub notes: Option<String>,
	/// New set of key/value tags. Replaces the whole set.
	pub tags: Option<TagMap>,
}

/// Edit a machine.
///
/// Moving a machine to another group moves the applications on it: an
/// application's group is never set independently of its machine's.
#[utoipa::path(
	post,
	path = "/update",
	operation_id = "machines_update",
	tag = "machines",
	security(("tailscale-user" = [])),
	request_body = MachineUpdateArgs,
	responses(
		(status = 200, body = Machine),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn update(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineUpdateArgs>,
) -> Result<Json<Machine>> {
	let mut conn = state.db.get().await?;
	let updated = Machine::update(
		&mut conn,
		args.machine_id,
		MachineUpdate {
			name: args.name,
			group_id: args.group_id,
			cloud: args.cloud,
			geolocation: args.geolocation,
			is_monitored: args.is_monitored,
			alert_when_down_for: args.alert_when_down_for.map(|secs| {
				database::pg_duration::PgDuration(jiff::SignedDuration::from_secs(secs))
			}),
			notes: args.notes,
			tags: args.tags,
		},
	)
	.await?;
	Ok(Json(updated))
}

/// Archive a machine, and with it the applications on it.
///
/// A box going away takes its workloads with it. Archival is not deletion:
/// the records and their history remain.
#[utoipa::path(
	post,
	path = "/archive",
	operation_id = "machines_archive",
	tag = "machines",
	security(("tailscale-user" = [])),
	request_body = MachineIdArgs,
	responses(
		(status = 200, body = ()),
		(status = 404, body = ProblemDetailsSchema),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn archive(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Machine::archive(&mut conn, args.machine_id).await?;
	Ok(Json(()))
}

/// The identity for a tailnet node, reusing the one already on it or minting
/// one. An identity resolved this way holds the machine role: a tailnet node an
/// operator points at is a box.
// spec: FLT#identities
async fn resolve_tailnet_device(
	state: &AppState,
	conn: &mut database::diesel_async::AsyncPgConnection,
	identifier: &str,
) -> Result<Uuid> {
	let directory = state
		.tailnet_directory
		.as_ref()
		.ok_or(AppError::AuthTailnetDirectoryUnavailable)?;
	let entry = directory
		.resolve_identifier(identifier)
		.await
		.map_err(|_| AppError::AuthTailnetDirectoryUnavailable)?
		.ok_or_else(|| AppError::NotFound("no tailnet device matches that identifier".into()))?;

	Ok(
		match Device::from_tailscale_node_id(conn, &entry.node_id).await? {
			Some(existing) => existing.id,
			None => {
				Device::create_with_tailscale(
					conn,
					TailscaleIdentity {
						node_id: entry.node_id.clone(),
						node_name: Some(entry.node_name.clone()),
						tailnet: Some(entry.tailnet.clone()),
					},
					DeviceRole::Machine,
				)
				.await?
				.id
			}
		},
	)
}

/// Request to bind a machine to the identity of a Tailscale node.
#[derive(Deserialize, ToSchema)]
pub struct AttachTailscaleDeviceArgs {
	/// The machine to attach the identity to.
	pub machine_id: Uuid,
	/// Any of: a Tailscale CGNAT/ULA IP, a node id, or a DNS name.
	pub identifier: String,
}

/// Attach an identity to a machine via a Tailscale identifier.
///
/// Resolves the identifier to a tailnet node, finds the identity already on
/// that node or mints one for it, and binds it to the machine. Useful when an
/// operator can already see the box on the tailnet and wants to name it now
/// rather than wait for enrolment. `registered_at` stays unset: naming a box is
/// not the box arriving.
///
/// Returns 409 if the resolved identity already speaks for another live
/// machine; detach it there first.
#[utoipa::path(
	post,
	path = "/attach_tailscale_device",
	operation_id = "machines_attach_tailscale_device",
	tag = "machines",
	security(("tailscale-admin" = [])),
	request_body = AttachTailscaleDeviceArgs,
	responses(
		(status = 200, description = "Identity now bound to the machine.", body = Uuid, content_type = "application/json"),
		(status = 404, description = "Identifier does not resolve to a known tailnet node.", body = ProblemDetailsSchema),
		(status = 409, description = "The resolved identity already speaks for another machine.", body = ProblemDetailsSchema),
		(status = 503, description = "Tailnet directory not configured or unreachable.", body = ProblemDetailsSchema),
	),
)]
pub async fn attach_tailscale_device(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<AttachTailscaleDeviceArgs>,
) -> Result<Json<Uuid>> {
	let mut conn = state.db.get().await?;
	let device_id = resolve_tailnet_device(&state, &mut conn, &args.identifier).await?;

	// An identity belongs to at most one machine, so there is one row to check.
	// An archived machine has already released its identity, so it does not
	// stand in the way.
	if let Some(other) = Machine::get_by_device_id(&mut conn, device_id).await?
		&& other.id != args.machine_id
		&& other.deleted_at.is_none()
	{
		return Err(AppError::Conflict(format!(
			"identity {device_id} already speaks for another machine",
		)));
	}

	Machine::bind_device(&mut conn, args.machine_id, device_id).await?;
	Ok(Json(device_id))
}

/// Enrollment token lifetime: 7 days (human operational timescale).
const ENROLLMENT_TTL: jiff::SignedDuration = jiff::SignedDuration::from_hours(24 * 7);

/// A freshly-minted enrollment ticket: the encrypted enrollment payload and
/// the passphrase that decrypts it.
#[derive(Serialize, ToSchema)]
pub struct EnrollmentTicket {
	/// Base64 (standard) of the age-encrypted enrollment JSON to feed to
	/// `bestool canopy register`. Encrypted under `passphrase` (age/scrypt), so
	/// it is safe to copy around on its own.
	pub ticket: String,
	/// Freshly-generated 4-word passphrase that decrypts `ticket`. Share this
	/// out-of-band (a separate channel from the ticket itself).
	pub passphrase: String,
	/// When the enrollment token inside the ticket expires.
	pub expires_at: Timestamp,
}

/// Mint (or reissue) an enrollment ticket for a machine.
///
/// Creates a fresh enrollment token and returns it wrapped in a
/// passphrase-encrypted ticket the operator runs through bestool on the
/// enrolling machine, plus the 4-word passphrase that decrypts it. The
/// plaintext token lives only inside the encrypted ticket; reissuing
/// invalidates any prior token. Fails if the server is archived.
#[utoipa::path(
	post,
	path = "/mint_enrollment",
	tag = "machines",
	security(("tailscale-admin" = [])),
	request_body = MachineIdArgs,
	responses(
		(status = 200, body = EnrollmentTicket),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn mint_enrollment(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineIdArgs>,
) -> Result<Json<EnrollmentTicket>> {
	use algae_cli::{
		passphrases::{Passphrase, SecretString},
		streams::encrypt_stream,
	};

	let mut conn = state.db.get().await?;

	let machine = Machine::get_by_id(&mut conn, args.machine_id).await?;
	if machine.deleted_at.is_some() {
		return Err(AppError::Conflict("machine is archived".into()));
	}

	let api_url = std::env::var("PUBLIC_URL")
		.map_err(|_| AppError::custom("PUBLIC_URL is not configured"))?;

	let (token, plaintext) =
		MachineEnrollmentToken::mint(&mut conn, args.machine_id, ENROLLMENT_TTL).await?;

	let payload = serde_json::json!({
		"v": "enroll-1",
		"api_url": api_url,
		"server_id": args.machine_id,
		"token": plaintext,
	});
	let payload_bytes = serde_json::to_vec(&payload).map_err(AppError::custom)?;

	// Encrypt the payload with a fresh 4-word passphrase (age/scrypt), the same
	// primitives bestool's `protect`/`reveal` use. The ciphertext is base64'd
	// for transport; the passphrase travels out-of-band.
	let passphrase = crate::fns::generate_passphrase();
	let key = Passphrase::new(SecretString::from(passphrase.clone()));

	let mut encrypted = Vec::new();
	encrypt_stream(
		&payload_bytes[..],
		futures::io::Cursor::new(&mut encrypted),
		Box::new(key),
	)
	.await
	.map_err(|e| AppError::custom(format!("encrypting enrollment ticket: {e}")))?;

	let ticket = base64::engine::general_purpose::STANDARD.encode(&encrypted);

	Ok(Json(EnrollmentTicket {
		ticket,
		passphrase,
		expires_at: token.expires_at,
	}))
}

/// Revoke any outstanding enrollment ticket for a machine.
///
/// Use this when a ticket was issued by mistake or is no longer needed.
/// Afterwards, the enrollment status endpoint reports no outstanding token,
/// and the revoked ticket can no longer be used to enroll.
#[utoipa::path(
	post,
	path = "/revoke_enrollment",
	tag = "machines",
	security(("tailscale-admin" = [])),
	request_body = MachineIdArgs,
	responses(
		(status = 200),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn revoke_enrollment(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	MachineEnrollmentToken::revoke(&mut conn, args.machine_id).await?;
	Ok(Json(()))
}

/// A machine's enrollment state: whether a device has registered, and
/// whether an enrollment token is currently outstanding.
#[derive(Serialize, ToSchema)]
pub struct EnrollmentStatus {
	/// When enrollment completed. Omitted while still awaiting the first
	/// check-in.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub registered_at: Option<Timestamp>,
	/// Expiry of the currently-active enrollment token, if one is outstanding.
	/// Never reveals the token itself.
	#[serde(skip_serializing_if = "Option::is_none")]
	pub token_expires_at: Option<Timestamp>,
	/// When the currently-active enrollment token was issued, if one is
	/// outstanding — e.g. to show "a ticket was issued on <date>".
	#[serde(skip_serializing_if = "Option::is_none")]
	pub token_issued_at: Option<Timestamp>,
}

/// Get the enrollment state of a machine.
///
/// Reports whether the machine has completed enrollment, and whether an
/// enrollment token is currently outstanding (issue and expiry times only —
/// the token itself is never revealed).
#[utoipa::path(
	post,
	path = "/enrollment_status",
	tag = "machines",
	security(("tailscale-admin" = [])),
	request_body = MachineIdArgs,
	responses(
		(status = 200, body = EnrollmentStatus),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn enrollment_status(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineIdArgs>,
) -> Result<Json<EnrollmentStatus>> {
	let mut conn = state.db.get().await?;
	let machine = Machine::get_by_id(&mut conn, args.machine_id).await?;
	let active = MachineEnrollmentToken::active_for(&mut conn, args.machine_id).await?;
	Ok(Json(EnrollmentStatus {
		registered_at: machine.registered_at,
		token_expires_at: active.as_ref().map(|t| t.expires_at),
		token_issued_at: active.as_ref().map(|t| t.created_at),
	}))
}
