//! Operator-facing environment inventory: a group's live applications at one
//! rank, the address each is reached at, and the variables that configure them.
//!
//! Assembled from what Canopy already holds (group membership, rank,
//! application type, the tailnet name of the device bound to each application's
//! machine, and the application/group tag merge), so configuration tooling
//! reads the fleet from here rather than from a file kept in step by hand.
//!
//! It carries the secret variables too (see [`super::inventory_secrets`]), which
//! is why it is served to an administrator alone.
// spec: INV

use std::collections::{BTreeMap, BTreeSet};

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::{
	Uuid,
	server::{RESERVED_TAG_PREFIX, TagMap, app_type::ApplicationType, rank::ServerRank},
};
use database::{
	Device,
	applications::Application,
	inventory_secret_variables::{InventorySecretVariable, SecretScope},
	machines::Machine,
	maintenance_windows::MaintenanceWindow,
	server_groups::ServerGroup,
	upgrade_plans::UpgradePlan,
};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use crate::state::AppState;

/// How long after someone else sets a secret variable a run on the
/// environment is held off. The same for every environment.
const SETTINGS_RECENCY: SignedDuration = SignedDuration::from_mins(10);

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(for_group))
}

/// Which environment to serve the inventory for: exactly one of the group's
/// identifier or its name, and the rank where the group holds more than one
/// environment.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct InventoryArgs {
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
	/// What the run is doing to the environment. Configuring where not named.
	#[serde(default)]
	pub intent: RunIntent,
}

/// What a run intends to do to the environment it reads. An upgrade of a
/// production environment needs the group's open upgrade plan behind it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize, ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum RunIntent {
	#[default]
	Configure,
	Upgrade,
}

/// One application in an environment.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InventoryHost {
	/// Identifier of the application.
	pub id: Uuid,
	/// The application's name within its group, falling back to its host and
	/// then its identifier, so a member always has something to be addressed
	/// as.
	pub name: String,
	/// What the application is: the software and the role it plays together.
	pub r#type: ApplicationType,
	/// Identifier of the machine the application runs on.
	pub machine_id: Uuid,
	/// The address to reach the application at: the tailnet name of the device
	/// bound to its machine, or its own recorded host where no device is bound.
	/// Null when Canopy holds neither, in which case a variable has to supply
	/// it.
	pub address: Option<String>,
	/// The application's effective variables: its own tags over its group's,
	/// with the reserved read-only tags left out. This is what a run acts on.
	pub vars: VarMap,
	/// The variables the application sets itself, so a value inherited from the
	/// group can be told from one set here even where the two agree.
	pub own_vars: VarMap,
	/// Which of `vars` are secret, so a caller can keep them out of anything it
	/// writes down.
	pub secret_vars: Vec<String>,
}

/// An environment's inventory: its applications and the variables that
/// configure them.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct InventoryView {
	/// Identifier of the server group the inventory covers.
	pub group_id: Uuid,
	/// Name of the server group.
	pub group: String,
	/// Rank of the environment served.
	pub rank: ServerRank,
	/// Variables belonging to the environment rather than to any one
	/// application. Every application carries these too, under its own
	/// overrides.
	pub vars: VarMap,
	/// Which of `vars` are secret.
	pub secret_vars: Vec<String>,
	/// The environment's applications, ordered by name.
	pub hosts: Vec<InventoryHost>,
}

/// Variables as a JSON object, whose values are whatever the stored tags
/// decoded to.
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

/// A stored tag value as a variable.
///
/// `true` and `false` become booleans and a JSON array or object becomes that
/// array or object; everything else stays the text it was stored as, a bare
/// number included, since a number here is far more often a version or an
/// identifier than a quantity.
fn decode(value: &str) -> Value {
	match value {
		"true" => Value::Bool(true),
		"false" => Value::Bool(false),
		_ if value.starts_with('[') || value.starts_with('{') => {
			serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()))
		}
		_ => Value::String(value.to_owned()),
	}
}

fn vars(tags: &TagMap) -> VarMap {
	VarMap(
		tags.0
			.iter()
			.filter(|(key, _)| !key.starts_with(RESERVED_TAG_PREFIX))
			.map(|(key, value)| (key.clone(), decode(value)))
			.collect(),
	)
}

