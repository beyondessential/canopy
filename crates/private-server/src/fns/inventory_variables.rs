//! Setting and listing the variables that configure an environment.
//!
//! A secret's value goes to canopy's secret store and is served back only as
//! part of an inventory (see [`super::inventory`]). Nothing here returns one.
// spec: INV#inventory-variables

use algae_cli::passphrases::SecretString;
use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	backup_secrets::BackupSecrets,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use commons_types::{Uuid, server::rank::ServerRank};
use database::{
	inventory_variables::{InventoryVariable, VariableScope},
	machines::Machine,
	server_groups::ServerGroup,
};
use serde::Deserialize;
use serde_json::Value;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(for_group))
		.routes(routes!(set))
		.routes(routes!(remove))
}

/// Which scope a request addresses: a group, one of its environments, or one
/// machine.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
pub struct ScopeArgs {
	/// Identifier of the machine, for a variable one machine carries.
	pub machine_id: Option<Uuid>,
	/// Identifier of the server group, for a variable a whole group carries
	/// or one of its environments does.
	pub server_group_id: Option<Uuid>,
	/// Rank of the environment within the group, for a variable one
	/// environment carries rather than the whole group.
	pub rank: Option<ServerRank>,
}

impl TryFrom<ScopeArgs> for VariableScope {
	type Error = AppError;

	fn try_from(args: ScopeArgs) -> Result<Self> {
		match (args.machine_id, args.server_group_id, args.rank) {
			(Some(machine_id), None, None) => Ok(Self::Machine { machine_id }),
			(None, Some(group_id), Some(rank)) => Ok(Self::Environment { group_id, rank }),
			(None, Some(group_id), None) => Ok(Self::Group { group_id }),
			_ => Err(AppError::BadRequest(
				"a variable is set on one machine, on one environment (a group and a rank), \
				 or on a whole group"
					.into(),
			)),
		}
	}
}

/// Set or replace one variable.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SetArgs {
	#[serde(flatten)]
	pub scope: ScopeArgs,
	/// The variable's name.
	pub name: String,
	/// The value, as JSON.
	#[schema(value_type = serde_json::Value)]
	pub value: Value,
	/// Whether the value is a secret, held in the secret store and served only
	/// as part of an inventory.
	#[serde(default)]
	pub secret: bool,
}

/// Name one variable to forget.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RemoveArgs {
	#[serde(flatten)]
	pub scope: ScopeArgs,
	/// The variable's name.
	pub name: String,
}

/// Which group's variables to list.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct VariablesForGroupArgs {
	/// Identifier of the server group.
	pub server_group_id: Uuid,
}

/// Everything set under a group: its own, its environments', and those of the
/// machines in it.
///
/// A secret's value is not among them; it is served only as part of an
/// inventory, and only to an administrator.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "inventory_variables_for_group",
	tag = "inventory",
	security(("tailscale-user" = [])),
	request_body = VariablesForGroupArgs,
	responses(
		(status = 200, body = Vec<InventoryVariable>),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<VariablesForGroupArgs>,
) -> Result<Json<Vec<InventoryVariable>>> {
	let mut conn = state.db.get().await?;
	ServerGroup::get_by_id(&mut conn, args.server_group_id).await?;
	Ok(Json(
		InventoryVariable::list_under_group(&mut conn, args.server_group_id).await?,
	))
}

/// Set or replace a variable.
///
/// A secret's value goes to the secret store keyed by name, and the row holds
/// no value at all. Turning a secret into a plain variable forgets the stored
/// value, so a later switch back never resurrects a stale one.
#[utoipa::path(
	post,
	path = "/set",
	operation_id = "inventory_variables_set",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = SetArgs,
	responses(
		(status = 200, body = InventoryVariable),
		(status = 400, description = "Not one scope, not a usable variable name, or `ansible_host` outside machine scope", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group or machine", body = ProblemDetailsSchema),
		(status = 502, description = "The secret store is unavailable", body = ProblemDetailsSchema),
	),
)]
pub async fn set(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<SetArgs>,
) -> Result<Json<InventoryVariable>> {
	let scope = VariableScope::try_from(args.scope)?;
	check_name(&args.name)?;
	check_machine_scoped(scope, &args.name)?;

	let mut conn = state.db.get().await?;
	check_scope(&mut conn, scope).await?;

	let set = if args.secret {
		let kube = secret_store(&state)?;
		let secret = scope.secret_name();
		let mut keys = kube
			.try_read_secret_keys(&secret)
			.await?
			.unwrap_or_default();
		keys.insert(args.name.clone(), stored(&args.value));
		kube.put_secret_keys(&secret, &keys).await?;

		InventoryVariable::set(&mut conn, scope, &args.name, None, Some(&admin.0.login)).await?
	} else {
		let was_secret = InventoryVariable::at(&mut conn, scope, &args.name)
			.await?
			.is_some_and(|prior| prior.is_secret);
		let set = InventoryVariable::set(
			&mut conn,
			scope,
			&args.name,
			Some(&args.value),
			Some(&admin.0.login),
		)
		.await?;
		if was_secret {
			forget_secret_value(&state, scope, &args.name).await?;
		}
		set
	};

	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		secret = args.secret,
		"inventory variable set"
	);
	Ok(Json(set))
}

