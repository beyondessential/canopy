//! What Ansible is served for an environment: the machines a configuration run acts on, the
//! applications each carries, the address each is reached at, and the
//! variables that configure them.
//!
//! Assembled from what canopy already holds (group membership, rank, the
//! applications on each machine, and the tailnet name of the device bound to
//! it) plus the variables set against the group, the environment, and the
//! machine (see [`super::ansible_variables`]).
//!
//! An environment holds at most one run lease and the environment is served to
//! its holder alone, so two runs never act on one environment at once. It
//! carries secret values, so it is served to an administrator.
// spec: INV

mod hosts;
mod import;
mod inventory_file;
mod limit;
mod runs;

use std::collections::{BTreeMap, BTreeSet};

use algae_cli::passphrases::{ExposeSecret, SecretString};
use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use commons_types::{
	Uuid,
	server::{app_type::ApplicationType, rank::ServerRank},
};
use database::{
	Device,
	ansible_leases::{AnsibleLease, ReleaseReason, RunIntent, RunSpec},
	ansible_variables::{AnsibleVariable, VariableScope},
	applications::Application,
	audit::{Action, LeaseAssignment, Record, RecordKind},
	machines::Machine,
	maintenance_windows::MaintenanceWindow,
	server_groups::ServerGroup,
	upgrade_plans::UpgradePlan,
};
use futures::{StreamExt, TryStreamExt};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::state::AppState;

/// How many of an environment's machine Secrets are read at once. Every
/// machine carrying a secret variable is its own read, so an unbounded fan-out
/// would put a large environment's whole apiserver burst on one run's startup.
const SECRET_READS_AT_ONCE: usize = 8;

/// The variable naming the address a run connects to, which overrides the one
/// canopy holds for the machine.
pub(super) const ANSIBLE_HOST: &str = "ansible_host";

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(
			routes!(read_only, audit(AnsibleRead, AnsibleRunStarted, MaintenanceDeclared): for_group),
		)
		.routes(routes!(write, audit(AnsibleLeaseTaken, AnsibleLeaseTakenOver): take_lease))
		.routes(routes!(write, audit(AnsibleLeaseExtended): extend_lease))
		.routes(routes!(write, audit(AnsibleLeaseReleased): release_lease))
		.routes(routes!(read_only, silent: run_state))
		.routes(routes!(read_only, silent: runs::run_options))
		.routes(routes!(read_only, silent: runs::limit_preview))
		.routes(routes!(read_only, silent: import::import_preview))
		.routes(routes!(write, audit(AnsibleInventoryImported): import::import_apply))
}

/// Which environment to act on: exactly one of the group's identifier or its
/// name, and the rank where the group holds more than one environment.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct EnvironmentArgs {
	/// Identifier of the server group.
	#[serde(default)]
	pub server_group_id: Option<Uuid>,
	/// Name of the server group, matched exactly.
	#[serde(default)]
	pub group: Option<String>,
	/// Rank of the environment within the group. Required only where the
	/// group's ranked applications span more than one rank.
	#[serde(default)]
	pub rank: Option<ServerRank>,
}

/// Take the lease on an environment, which a run holds while it runs.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct TakeLeaseArgs {
	#[serde(flatten)]
	pub environment: EnvironmentArgs,
	/// What the run intends. Configuring where not named.
	#[serde(default)]
	pub intent: RunIntent,
	/// What the holder is doing, shown to whoever is refused meanwhile.
	#[serde(default)]
	pub note: Option<String>,
	/// Take over a lease another operator holds, which is audited.
	#[serde(default)]
	pub take_over: bool,
	/// The playbook the run is started with, from the catalogue.
	#[serde(default)]
	pub playbook: Option<String>,
	/// What the run is limited to, as an Ansible host pattern. The whole
	/// environment where not given.
	#[serde(default)]
	pub limit: Option<String>,
	/// The tags the run runs.
	#[serde(default)]
	pub tags: Vec<String>,
	/// The tags the run skips.
	#[serde(default)]
	pub skip_tags: Vec<String>,
	/// The application type each machine with nothing on it is assigned for
	/// this run.
	#[serde(default)]
	pub assignments: Vec<runs::AssignmentArg>,
}

/// Name a lease.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct LeaseArgs {
	/// Identifier of the lease.
	pub lease_id: Uuid,
}

