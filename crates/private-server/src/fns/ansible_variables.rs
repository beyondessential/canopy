//! Setting and listing the variables that configure an environment.
//!
//! A secret's value goes to canopy's secret store and is served back only as
//! part of what an environment serves (see [`super::ansible`]). Nothing here returns one.
// spec: ANSVAR#variables

use algae_cli::passphrases::SecretString;
use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	backup_secrets::BackupSecrets,
	effect::EffectFailed,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use commons_types::{Uuid, server::rank::ServerRank};
use database::{
	ansible_catalogue::{
		Catalogue, CataloguePlaybook, CatalogueVariable, Loaded, Problem, ScopeKind,
	},
	ansible_variables::{AnsibleVariable, VariableScope},
	audit::{Action, Pending},
	machines::Machine,
	server_groups::ServerGroup,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: for_group))
		.routes(routes!(read_only, silent: catalogue))
		.routes(routes!(write, audit(AnsibleVariableSet): set))
		.routes(routes!(write, audit(AnsibleVariableRemoved): remove))
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
	/// as part of what an environment serves.
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

/// A variable, and where the catalogue disagrees with it as it stands.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AnsibleVariableView {
	#[serde(flatten)]
	pub variable: AnsibleVariable,
	/// What is wrong with the variable against the catalogue as it is now: a
	/// value that no longer matches its type, a scope it may no longer be set
	/// at, a name that has become reserved, or a value the catalogue calls
	/// secret that is held in the clear. Empty where it agrees, and where no
	/// catalogue has been read. A secret's value cannot be checked, so only
	/// where it is set is.
	pub problems: Vec<String>,
}

/// Everything set under a group: its own, its environments', and those of the
/// machines in it.
///
/// A secret's value is not among them; it is served only as part of what an
/// environment serves, and only to an administrator.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "ansible_variables_for_group",
	tag = "ansible",
	security(("tailscale-user" = [])),
	request_body = VariablesForGroupArgs,
	responses(
		(status = 200, body = Vec<AnsibleVariableView>),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
	),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<VariablesForGroupArgs>,
) -> Result<Json<Vec<AnsibleVariableView>>> {
	let mut conn = state.db.get().await?;
	ServerGroup::get_by_id(&mut conn, args.server_group_id).await?;
	let catalogue = Loaded::get(&mut conn).await?;
	Ok(Json(
		AnsibleVariable::list_under_group(&mut conn, args.server_group_id)
			.await?
			.into_iter()
			.map(|variable| AnsibleVariableView {
				problems: catalogue
					.as_ref()
					.map(|loaded| problems(&loaded.catalogue, &variable))
					.unwrap_or_default(),
				variable,
			})
			.collect(),
	))
}

fn problems(catalogue: &Catalogue, variable: &AnsibleVariable) -> Vec<String> {
	let scope = ScopeKind::from(variable.scope());
	let mut problems = match &variable.value {
		Some(value) => catalogue.violations(scope, &variable.name, value),
		None => catalogue.placement_violations(scope, &variable.name),
	};
	if !variable.is_secret && catalogue.is_secret(&variable.name) {
		problems.push(format!(
			"{:?} is secret in the catalogue but held in the clear; Canopy moves it into the secret store when it next reads the catalogue",
			variable.name
		));
	}
	problems
}

/// What the catalogue says, and when it was read.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CatalogueView {
	/// The variables it describes, by name.
	pub variables: Vec<CatalogueVariable>,
	/// The playbooks a run can start.
	pub playbooks: Vec<CataloguePlaybook>,
	/// What it says of tags that have a documented meaning.
	pub tags: BTreeMap<String, String>,
	/// The infra commit it was read at.
	pub source_commit: Option<String>,
	/// When this copy was read.
	#[schema(value_type = String, format = DateTime)]
	pub read_at: Timestamp,
	/// Why the latest attempt to read a newer one did not replace it, where it
	/// did not.
	pub problem: Option<Problem>,
	/// What was wrong with it.
	pub problem_detail: Option<String>,
}

