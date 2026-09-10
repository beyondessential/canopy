//! An environment's inventory: the machines a configuration run acts on, the
//! applications each carries, the address each is reached at, and the
//! variables that configure them.
//!
//! Assembled from what canopy already holds (group membership, rank, the
//! applications on each machine, and the tailnet name of the device bound to
//! it) plus the variables set against the group, the environment, and the
//! machine (see [`super::inventory_variables`]).
//!
//! An environment holds at most one run lease and the inventory is served to
//! its holder alone, so two runs never act on one environment at once. It
//! carries secret values, so it is served to an administrator.
// spec: INV

use std::collections::{BTreeMap, BTreeSet};

use algae_cli::passphrases::{ExposeSecret, SecretString};
use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::{
	Uuid,
	server::{app_type::ApplicationType, rank::ServerRank},
};
use database::{
	Device,
	applications::Application,
	inventory_leases::{InventoryLease, RunIntent},
	inventory_variables::{InventoryVariable, VariableScope},
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
		.routes(routes!(for_group))
		.routes(routes!(take_lease))
		.routes(routes!(extend_lease))
		.routes(routes!(release_lease))
		.routes(routes!(run_state))
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
	/// group's live applications span more than one rank.
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
}

/// Name a lease.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct LeaseArgs {
	/// Identifier of the lease.
	pub lease_id: Uuid,
}

/// Read an environment's inventory under a lease held on it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct InventoryArgs {
	/// Identifier of the lease the run holds, from `take_lease`.
	pub lease_id: Uuid,
}

/// One application on a machine, so a run knows what it is configuring there.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InventoryApplication {
	/// Identifier of the application.
	pub id: Uuid,
	/// The application's name within its group, falling back to its host and
	/// then its identifier.
	pub name: String,
	/// What the application is: the software and the role it plays together.
	pub r#type: ApplicationType,
}

/// One machine in an environment.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InventoryHost {
	/// Identifier of the machine.
	pub id: Uuid,
	/// The machine's name, falling back to its identifier.
	pub name: String,
	/// The address to reach it at: an `ansible_host` variable, the tailnet name
	/// of the device bound to it, or the recorded host of an application on it.
	/// Null where canopy holds none of those.
	pub address: Option<String>,
	/// The applications the machine carries in this environment, by name.
	pub applications: Vec<InventoryApplication>,
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
	pub lease: Option<InventoryLease>,
	/// The window holding over the environment or over a machine in it: the
	/// one refusing a take where one does, and otherwise the caller's own.
	pub window: Option<MaintenanceWindow>,
	/// Whether that window is someone else's, which is what refuses a take.
	pub refuses: bool,
}