/// Forget a variable, value and all.
///
/// The row goes first, so a value the secret store will not let go of leaves
/// nothing behind that would refuse every later read of the inventory.
#[utoipa::path(
	post,
	path = "/remove",
	operation_id = "inventory_variables_remove",
	tag = "inventory",
	security(("tailscale-admin" = [])),
	request_body = RemoveArgs,
	responses(
		(status = 200, description = "Removed", body = ()),
		(status = 400, description = "Not one scope", body = ProblemDetailsSchema),
		(status = 404, description = "No variable of that name in that scope", body = ProblemDetailsSchema),
		(status = 502, description = "The secret store is unavailable", body = ProblemDetailsSchema),
	),
)]
pub async fn remove(
	State(state): State<AppState>,
	admin: TailscaleAdmin,
	Json(args): Json<RemoveArgs>,
) -> Result<Json<()>> {
	let scope = VariableScope::try_from(args.scope)?;
	let mut conn = state.db.get().await?;

	let Some(removed) = InventoryVariable::remove(&mut conn, scope, &args.name).await? else {
		return Err(AppError::NotFound(format!(
			"no variable {:?} in that scope",
			args.name
		)));
	};

	if removed.is_secret {
		forget_secret_value(&state, scope, &args.name).await?;
	}

	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		secret = removed.is_secret,
		"inventory variable removed"
	);
	Ok(Json(()))
}

/// A value for the secret store, which holds strings. Its JSON encoding, so a
/// value round-trips as the type it was set as.
fn stored(value: &Value) -> SecretString {
	SecretString::from(value.to_string())
}

/// The value behind a name the secret store may be holding. Called where a
/// name is removed, and where it stops being a secret, so a later switch back
/// never resurrects a stale value.
async fn forget_secret_value(state: &AppState, scope: VariableScope, name: &str) -> Result<()> {
	let kube = secret_store(state)?;
	let secret = scope.secret_name();
	let Some(mut keys) = kube.try_read_secret_keys(&secret).await? else {
		return Ok(());
	};
	if keys.remove(name).is_none() {
		return Ok(());
	}
	if keys.is_empty() {
		kube.delete_password(&secret).await
	} else {
		kube.put_secret_keys(&secret, &keys).await
	}
}

pub(super) fn secret_store(state: &AppState) -> Result<&BackupSecrets> {
	state
		.kube
		.as_ref()
		.ok_or_else(|| AppError::Upstream("secret store not configured".into()))
}

/// The longest key a kubernetes Secret takes.
const NAME_LIMIT: usize = 253;

/// The name is the key a secret's value is stored under, so every name is held
/// to what a key may be: `.` and `..` name a path rather than a key, and a key
/// is bounded.
fn check_name(name: &str) -> Result<()> {
	if name.is_empty()
		|| name == "."
		|| name == ".."
		|| name.len() > NAME_LIMIT
		|| !name
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '.' | '_'))
	{
		return Err(AppError::BadRequest(format!(
			"{name:?} is not a usable variable name: letters, digits, `-`, `.` and `_`, \
			 at most {NAME_LIMIT} of them, and not `.` or `..`"
		)));
	}
	Ok(())
}

/// `ansible_host` names one machine, so a wider scope would give every machine
/// in the environment the same address.
fn check_machine_scoped(scope: VariableScope, name: &str) -> Result<()> {
	if name == super::inventory::ANSIBLE_HOST && !matches!(scope, VariableScope::Machine { .. }) {
		return Err(AppError::BadRequest(format!(
			"{name:?} names one machine, so it is set on a machine rather than on a group or an environment"
		)));
	}
	Ok(())
}

async fn check_scope(
	conn: &mut database::diesel_async::AsyncPgConnection,
	scope: VariableScope,
) -> Result<()> {
	match scope {
		VariableScope::Group { group_id } | VariableScope::Environment { group_id, .. } => {
			ServerGroup::get_by_id(conn, group_id).await.map(drop)
		}
		VariableScope::Machine { machine_id } => {
			Machine::get_by_id(conn, machine_id).await.map(drop)
		}
	}
}