/// The catalogue Canopy holds.
///
/// It shapes how a variable is entered and which playbooks a run can start.
/// Null while Canopy has never read one.
#[utoipa::path(
	post,
	path = "/catalogue",
	operation_id = "ansible_variables_catalogue",
	tag = "ansible",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Option<CatalogueView>),
	),
)]
pub async fn catalogue(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<Option<CatalogueView>>> {
	let mut conn = state.db.get().await?;
	let Some(loaded) = Loaded::get(&mut conn).await? else {
		return Ok(Json(None));
	};
	Ok(Json(Some(CatalogueView {
		variables: loaded.catalogue.variables().cloned().collect(),
		playbooks: loaded.catalogue.playbooks().to_vec(),
		tags: loaded.catalogue.tag_notes().clone(),
		source_commit: loaded.source_commit,
		read_at: loaded.read_at,
		problem: loaded.problem,
		problem_detail: loaded.problem_detail,
	})))
}

/// Set or replace a variable.
///
/// A secret's value goes to the secret store keyed by name, and the row holds
/// no value at all. Turning a secret into a plain variable forgets the stored
/// value, so a later switch back never resurrects a stale one.
#[utoipa::path(
	post,
	path = "/set",
	operation_id = "ansible_variables_set",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = SetArgs,
	responses(
		(status = 200, body = AnsibleVariable),
		(status = 400, description = "Not one scope, not a usable variable name, or an `ansible_host` outside machine scope or not a string", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group or machine", body = ProblemDetailsSchema),
		(status = 502, description = "The secret store is unavailable", body = ProblemDetailsSchema),
	),
)]
pub async fn set(
	State(state): State<AppState>,
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<SetArgs>,
) -> Result<Json<AnsibleVariable>> {
	let scope = VariableScope::try_from(args.scope)?;
	check_name(&args.name)?;
	check_ansible_host(scope, &args.name, &args.value)?;

	let mut conn = audit.begin().await?;
	check_scope(&mut conn, scope).await?;
	check_catalogue(&mut conn, scope, &args.name, &args.value, args.secret).await?;
	let (variable, scope_record, rank) =
		AnsibleVariable::records(&mut conn, scope, &args.name).await?;
	let action = |secret| Action::AnsibleVariableSet {
		variable: variable.clone(),
		scope: scope_record.clone(),
		rank,
		secret,
	};

	let set = if args.secret {
		let store = secret_store(&state)?;
		// The secret store is outside Canopy's records, so the entry stands
		// before it is written, and is settled by whether it took.
		let pending = audit.commit_ahead(action(true)).await?;
		if let Err(failed) = store
			.put_secret_key(&scope.secret_name(), &args.name, &stored(&args.value))
			.await
		{
			return Err(audit.effect_failed(&pending, failed).await);
		}

		let set =
			AnsibleVariable::set(&mut conn, scope, &args.name, None, Some(&admin.0.login)).await?;
		conn.settle_done(&pending, None).await?;
		set
	} else {
		let was_secret = AnsibleVariable::at(&mut conn, scope, &args.name)
			.await?
			.is_some_and(|prior| prior.is_secret);
		let set = AnsibleVariable::set(
			&mut conn,
			scope,
			&args.name,
			Some(&args.value),
			Some(&admin.0.login),
		)
		.await?;
		if was_secret {
			// Forgetting the value it held as a secret is outside Canopy's
			// records, as writing one is.
			let pending = audit.commit_ahead(action(false)).await?;
			forget_secret_value(&audit, &pending, &state, scope, &args.name).await?;
			conn.settle_done(&pending, None).await?;
		} else {
			conn.record(action(false)).await?;
		}
		set
	};

	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		secret = args.secret,
		"ansible variable set"
	);
	Ok(Json(set))
}