/// Give a lease back.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ReleaseLeaseArgs {
	/// Identifier of the lease.
	pub lease_id: Uuid,
	/// Release only while no run has read the lease, as giving up on a run that
	/// never started does, and refuse once one has.
	#[serde(default)]
	pub unless_started: bool,
}

/// Read what an environment serves Ansible, under a lease held on it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AnsibleArgs {
	/// Identifier of the lease the run holds, from `take_lease`.
	pub lease_id: Uuid,
	/// The intent the run was started with, where it was given one. A read
	/// naming an intent other than the lease's is refused.
	#[serde(default)]
	pub intent: Option<RunIntent>,
}

/// One application on a machine, so a run knows what it is configuring there.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnsibleApplication {
	/// Identifier of the application.
	pub id: Uuid,
	/// The application's name, or its type's where nobody has named it.
	// spec: FLT#naming
	pub name: String,
	/// What the application is: the software and the role it plays together.
	pub r#type: ApplicationType,
}

/// One machine in an environment.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnsibleHost {
	/// Identifier of the machine.
	pub id: Uuid,
	/// The machine's name, falling back to its identifier.
	pub name: String,
	/// What a run's limit and output name the machine by: the first label of
	/// its tailnet name, or its address, or its Canopy name where that would
	/// leave two machines the same.
	pub host_name: String,
	/// The address to reach it at: an `ansible_host` variable, the tailnet name
	/// of the device bound to it, or the recorded host of an application on it.
	/// Null where canopy holds none of those.
	pub address: Option<String>,
	/// The applications the machine carries in this environment, by name.
	pub applications: Vec<AnsibleApplication>,
	/// The application type this run assigns the machine where nothing is on it
	/// yet, which a run groups it by as it would by an application.
	pub assigned_type: Option<String>,
	/// The effective variables: the machine's over the environment's over the
	/// group's. This is what a run acts on.
	pub vars: VarMap,
	/// The variables the machine sets itself, so a value inherited from a wider
	/// scope can be told from one set here even where the two agree.
	pub own_vars: VarMap,
	/// Which of `vars` are secret.
	pub secret_vars: Vec<String>,
}

/// The lease and the maintenance window a run on an environment would meet.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RunState {
	/// The lease holding the environment, where one holds.
	pub lease: Option<AnsibleLease>,
	/// The window holding over the environment or over a machine in it: the
	/// one refusing a take where one does, and otherwise the caller's own.
	pub window: Option<MaintenanceWindow>,
	/// Whether that window is someone else's, which is what refuses a take.
	pub refuses: bool,
}

/// What an environment serves Ansible.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnsibleView {
	/// Identifier of the server group the environment belongs to.
	pub group_id: Uuid,
	/// Name of the server group.
	pub group: String,
	/// Rank of the environment served.
	pub rank: ServerRank,
	/// The lease it is served under, whose intent, playbook, limit and tags the
	/// run takes rather than how it was started.
	// spec: INV#what-a-run-is-served
	pub lease: AnsibleLease,
	/// The group's and the environment's variables, merged. Every machine
	/// below carries these too, under its own overrides.
	pub vars: VarMap,
	/// Which of `vars` are secret.
	pub secret_vars: Vec<String>,
	/// The environment's machines, ordered by name.
	pub hosts: Vec<AnsibleHost>,
}

/// Variables as a JSON object.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(transparent)]
pub struct VarMap(pub BTreeMap<String, Value>);

impl utoipa::PartialSchema for VarMap {
	fn schema() -> utoipa::openapi::RefOr<utoipa::openapi::schema::Schema> {
		use utoipa::openapi::schema::{AdditionalProperties, Object, SchemaType, Type};

		let mut object = Object::with_type(SchemaType::Type(Type::Object));
		object.description = Some("Variables as a JSON object.".to_string());
		object.additional_properties = Some(Box::new(AdditionalProperties::FreeForm(true)));
		utoipa::openapi::RefOr::T(utoipa::openapi::schema::Schema::Object(object))
	}
}

impl utoipa::ToSchema for VarMap {}

/// The environment a request names, and the machines and applications in it.
struct Environment {
	group: ServerGroup,
	rank: ServerRank,
	applications: Vec<Application>,
	/// The machines a run is served: those at the rank that a device is bound
	/// to or something on has reported.
	machines: Vec<Machine>,
	/// Every live machine at the rank, whether or not a run can be served it.
	at_rank: Vec<Machine>,
}