/// An environment's inventory.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InventoryView {
	/// Identifier of the server group the inventory covers.
	pub group_id: Uuid,
	/// Name of the server group.
	pub group: String,
	/// Rank of the environment served.
	pub rank: ServerRank,
	/// The group's and the environment's variables, merged. Every machine
	/// below carries these too, under its own overrides.
	pub vars: VarMap,
	/// Which of `vars` are secret.
	pub secret_vars: Vec<String>,
	/// The environment's machines, ordered by name.
	pub hosts: Vec<InventoryHost>,
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
	machines: Vec<Machine>,
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

	let members = Application::list_live_in_group(conn, group.id).await?;
	if members.is_empty() {
		return Err(AppError::Conflict(format!(
			"server group {:?} has no live members",
			group.name
		)));
	}

	// An application carrying no rank is at ServerRank's own default, so every
	// live application belongs to exactly one of its group's environments.
	let ranks: BTreeSet<ServerRank> = members
		.iter()
		.map(|application| application.rank.unwrap_or_default())
		.collect();
	let rank = match args.rank {
		Some(rank) => rank,
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
		None => ranks.into_iter().next().unwrap_or_default(),
	};

	let applications: Vec<Application> = members
		.into_iter()
		.filter(|application| application.rank.unwrap_or_default() == rank)
		.collect();
	if applications.is_empty() {
		return Err(AppError::Conflict(format!(
			"server group {:?} has no live application at rank {rank}",
			group.name
		)));
	}

	let machine_ids: Vec<Uuid> = applications
		.iter()
		.map(|application| application.machine_id)
		.collect::<BTreeSet<_>>()
		.into_iter()
		.collect();
	let machines = Machine::get_many(conn, &machine_ids).await?;

	Ok(Environment {
		group,
		rank,
		applications,
		machines,
	})
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
	operation_id = "inventory_take_lease",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = TakeLeaseArgs,
	responses(
		(status = 200, body = InventoryLease),
		(status = 400, description = "Neither or both of the group arguments", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "Archived, empty, ambiguously named, spanning environments, held by someone else, under someone else's maintenance, or an unplanned upgrade of production", body = ProblemDetailsSchema),
	),
)]
pub async fn take_lease(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<TakeLeaseArgs>,
) -> Result<Json<InventoryLease>> {
	let mut conn = state.db.get().await?;
	let environment = resolve_environment(&mut conn, &args.environment).await?;
	let group = &environment.group;
	let rank = environment.rank;
	let login = admin.0.login.as_str();
	let now = Timestamp::now();

	if let Some(held) = InventoryLease::open_for(&mut conn, group.id, rank).await?
		&& held.holds_for_another(Some(login), now)
		&& !args.take_over
	{
		return Err(AppError::Conflict(held.held_by_another()));
	}

	let machine_ids: Vec<Uuid> = environment
		.machines
		.iter()
		.map(|machine| machine.id)
		.collect();
	let windows = MaintenanceWindow::open_over(&mut conn, group.id, &machine_ids).await?;
	if let Some(window) = refusing_window(&windows, login, now) {
		return Err(AppError::Conflict(under_maintenance(
			group,
			&environment.machines,
			window,
		)));
	}

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

	let taken = InventoryLease::take(
		&mut conn,
		group.id,
		rank,
		args.intent,
		Some(login),
		args.note.as_deref(),
		args.take_over,
	)
	.await?;

	tracing::info!(
		login = %admin.0.login,
		group = %group.name,
		%rank,
		intent = %args.intent,
		take_over = args.take_over,
		"inventory lease taken"
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
	operation_id = "inventory_extend_lease",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = LeaseArgs,
	responses(
		(status = 200, body = InventoryLease),
		(status = 404, description = "No such lease", body = ProblemDetailsSchema),
		(status = 409, description = "Held by someone else, or no longer held", body = ProblemDetailsSchema),
	),
)]
pub async fn extend_lease(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<LeaseArgs>,
) -> Result<Json<InventoryLease>> {
	let mut conn = state.db.get().await?;
	let lease = InventoryLease::get(&mut conn, args.lease_id).await?;
	held_by_caller(&lease, &admin.0.login)?;
	if !lease.holds_at(Timestamp::now()) {
		return Err(AppError::Conflict(no_longer_held(&lease, &admin.0.login)));
	}
	let extended = InventoryLease::extend(&mut conn, lease.id).await?;
	tracing::info!(login = %admin.0.login, lease = %lease.id, "inventory lease extended");
	Ok(Json(extended))
}