/// Forget a variable, value and all.
///
/// The row and its stored value go together: when the secret store refuses,
/// both stay, and when it does not answer the row stays for a retry to remove.
#[utoipa::path(
	post,
	path = "/remove",
	operation_id = "ansible_variables_remove",
	tag = "ansible",
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
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<RemoveArgs>,
) -> Result<Json<()>> {
	let scope = VariableScope::try_from(args.scope)?;
	let mut conn = audit.begin().await?;

	let Some(removed) = AnsibleVariable::remove(&mut conn, scope, &args.name).await? else {
		return Err(AppError::NotFound(format!(
			"no variable {:?} in that scope",
			args.name
		)));
	};
	let (variable, scope_record, rank) =
		AnsibleVariable::records(&mut conn, scope, &args.name).await?;
	let action = Action::AnsibleVariableRemoved {
		variable,
		scope: scope_record,
		rank,
		secret: removed.is_secret,
	};

	if removed.is_secret {
		// Forgetting the stored value is outside Canopy's records, so the entry
		// stands before it, and is settled by whether it took.
		let pending = audit.commit_ahead(action).await?;
		forget_secret_value(&audit, &pending, &state, scope, &args.name).await?;
		conn.settle_done(&pending, None).await?;
	} else {
		conn.record(action).await?;
	}

	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		secret = removed.is_secret,
		"ansible variable removed"
	);
	Ok(Json(()))
}

/// A value for the secret store, which holds strings. Its JSON encoding, so a
/// value round-trips as the type it was set as.
fn stored(value: &Value) -> SecretString {
	SecretString::from(value.to_string())
}

/// Forget the value behind a name the secret store may be holding. Called where
/// a name is removed, and where it stops being a secret, so a later switch back
/// never resurrects a stale value.
///
/// The action's entry is committed ahead of it, and settled failed here where
/// the store is known to have written nothing. The caller settles it done.
async fn forget_secret_value(
	audit: &Audit,
	pending: &Pending,
	state: &AppState,
	scope: VariableScope,
	name: &str,
) -> Result<()> {
	let forgot = match secret_store(state) {
		Ok(store) => store.forget_secret_key(&scope.secret_name(), name).await,
		Err(error) => Err(EffectFailed::nothing_done(error)),
	};
	match forgot {
		Ok(()) => Ok(()),
		Err(failed) => Err(audit.effect_failed(pending, failed).await),
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
pub(super) fn check_name(name: &str) -> Result<()> {
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

/// Hold a variable the catalogue describes to it: its type, the scopes it may be
/// set at, that a run's own variables are not set, and that it is secret exactly
/// when the catalogue says so. A name the catalogue does not describe, and any
/// name while no catalogue has been read, is not held to anything.
// spec: ANSVAR#catalogue
async fn check_catalogue(
	conn: &mut database::diesel_async::AsyncPgConnection,
	scope: VariableScope,
	name: &str,
	value: &Value,
	secret: bool,
) -> Result<()> {
	let Some(loaded) = Loaded::get(conn).await? else {
		return Ok(());
	};
	let catalogue = &loaded.catalogue;
	let violations = catalogue.violations(ScopeKind::from(scope), name, value);
	if !violations.is_empty() {
		return Err(AppError::BadRequest(violations.join("; ")));
	}
	if catalogue.variable(name).is_some() && catalogue.is_secret(name) != secret {
		return Err(AppError::BadRequest(if secret {
			format!("{name:?} is not secret in the catalogue, so it is set as a plain value")
		} else {
			format!("{name:?} is secret in the catalogue, so it is set as a secret")
		}));
	}
	Ok(())
}

/// `ansible_host` names one machine, so a wider scope would give every machine
/// in the environment the same address. It is served as the address a run
/// connects to, which is read as a string, so a value of any other shape would
/// leave the machine on the address it was overriding.
fn check_ansible_host(scope: VariableScope, name: &str, value: &Value) -> Result<()> {
	if name != super::ansible::ANSIBLE_HOST {
		return Ok(());
	}
	if !matches!(scope, VariableScope::Machine { .. }) {
		return Err(AppError::BadRequest(format!(
			"{name:?} names one machine, so it is set on a machine rather than on a group or an environment"
		)));
	}
	if !value.is_string() {
		return Err(AppError::BadRequest(format!(
			"{name:?} is the address a run connects to, so it is a string"
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