impl Environment {
	/// The machines whose maintenance windows hold over the environment: every
	/// one at the rank, as a run's own declaration covers them, so what refuses
	/// a take, what the page shows holding, and what the run's first read meets
	/// are the same windows.
	// spec: ANSRUN#work-under-way
	fn window_machine_ids(&self) -> Vec<Uuid> {
		self.at_rank.iter().map(|machine| machine.id).collect()
	}
}

async fn resolve_group(
	conn: &mut database::diesel_async::AsyncPgConnection,
	args: &EnvironmentArgs,
) -> Result<ServerGroup> {
	match (args.server_group_id, args.group.as_deref()) {
		(Some(id), None) => ServerGroup::get_by_id(conn, id).await,
		(None, Some(name)) => {
			let (live, archived): (Vec<_>, Vec<_>) = ServerGroup::find_by_name(conn, name)
				.await?
				.into_iter()
				.partition(|group| group.deleted_at.is_none());
			if live.len() > 1 {
				return Err(AppError::Conflict(format!(
					"{name:?} names {} server groups; ask by identifier",
					live.len()
				)));
			}
			live.into_iter()
				.next()
				.or_else(|| archived.into_iter().next())
				.ok_or_else(|| AppError::NotFound(format!("no server group named {name:?}")))
		}
		_ => Err(AppError::BadRequest(
			"give exactly one of server_group_id or group".into(),
		)),
	}
}

async fn resolve_environment(
	conn: &mut database::diesel_async::AsyncPgConnection,
	args: &EnvironmentArgs,
) -> Result<Environment> {
	let group = resolve_group(conn, args).await?;
	if group.deleted_at.is_some() {
		return Err(AppError::Conflict(format!(
			"server group {:?} is archived",
			group.name
		)));
	}

	let live = Machine::list_for_group(conn, group.id).await?;
	// A machine is in the environment its rank names, whether or not anything
	// on it has reported yet, and a pending one is in none.
	// spec: GRP#environments
	let ranks: BTreeSet<ServerRank> = live.iter().filter_map(Machine::environment_rank).collect();
	let rank = match args.rank {
		Some(rank) if ranks.contains(&rank) => rank,
		Some(rank) => {
			return Err(AppError::Conflict(format!(
				"server group {:?} has no machine at rank {rank}",
				group.name
			)));
		}
		None if ranks.len() > 1 => {
			return Err(AppError::Conflict(format!(
				"server group {:?} holds {} environments ({}); name the rank",
				group.name,
				ranks.len(),
				ranks
					.iter()
					.map(ServerRank::to_string)
					.collect::<Vec<_>>()
					.join(", ")
			)));
		}
		None => ranks.into_iter().next().ok_or_else(|| {
			AppError::Conflict(format!(
				"server group {:?} has nothing ranked yet, so it has no environment",
				group.name
			))
		})?,
	};

	let at_rank: Vec<Machine> = live
		.into_iter()
		.filter(|machine| machine.environment_rank() == Some(rank))
		.collect();
	let at_rank_ids: BTreeSet<Uuid> = at_rank.iter().map(|machine| machine.id).collect();
	let applications: Vec<Application> = Application::list_live_in_group(conn, group.id)
		.await?
		.into_iter()
		.filter(|application| {
			application
				.machine_id
				.is_some_and(|machine| at_rank_ids.contains(&machine))
		})
		.collect();

	// A run reaches a machine only once a device is bound to it or something on
	// it has reported, so the rest are left out rather than refusing the
	// environment: a run is how a bound machine's applications get there.
	// spec: INV#what-a-run-is-served
	let machines: Vec<Machine> = at_rank
		.iter()
		.filter(|machine| {
			machine.device_id.is_some()
				|| applications.iter().any(|application| {
					application.machine_id == Some(machine.id)
						&& application.registered_at.is_some()
				})
		})
		.cloned()
		.collect();

	Ok(Environment {
		group,
		rank,
		applications,
		machines,
		at_rank,
	})
}

/// The group and lease an entry about a lease names. The lease is named by the
/// environment it holds.
fn lease_records(group: &ServerGroup, lease: &AnsibleLease) -> (Record, Record) {
	(
		Record::new(RecordKind::Group, group.id, group.name.clone()),
		Record::new(
			RecordKind::Lease,
			lease.id,
			format!("{} {}", group.name, lease.rank),
		),
	)
}