/// Give the environment back when the run ends.
///
/// Releasing is audited with who did it, since it can be another operator
/// taking work over rather than the holder finishing.
#[utoipa::path(
	post,
	path = "/release_lease",
	operation_id = "inventory_release_lease",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = LeaseArgs,
	responses(
		(status = 200, description = "Released", body = ()),
		(status = 404, description = "No such lease, or it was already released", body = ProblemDetailsSchema),
	),
)]
pub async fn release_lease(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<LeaseArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	let lease = InventoryLease::get(&mut conn, args.lease_id).await?;
	if !InventoryLease::release(&mut conn, lease.id, Some(&admin.0.login)).await? {
		return Err(AppError::NotFound("that lease is no longer held".into()));
	}
	tracing::info!(
		login = %admin.0.login,
		lease = %lease.id,
		held_by = lease.held_by.as_deref().unwrap_or("unknown"),
		"inventory lease released"
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
	operation_id = "inventory_run_state",
	tag = "inventory",
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

	let lease = InventoryLease::open_for(&mut conn, environment.group.id, environment.rank)
		.await?
		.filter(|lease| lease.holds_at(now));

	let machine_ids: Vec<Uuid> = environment
		.machines
		.iter()
		.map(|machine| machine.id)
		.collect();
	let windows =
		MaintenanceWindow::open_over(&mut conn, environment.group.id, &machine_ids).await?;
	let refusing = refusing_window(&windows, login, now);

	Ok(Json(RunState {
		refuses: refusing.is_some(),
		window: refusing
			.or_else(|| windows.iter().find(|window| window.holds_at(now)))
			.cloned(),
		lease,
	}))
}

/// Serve the inventory of the environment the caller holds the lease on.
///
/// Refuses a lease that is not the caller's or no longer holds, and a secret
/// variable whose value cannot be read: a run receiving a machine that looks
/// configured and is missing a value is worse than one that does not run.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "inventory_for_group",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = InventoryArgs,
	responses(
		(status = 200, body = InventoryView),
		(status = 404, description = "No such lease", body = ProblemDetailsSchema),
		(status = 409, description = "The lease is someone else's, no longer holds, the environment is gone, or two machines share an address", body = ProblemDetailsSchema),
		(status = 502, description = "A secret variable could not be read", body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<InventoryArgs>,
) -> Result<Json<InventoryView>> {
	let mut conn = state.db.get().await?;
	let lease = InventoryLease::get(&mut conn, args.lease_id).await?;
	held_by_caller(&lease, &admin.0.login)?;
	if !lease.holds_at(Timestamp::now()) {
		return Err(AppError::Conflict(no_longer_held(&lease, &admin.0.login)));
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
	let mut wide = Scoped::read(
		&state,
		group_scope,
		&InventoryVariable::list_at(&mut conn, group_scope).await?,
	)
	.await?;
	wide.overlay(
		&Scoped::read(
			&state,
			environment_scope,
			&InventoryVariable::list_at(&mut conn, environment_scope).await?,
		)
		.await?,
	);

	let mut by_machine: BTreeMap<Uuid, Vec<InventoryVariable>> = BTreeMap::new();
	for variable in InventoryVariable::list_for_machines(&mut conn, &machine_ids).await? {
		if let Some(machine_id) = variable.machine_id {
			by_machine.entry(machine_id).or_default().push(variable);
		}
	}

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
	for (machine, own) in machines.iter().zip(owned) {
		let mut effective = wide.clone();
		effective.overlay(&own);

		let on_machine: Vec<&Application> = applications
			.iter()
			.filter(|application| application.machine_id == machine.id)
			.collect();
		let address = effective
			.vars
			.0
			.get(ANSIBLE_HOST)
			.and_then(Value::as_str)
			.map(str::to_owned)
			.or_else(|| {
				machine
					.device_id
					.and_then(|device| tailnet.get(&device).cloned())
			})
			.or_else(|| {
				on_machine
					.iter()
					.find_map(|application| host_of(application))
			});

		hosts.push(InventoryHost {
			id: machine.id,
			name: machine
				.name
				.clone()
				.unwrap_or_else(|| machine.id.to_string()),
			address,
			applications: on_machine
				.into_iter()
				.map(|application| InventoryApplication {
					id: application.id,
					name: application
						.name
						.clone()
						.or_else(|| host_of(application))
						.unwrap_or_else(|| application.id.to_string()),
					r#type: application.r#type.clone(),
				})
				.collect(),
			vars: effective.vars,
			own_vars: own.vars,
			secret_vars: effective.secret.into_iter().collect(),
		});
	}
	hosts.sort_by(|a, b| a.name.cmp(&b.name));
	reject_shared_address(&hosts)?;

	tracing::info!(
		login = %admin.0.login,
		group = %group.name,
		%rank,
		intent = %lease.intent,
		lease = %lease.id,
		"inventory served"
	);

	Ok(Json(InventoryView {
		group_id: group.id,
		group: group.name,
		rank,
		vars: wide.vars,
		secret_vars: wide.secret.into_iter().collect(),
		hosts,
	}))
}

/// The window a take is refused for: one holding over the environment that
/// belongs to somebody else. The group page and the refusal read the same
/// windows the same way, so they cannot disagree.
fn refusing_window<'a>(
	windows: &'a [MaintenanceWindow],
	login: &str,
	now: Timestamp,
) -> Option<&'a MaintenanceWindow> {
	windows.iter().find(|window| {
		window.holds_at(now)
			&& window
				.declared_by
				.as_deref()
				.is_some_and(|who| who != login)
			&& window.amended_by.as_deref() != Some(login)
	})
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
fn reject_shared_address(hosts: &[InventoryHost]) -> Result<()> {
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
		variables: &[InventoryVariable],
	) -> Result<Self> {
		let secrets: BTreeMap<String, SecretString> =
			if variables.iter().any(|variable| variable.is_secret) {
				super::inventory_variables::secret_store(state)?
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
					// Exposed once, into the value this inventory exists to
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

fn host_of(application: &Application) -> Option<String> {
	application
		.host
		.as_ref()
		.and_then(|host| host.0.host_str().map(str::to_owned))
}

fn held_by_caller(lease: &InventoryLease, login: &str) -> Result<()> {
	match lease.held_by.as_deref() {
		Some(who) if who == login => Ok(()),
		_ => Err(AppError::Conflict(lease.held_by_another())),
	}
}

/// Why a lease stopped holding, which decides whether taking another is the
/// whole answer: an operator whose lease was taken over is walking into work
/// somebody else has started.
fn no_longer_held(lease: &InventoryLease, login: &str) -> String {
	match lease.released_by.as_deref() {
		Some(who) if who != login => {
			format!("that lease was taken over by {who}; talk to them before taking another")
		}
		Some(_) => {
			"that lease has been released; take one again before reading the inventory".into()
		}
		None => "that lease has expired; take one again before reading the inventory".into(),
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
		Some(machine) => format!(
			"machine {:?} in group {:?}",
			machine
				.name
				.clone()
				.unwrap_or_else(|| machine.id.to_string()),
			group.name
		),
		None => format!("server group {:?}", group.name),
	}
}