async fn resolve_group(
	conn: &mut database::diesel_async::AsyncPgConnection,
	args: InventoryArgs,
) -> Result<ServerGroup> {
	match (args.server_group_id, args.group) {
		(Some(id), None) => ServerGroup::get_by_id(conn, id).await,
		(None, Some(name)) => {
			let (live, archived): (Vec<_>, Vec<_>) = ServerGroup::find_by_name(conn, &name)
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

/// Serve one environment's inventory.
///
/// Refuses a group Canopy does not have, one that has been archived, one
/// holding several environments with no rank named, a rank with no live
/// application to configure, an environment someone else has work under way on
/// (a maintenance window they declared, or a secret variable they set moments
/// ago), an upgrade of production with no plan recorded, and a secret variable
/// whose value cannot be read, saying which it was: a refusal is a decision to
/// respect, and a caller has to be able to tell it from Canopy being
/// unreachable.
///
/// Requires admin access, the inventory carrying the secret variables' values.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "inventory_for_group",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = InventoryArgs,
	responses(
		(status = 200, body = InventoryView),
		(status = 400, description = "Neither or both of the group arguments", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "Archived, empty, ambiguously named, spanning environments, under someone else's work, or an unplanned upgrade of production", body = ProblemDetailsSchema),
		(status = 502, description = "A secret variable could not be read", body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<InventoryArgs>,
) -> Result<Json<InventoryView>> {
	let mut conn = state.db.get().await?;
	let args_rank = args.rank;
	let intent = args.intent;
	let group = resolve_group(&mut conn, args).await?;

	if group.deleted_at.is_some() {
		return Err(AppError::Conflict(format!(
			"server group {:?} is archived",
			group.name
		)));
	}

	let members = Application::list_live_in_group(&mut conn, group.id).await?;
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
	let rank = match args_rank {
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

	if intent == RunIntent::Upgrade
		&& rank == ServerRank::Production
		&& UpgradePlan::open_for_group(&mut conn, group.id)
			.await?
			.is_none()
	{
		return Err(AppError::Conflict(format!(
			"server group {:?} has no upgrade plan; record one before upgrading production",
			group.name
		)));
	}

	// The identity speaks for the box, so an application's address comes from
	// the device bound to the machine it runs on.
	let machine_ids: Vec<Uuid> = applications
		.iter()
		.map(|application| application.machine_id)
		.collect();
	let machines = Machine::get_many(&mut conn, &machine_ids).await?;
	let devices_by_machine: BTreeMap<Uuid, Uuid> = machines
		.iter()
		.filter_map(|machine| machine.device_id.map(|device| (machine.id, device)))
		.collect();
	let device_ids: Vec<Uuid> = devices_by_machine.values().copied().collect();
	let tailnet = Device::tailscale_names_by_ids(&mut conn, &device_ids).await?;

	let application_ids: Vec<Uuid> = applications
		.iter()
		.map(|application| application.id)
		.collect();
	let now = Timestamp::now();
	let login = admin.0.login.as_str();

	let windows = MaintenanceWindow::open_over(&mut conn, group.id, &machine_ids).await?;
	if let Some(window) = windows.iter().find(|window| {
		window.holds_at(now)
			&& window
				.declared_by
				.as_deref()
				.is_some_and(|who| who != login)
	}) {
		return Err(AppError::Conflict(under_maintenance(
			&group, &machines, window,
		)));
	}

	let environment_declared =
		InventorySecretVariable::list_for_environment(&mut conn, group.id, rank).await?;
	let application_declared =
		InventorySecretVariable::list_for_applications(&mut conn, &application_ids).await?;
	if let Some(variable) = environment_declared
		.iter()
		.chain(application_declared.iter())
		.find(|variable| {
			variable.set_by.as_deref().is_some_and(|who| who != login)
				&& variable.updated_at > now - SETTINGS_RECENCY
		}) {
		return Err(AppError::Conflict(recently_changed(
			&group,
			&applications,
			variable,
		)));
	}

	let environment_secrets = read_secrets(
		&state,
		SecretScope::Environment {
			group_id: group.id,
			rank,
		},
		&environment_declared,
	)
	.await?;
	let mut application_secrets: BTreeMap<Uuid, VarMap> = BTreeMap::new();
	for (application_id, declared) in by_application(application_declared) {
		let read = read_secrets(
			&state,
			SecretScope::Application { application_id },
			&declared,
		)
		.await?;
		application_secrets.insert(application_id, read);
	}

	tracing::info!(
		login = %admin.0.login,
		group = %group.name,
		%rank,
		?intent,
		secrets = environment_secrets.0.len()
			+ application_secrets.values().map(|vars| vars.0.len()).sum::<usize>(),
		"inventory served"
	);

	let mut environment_vars = vars(&group.tags);
	let environment_secret_names: Vec<String> = environment_secrets.0.keys().cloned().collect();
	environment_vars.0.extend(environment_secrets.0);

	let mut hosts: Vec<InventoryHost> = applications
		.into_iter()
		.map(|application| {
			let host = application
				.host
				.as_ref()
				.and_then(|host| host.0.host_str().map(str::to_owned));
			let address = devices_by_machine
				.get(&application.machine_id)
				.and_then(|device| tailnet.get(device).cloned())
				.or_else(|| host.clone());
			let own_secrets = application_secrets
				.remove(&application.id)
				.unwrap_or_default();
			let mut secret_vars = environment_secret_names.clone();
			secret_vars.extend(own_secrets.0.keys().cloned());
			secret_vars.sort();
			secret_vars.dedup();

			let mut own_vars = vars(&application.tags);
			own_vars.0.extend(own_secrets.0);
			let mut effective = environment_vars.0.clone();
			effective.extend(own_vars.0.iter().map(|(k, v)| (k.clone(), v.clone())));
			InventoryHost {
				name: application
					.name
					.clone()
					.or(host)
					.unwrap_or_else(|| application.id.to_string()),
				vars: VarMap(effective),
				own_vars,
				secret_vars,
				id: application.id,
				r#type: application.r#type,
				machine_id: application.machine_id,
				address,
			}
		})
		.collect();
	hosts.sort_by(|a, b| a.name.cmp(&b.name));

	Ok(Json(InventoryView {
		group_id: group.id,
		group: group.name,
		rank,
		vars: environment_vars,
		secret_vars: environment_secret_names,
		hosts,
	}))
}

fn under_maintenance(
	group: &ServerGroup,
	machines: &[Machine],
	window: &MaintenanceWindow,
) -> String {
	let who = window.declared_by.as_deref().unwrap_or("an operator");
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

fn recently_changed(
	group: &ServerGroup,
	applications: &[Application],
	variable: &InventorySecretVariable,
) -> String {
	let who = variable.set_by.as_deref().unwrap_or("an operator");
	format!(
		"secret variable {:?} on {} was set by {who} at {}; their change may still be under way",
		variable.name,
		application_target(group, applications, variable.application_id),
		variable.updated_at.strftime("%Y-%m-%d %H:%M UTC"),
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

fn application_target(
	group: &ServerGroup,
	applications: &[Application],
	application_id: Option<Uuid>,
) -> String {
	match application_id.and_then(|id| applications.iter().find(|it| it.id == id)) {
		Some(application) => format!(
			"application {:?} in group {:?}",
			application
				.name
				.clone()
				.or_else(|| {
					application
						.host
						.as_ref()
						.and_then(|host| host.0.host_str().map(str::to_owned))
				})
				.unwrap_or_else(|| application.id.to_string()),
			group.name
		),
		None => format!("server group {:?}", group.name),
	}
}

fn by_application(
	declared: Vec<InventorySecretVariable>,
) -> BTreeMap<Uuid, Vec<InventorySecretVariable>> {
	let mut out: BTreeMap<Uuid, Vec<InventorySecretVariable>> = BTreeMap::new();
	for var in declared {
		if let Some(application_id) = var.application_id {
			out.entry(application_id).or_default().push(var);
		}
	}
	out
}

/// The values behind a scope's declared names.
///
/// A name Canopy holds but cannot produce a value for refuses the whole
/// inventory: a run receiving a member that looks configured and is missing a
/// value is worse than one that does not run.
async fn read_secrets(
	state: &AppState,
	scope: SecretScope,
	declared: &[InventorySecretVariable],
) -> Result<VarMap> {
	if declared.is_empty() {
		return Ok(VarMap::default());
	}

	let kube = super::inventory_secrets::secret_store(state)?;
	let held = kube
		.try_read_keys(&scope.secret_name())
		.await?
		.unwrap_or_default();

	let mut out = BTreeMap::new();
	for var in declared {
		let value = held.get(&var.name).ok_or_else(|| {
			AppError::Upstream(format!(
				"secret variable {:?} has no value in the secret store",
				var.name
			))
		})?;
		out.insert(var.name.clone(), decode(value));
	}
	Ok(VarMap(out))
}