/// Take the environment's run lease.
///
/// Refuses a group canopy does not have, one that has been archived, one
/// holding several environments with no rank named, a rank with no live
/// application to configure, an environment another operator holds or has
/// declared maintenance over, and an upgrade of production with no plan
/// recorded, saying which it was so an operator knows what to do or who to
/// wait for.
#[utoipa::path(
	post,
	path = "/take_lease",
	operation_id = "ansible_take_lease",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = TakeLeaseArgs,
	responses(
		(status = 200, body = AnsibleLease),
		(status = 400, description = "Neither or both of the group arguments", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "Archived, empty, ambiguously named, spanning environments, held by someone else, under someone else's maintenance, or an unplanned upgrade of production", body = ProblemDetailsSchema),
	),
)]
pub async fn take_lease(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<TakeLeaseArgs>,
) -> Result<Json<AnsibleLease>> {
	let mut conn = audit.begin().await?;
	let environment = resolve_environment(&mut conn, &args.environment).await?;
	let group = &environment.group;
	let rank = environment.rank;
	let login = admin.0.login.as_str();
	let now = Timestamp::now();

	let held = AnsibleLease::open_for(&mut conn, group.id, rank).await?;
	let taking_over = held
		.as_ref()
		.filter(|held| held.holds_for_another(Some(login), now));
	if let Some(held) = taking_over
		&& !args.take_over
	{
		return Err(AppError::Conflict(held.held_by_another()));
	}
	let previous_holder = taking_over.map(|held| held.held_by.clone());

	if args.intent == RunIntent::Upgrade
		&& rank == ServerRank::Production
		&& UpgradePlan::open_for_environment(&mut conn, group.id, rank)
			.await?
			.is_none()
	{
		return Err(AppError::Conflict(format!(
			"server group {:?} has no upgrade plan; record one before upgrading production",
			group.name
		)));
	}

	let checked = runs::check(&mut conn, &environment, &args).await?;

	// A window over any member refuses the environment, since a run acts on the
	// environment as a whole, whatever its limit leaves out.
	let machine_ids = environment.window_machine_ids();
	// The windows over the environment are read under the group's lock, which
	// a window moving over any part of the group also takes, so a window cannot
	// move off the environment between this check and the lease it lets through.
	// spec: ANSRUN#work-under-way
	let taken = AnsibleLease::take(
		&mut conn,
		group.id,
		rank,
		&machine_ids,
		&RunSpec {
			intent: args.intent,
			note: args.note.as_deref(),
			playbook: args.playbook.as_deref(),
			limit_pattern: args.limit.as_deref(),
			limited_to: checked.limited_to.as_deref(),
			tags: &args.tags,
			skip_tags: &args.skip_tags,
			assignments: &checked.assignments,
		},
		Some(login),
		args.take_over,
	)
	.await?
	.map_err(|window| {
		AppError::Conflict(under_maintenance(group, &environment.at_rank, &window))
	})?;

	let (group_record, lease_record) = lease_records(group, &taken);
	let (intent, note, expires_at) = (
		args.intent.to_string(),
		taken.note.clone(),
		taken.expires_at,
	);
	let limited_to: Vec<Record> =
		Record::machines(&mut conn, taken.limited_to.as_deref().unwrap_or_default()).await?;
	let assignments: Vec<LeaseAssignment> = taken
		.assignments
		.iter()
		.map(|(machine, application_type)| LeaseAssignment {
			machine: checked
				.members
				.iter()
				.find(|member| member.id == *machine)
				.map(|member| member.name.clone())
				.unwrap_or_else(|| machine.to_string()),
			application_type: application_type.clone(),
		})
		.collect();
	conn.record(match previous_holder {
		Some(previous_holder) => Action::AnsibleLeaseTakenOver {
			group: group_record,
			lease: lease_record,
			limited_to,
			rank,
			intent,
			note,
			expires_at,
			previous_holder,
			playbook: taken.playbook.clone(),
			limit: taken.limit_pattern.clone(),
			tags: taken.tags.clone(),
			skip_tags: taken.skip_tags.clone(),
			assignments,
		},
		None => Action::AnsibleLeaseTaken {
			group: group_record,
			lease: lease_record,
			limited_to,
			rank,
			intent,
			note,
			expires_at,
			playbook: taken.playbook.clone(),
			limit: taken.limit_pattern.clone(),
			tags: taken.tags.clone(),
			skip_tags: taken.skip_tags.clone(),
			assignments,
		},
	})
	.await?;

	tracing::info!(
		login = %admin.0.login,
		group = %group.name,
		%rank,
		intent = %args.intent,
		take_over = args.take_over,
		"ansible lease taken"
	);
	Ok(Json(taken))
}

/// Push a held lease's expiry out, so a run still going keeps the environment.
///
/// Only the holder can extend, and only while the lease is unreleased: one that
/// has been released is gone, and taking a fresh one goes through the same
/// refusals a first one does.
#[utoipa::path(
	post,
	path = "/extend_lease",
	operation_id = "ansible_extend_lease",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = LeaseArgs,
	responses(
		(status = 200, body = AnsibleLease),
		(status = 404, description = "No such lease", body = ProblemDetailsSchema),
		(status = 409, description = "Held by someone else, or no longer held", body = ProblemDetailsSchema),
	),
)]
pub async fn extend_lease(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<LeaseArgs>,
) -> Result<Json<AnsibleLease>> {
	let mut conn = audit.begin().await?;
	let lease = AnsibleLease::get(&mut conn, args.lease_id).await?;
	held_by_caller(&lease, &admin.0.login)?;
	if !lease.holds_at(Timestamp::now()) {
		return Err(AppError::Conflict(no_longer_held(&lease, &admin.0.login)));
	}
	let extended = AnsibleLease::extend(&mut conn, lease.id).await?;
	let group = ServerGroup::get_by_id(&mut conn, extended.server_group_id).await?;
	let (group_record, lease_record) = lease_records(&group, &extended);
	conn.record(Action::AnsibleLeaseExtended {
		group: group_record,
		lease: lease_record,
		rank: extended.rank,
		expires_at: extended.expires_at,
	})
	.await?;
	tracing::info!(login = %admin.0.login, lease = %lease.id, "ansible lease extended");
	Ok(Json(extended))
}

/// Give the environment back when the run ends.
///
/// Releasing is audited with who did it, since it can be another operator
/// taking work over rather than the holder finishing.
#[utoipa::path(
	post,
	path = "/release_lease",
	operation_id = "ansible_release_lease",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = ReleaseLeaseArgs,
	responses(
		(status = 200, description = "Released", body = ()),
		(status = 404, description = "No such lease, or it was already released", body = ProblemDetailsSchema),
		(status = 409, description = "Asked to release only an unstarted lease, and its run has started", body = ProblemDetailsSchema),
	),
)]
pub async fn release_lease(
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<ReleaseLeaseArgs>,
) -> Result<Json<()>> {
	let mut conn = audit.begin().await?;
	let lease = AnsibleLease::get(&mut conn, args.lease_id).await?;
	let by = Some(admin.0.login.as_str());
	let released = if args.unless_started {
		AnsibleLease::release_unstarted(&mut conn, lease.id, by).await?
	} else {
		AnsibleLease::release(&mut conn, lease.id, by, ReleaseReason::Released).await?
	};
	if !released {
		let now = AnsibleLease::get(&mut conn, lease.id).await?;
		return Err(if now.released_at.is_none() {
			AppError::Conflict("that lease's run has started, so it is left held".into())
		} else {
			AppError::NotFound("that lease is no longer held".into())
		});
	}
	let group = ServerGroup::get_by_id(&mut conn, lease.server_group_id).await?;
	let (group_record, lease_record) = lease_records(&group, &lease);
	conn.record(Action::AnsibleLeaseReleased {
		group: group_record,
		lease: lease_record,
		rank: lease.rank,
		held_by: lease.held_by.clone(),
		reason: ReleaseReason::Released.to_string(),
	})
	.await?;
	tracing::info!(
		login = %admin.0.login,
		lease = %lease.id,
		held_by = lease.held_by.as_deref().unwrap_or("unknown"),
		"ansible lease released"
	);
	Ok(Json(()))
}

/// What a run on this environment would meet: the lease holding it and the
/// maintenance window over it, read the way taking a lease reads them so the
/// group page and the refusal agree.
///
/// Available to any operator: it names who is running and until when, and
/// carries nothing a run receives.
#[utoipa::path(
	post,
	path = "/run_state",
	operation_id = "ansible_run_state",
	tag = "ansible",
	security(("tailscale-user" = [])),
	request_body = EnvironmentArgs,
	responses(
		(status = 200, body = RunState),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "Archived, empty, or ambiguously named", body = ProblemDetailsSchema),
	),
)]
pub async fn run_state(
	State(state): State<AppState>,
	user: TailscaleUser,
	Json(args): Json<EnvironmentArgs>,
) -> Result<Json<RunState>> {
	let mut conn = state.db.get().await?;
	let environment = resolve_environment(&mut conn, &args).await?;
	let now = Timestamp::now();
	let login = user.login.as_str();

	let lease = AnsibleLease::open_for(&mut conn, environment.group.id, environment.rank)
		.await?
		.filter(|lease| lease.holds_at(now));

	let machine_ids = environment.window_machine_ids();
	let windows = MaintenanceWindow::open_over(
		&mut conn,
		environment.group.id,
		environment.rank,
		&machine_ids,
	)
	.await?;
	let refusing = MaintenanceWindow::refusing_run(&windows, login, now);

	Ok(Json(RunState {
		refuses: refusing.is_some(),
		window: refusing
			.or_else(|| windows.iter().find(|window| window.holds_at(now)))
			.cloned(),
		lease,
	}))
}

/// Serve the environment the caller holds the lease on.
///
/// Refuses a lease that is not the caller's or no longer holds, and a secret
/// variable whose value cannot be read: a run receiving a machine that looks
/// configured and is missing a value is worse than one that does not run.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "ansible_for_group",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = AnsibleArgs,
	responses(
		(status = 200, body = AnsibleView),
		(status = 404, description = "No such lease", body = ProblemDetailsSchema),
		(status = 409, description = "The lease is someone else's, no longer holds, was taken for another intent, the environment is gone, or two machines share an address", body = ProblemDetailsSchema),
		(status = 502, description = "A secret variable could not be read", body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<AnsibleArgs>,
) -> Result<Json<AnsibleView>> {
	// The environment is read, and its secrets fetched, on a connection that
	// holds no transaction, so nothing is held open across the secret store.
	let mut conn = state.db.get().await?;
	let lease = AnsibleLease::get(&mut conn, args.lease_id).await?;
	held_by_caller(&lease, &admin.0.login)?;
	if !lease.holds_at(Timestamp::now()) {
		return Err(AppError::Conflict(no_longer_held(&lease, &admin.0.login)));
	}
	// spec: ANSRUN#starting-a-run
	if let Some(claimed) = args.intent
		&& claimed != lease.intent
	{
		return Err(AppError::Conflict(format!(
			"this lease was taken to {}, not to {claimed}; take a lease to {claimed} for this run",
			lease.intent
		)));
	}
	// Recorded before the read, so a read that fails is recorded as failed: the
	// layer settles it by how the handler ends.
	let group_for_entry = ServerGroup::get_by_id(&mut conn, lease.server_group_id).await?;
	{
		let (group_record, lease_record) = lease_records(&group_for_entry, &lease);
		audit
			.read(Action::AnsibleRead {
				group: group_record,
				lease: lease_record,
				rank: lease.rank,
				intent: lease.intent.to_string(),
			})
			.await?;
	}

	let environment = resolve_environment(
		&mut conn,
		&EnvironmentArgs {
			server_group_id: Some(lease.server_group_id),
			group: None,
			rank: Some(lease.rank),
		},
	)
	.await?;
	let Environment {
		group,
		rank,
		applications,
		machines,
		..
	} = environment;

	let machine_ids: Vec<Uuid> = machines.iter().map(|machine| machine.id).collect();
	let device_ids: Vec<Uuid> = machines
		.iter()
		.filter_map(|machine| machine.device_id)
		.collect();
	let tailnet = Device::tailscale_names_by_ids(&mut conn, &device_ids).await?;

	let group_scope = VariableScope::Group { group_id: group.id };
	let environment_scope = VariableScope::Environment {
		group_id: group.id,
		rank,
	};
	let group_variables = AnsibleVariable::list_at(&mut conn, group_scope).await?;
	let environment_variables = AnsibleVariable::list_at(&mut conn, environment_scope).await?;
	let mut by_machine: BTreeMap<Uuid, Vec<AnsibleVariable>> = BTreeMap::new();
	for variable in AnsibleVariable::list_for_machines(&mut conn, &machine_ids).await? {
		if let Some(machine_id) = variable.machine_id {
			by_machine.entry(machine_id).or_default().push(variable);
		}
	}
	drop(conn);

	let mut wide = Scoped::read(&state, group_scope, &group_variables).await?;
	wide.overlay(&Scoped::read(&state, environment_scope, &environment_variables).await?);

	let reads: Vec<_> = machines
		.iter()
		.map(|machine| {
			Scoped::read(
				&state,
				VariableScope::Machine {
					machine_id: machine.id,
				},
				by_machine.get(&machine.id).map_or(&[][..], Vec::as_slice),
			)
		})
		.collect();
	let owned: Vec<Scoped> = futures::stream::iter(reads)
		.buffered(SECRET_READS_AT_ONCE)
		.try_collect()
		.await?;

	let mut hosts = Vec::with_capacity(machines.len());
	let mut tailnet_named_by_machine = Vec::with_capacity(machines.len());
	for (machine, own) in machines.iter().zip(owned) {
		let mut effective = wide.clone();
		effective.overlay(&own);

		let on_machine: Vec<&Application> = applications
			.iter()
			.filter(|application| application.machine_id == Some(machine.id))
			.collect();
		let (address, tailnet_named) = reach(
			machine,
			effective.vars.0.get(ANSIBLE_HOST).and_then(Value::as_str),
			&tailnet,
			&on_machine,
		);
		tailnet_named_by_machine.push(tailnet_named);

		let assigned_type = on_machine
			.is_empty()
			.then(|| lease.assignments.get(&machine.id).cloned())
			.flatten();
		hosts.push(AnsibleHost {
			id: machine.id,
			name: machine.name.clone(),
			host_name: String::new(),
			assigned_type,
			address,
			applications: on_machine
				.into_iter()
				.map(|application| AnsibleApplication {
					id: application.id,
					name: application.display_name(),
					r#type: application.r#type.clone(),
				})
				.collect(),
			vars: effective.vars,
			own_vars: own.vars,
			secret_vars: effective.secret.into_iter().collect(),
		});
	}
	// spec: INV#reaching-a-machine
	let names = hosts::host_names(
		&hosts
			.iter()
			.zip(&tailnet_named_by_machine)
			.map(|(host, tailnet)| hosts::Candidate {
				id: host.id,
				name: &host.name,
				address: host.address.as_deref(),
				tailnet: *tailnet,
				managed: host.vars.0.get(runs::MANAGED).is_some_and(runs::truthy),
			})
			.collect::<Vec<_>>(),
	)?;
	for (host, name) in hosts.iter_mut().zip(names) {
		host.host_name = name;
	}
	hosts.sort_by(|a, b| a.name.cmp(&b.name));
	reject_shared_address(&hosts)?;

	// The run starts in one transaction, begun once the read has served, so a
	// read that fails starts nothing and the next is the first again, and the
	// group's row is held only for the start's writes. The lease is read again
	// within it, since the run may have lost it while the environment was read.
	let mut conn = audit.begin().await?;
	let lease = AnsibleLease::get(&mut conn, lease.id).await?;
	held_by_caller(&lease, &admin.0.login)?;
	if !lease.holds_at(Timestamp::now()) {
		return Err(AppError::Conflict(no_longer_held(&lease, &admin.0.login)));
	}
	let lease = runs::start(&mut conn, lease, &group_for_entry).await?;

	tracing::info!(
		login = %admin.0.login,
		group = %group.name,
		%rank,
		intent = %lease.intent,
		lease = %lease.id,
		"ansible served"
	);

	Ok(Json(AnsibleView {
		group_id: group.id,
		group: group.name,
		rank,
		lease,
		vars: wide.vars,
		secret_vars: wide.secret.into_iter().collect(),
		hosts,
	}))
}

/// The operator a window belongs to: an amendment declares the same work, so
/// the one that stands speaks for the window over the original declaration.
fn window_operator(window: &MaintenanceWindow) -> Option<&str> {
	window
		.amended_by
		.as_deref()
		.or(window.declared_by.as_deref())
}

/// Two machines at one address would have a run configure one box twice and
/// leave the other untouched.
fn reject_shared_address(hosts: &[AnsibleHost]) -> Result<()> {
	let mut seen: BTreeMap<&str, &str> = BTreeMap::new();
	for host in hosts {
		let Some(address) = host.address.as_deref() else {
			continue;
		};
		if let Some(other) = seen.insert(address, &host.name) {
			return Err(AppError::Conflict(format!(
				"machines {other:?} and {:?} are both reached at {address:?}; \
				 an `ansible_host` variable on one of them has to say which box it is",
				host.name
			)));
		}
	}
	Ok(())
}

/// Variables gathered from one or more scopes, with the names among them whose
/// values are secret.
#[derive(Debug, Clone, Default)]
struct Scoped {
	vars: VarMap,
	secret: BTreeSet<String>,
}

impl Scoped {
	/// One scope's variables, its Secret read only where it holds one.
	async fn read(
		state: &AppState,
		scope: VariableScope,
		variables: &[AnsibleVariable],
	) -> Result<Self> {
		let secrets: BTreeMap<String, SecretString> =
			if variables.iter().any(|variable| variable.is_secret) {
				super::ansible_variables::secret_store(state)?
					.try_read_secret_keys(&scope.secret_name())
					.await?
					.unwrap_or_default()
			} else {
				BTreeMap::new()
			};

		let mut scoped = Self::default();
		for variable in variables {
			let value = match &variable.value {
				Some(value) => value.clone(),
				None => {
					let held = secrets.get(&variable.name).ok_or_else(|| {
						AppError::Upstream(format!(
							"secret variable {:?} has no value in the secret store",
							variable.name
						))
					})?;
					// Exposed once, into the value this serving exists to
					// serve to the run holding the lease.
					let held = held.expose_secret();
					serde_json::from_str(held).unwrap_or_else(|_| Value::String(held.to_owned()))
				}
			};
			scoped.vars.0.insert(variable.name.clone(), value);
			if variable.is_secret {
				scoped.secret.insert(variable.name.clone());
			}
		}
		Ok(scoped)
	}

	/// Lay a narrower scope's variables over these: its value wins, and its
	/// secrecy with it, so a name it sets in the clear stops being secret.
	fn overlay(&mut self, narrower: &Self) {
		for (name, value) in &narrower.vars.0 {
			self.vars.0.insert(name.clone(), value.clone());
			if narrower.secret.contains(name) {
				self.secret.insert(name.clone());
			} else {
				self.secret.remove(name);
			}
		}
	}
}

/// Where a run reaches a machine, and whether that is the tailnet name of the
/// device bound to it: the machine's `ansible_host` variable, else that
/// tailnet name, else the recorded host of an application on it. `None` where
/// Canopy holds none of them.
pub(super) fn reach(
	machine: &Machine,
	ansible_host: Option<&str>,
	tailnet: &std::collections::HashMap<Uuid, String>,
	applications: &[&Application],
) -> (Option<String>, bool) {
	if let Some(address) = ansible_host {
		return (Some(address.to_owned()), false);
	}
	if let Some(name) = machine
		.device_id
		.and_then(|device| tailnet.get(&device).cloned())
	{
		return (Some(name), true);
	}
	(
		applications
			.iter()
			.find_map(|application| host_of(application)),
		false,
	)
}

fn host_of(application: &Application) -> Option<String> {
	application
		.host
		.as_ref()
		.and_then(|host| host.0.host_str().map(str::to_owned))
}

fn held_by_caller(lease: &AnsibleLease, login: &str) -> Result<()> {
	match lease.held_by.as_deref() {
		Some(who) if who == login => Ok(()),
		_ => Err(AppError::Conflict(lease.held_by_another())),
	}
}

/// Why a lease stopped holding, which decides whether taking another is the
/// whole answer: an operator whose lease was taken over is walking into work
/// somebody else has started.
fn no_longer_held(lease: &AnsibleLease, login: &str) -> String {
	match lease.released_by.as_deref() {
		Some(who) if who != login => {
			format!("that lease was taken over by {who}; talk to them before taking another")
		}
		Some(_) => {
			"that lease has been released; take one again before reading the environment".into()
		}
		None => "that lease has expired; take one again before reading the environment".into(),
	}
}

fn under_maintenance(
	group: &ServerGroup,
	machines: &[Machine],
	window: &MaintenanceWindow,
) -> String {
	let who = window_operator(window).unwrap_or("an operator");
	let note = window
		.note
		.as_deref()
		.map(|note| format!("; {note}"))
		.unwrap_or_default();
	format!(
		"{} is under maintenance declared by {who} until {}{note}",
		machine_target(group, machines, window.machine_id),
		window.expected_end.strftime("%Y-%m-%d %H:%M UTC"),
	)
}

fn machine_target(group: &ServerGroup, machines: &[Machine], machine_id: Option<Uuid>) -> String {
	match machine_id.and_then(|id| machines.iter().find(|machine| machine.id == id)) {
		Some(machine) => format!("machine {:?} in group {:?}", machine.name, group.name),
		None => format!("server group {:?}", group.name),
	}
}
