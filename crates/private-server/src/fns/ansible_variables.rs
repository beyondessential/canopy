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
	backup_secrets::{BackupSecrets, ExposeSecret},
	effect::EffectFailed,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use commons_types::{Uuid, server::rank::ServerRank};
use database::{
	ansible_catalogue::{
		Catalogue, CataloguePlaybook, CatalogueVariable, Loaded, Problem, ScopeKind,
	},
	ansible_variables::{AnsibleVariable, HeldSecretValue, VariableScope},
	audit::{Action, Actor, Entry, Pending, names as audit_names},
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
		.routes(routes!(read_only, silent: history))
		.routes(routes!(read_only, silent: removed))
		.routes(routes!(write, audit(AnsibleVariableSet): restore))
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

/// A removal, by the entry that records it, so it can be undone.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct Removal {
	/// The entry of the removal, which `restore` undoes.
	pub entry_id: Uuid,
}

/// Name one variable, for its history.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct VariableArgs {
	#[serde(flatten)]
	pub scope: ScopeArgs,
	/// The variable's name.
	pub name: String,
}

/// Name one scope, for what was removed from it.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RemovedArgs {
	#[serde(flatten)]
	pub scope: ScopeArgs,
}

/// Undo one change to a variable, putting back what it replaced or removed.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RestoreArgs {
	#[serde(flatten)]
	pub scope: ScopeArgs,
	/// The variable's name.
	pub name: String,
	/// The entry of the change to undo, from the variable's history.
	pub entry_id: Uuid,
}

/// What a change did to a variable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Change {
	/// It was set, or restored.
	Set,
	/// It was removed.
	Removed,
}

/// One change in a variable's history.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct HistoryEntry {
	/// The entry that records the change.
	pub entry_id: Uuid,
	/// When it was made.
	#[schema(value_type = String, format = DateTime)]
	pub recorded_at: Timestamp,
	/// Who made it.
	pub actor: Actor,
	/// Whether it set the variable or removed it.
	pub change: Change,
	/// Whether the value it set or removed is a secret.
	pub secret: bool,
	/// The plain value it set or removed.
	#[schema(value_type = Option<serde_json::Value>)]
	pub value: Option<Value>,
	/// The plain value a set replaced.
	#[schema(value_type = Option<serde_json::Value>)]
	pub replaced: Option<Value>,
	/// Whether a set replaced a secret.
	pub replaced_secret: bool,
	/// The change a restore undid.
	pub restored_from: Option<Uuid>,
	/// Whether the change can be undone, putting back what it replaced or
	/// removed: a plain value its entry holds, or a secret still held.
	pub undoable: bool,
}

/// A variable removed from a scope in the last week and not set there since.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RemovedVariable {
	/// The variable's name.
	pub name: String,
	/// The removal, as its history names it.
	#[serde(flatten)]
	pub removal: HistoryEntry,
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
		(status = 400, description = "Not one scope, not a usable variable name, an `ansible_host` outside machine scope or not a string, or a `managed_by_ansible` outside machine scope", body = ProblemDetailsSchema),
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
	let set = set_value(
		&state,
		&audit,
		&admin.0.login,
		scope,
		&args.name,
		args.value,
		args.secret,
		None,
	)
	.await?;
	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		secret = args.secret,
		"ansible variable set"
	);
	Ok(Json(set))
}

/// Set a variable, recording the plain value set and the plain value it
/// replaced, and holding for a while a secret value it replaced.
// spec: ANSVAR#history
#[allow(clippy::too_many_arguments)]
async fn set_value(
	state: &AppState,
	audit: &Audit,
	login: &str,
	scope: VariableScope,
	name: &str,
	value: Value,
	secret: bool,
	restored_from: Option<Uuid>,
) -> Result<AnsibleVariable> {
	check_name(name)?;
	check_ansible_host(scope, name, &value)?;
	check_managed(scope, name)?;

	let mut conn = audit.begin().await?;
	check_scope(&mut conn, scope).await?;
	check_catalogue(&mut conn, scope, name, &value, secret).await?;
	let (variable, scope_record, rank) = AnsibleVariable::records(&mut conn, scope, name).await?;
	// Taken before the read, so an import into the group is not let in between
	// it and the write, nor another change to the name between what this
	// replaces and its write.
	scope.lock_group(&mut conn).await?;
	let prior = AnsibleVariable::at(&mut conn, scope, name).await?;
	let replaced_secret = prior.as_ref().is_some_and(|prior| prior.is_secret);
	let action = Action::AnsibleVariableSet {
		variable,
		scope: scope_record,
		rank,
		secret,
		value: (!secret).then(|| value.clone()),
		replaced: prior.and_then(|prior| prior.value),
		replaced_secret,
		restored_from,
	};

	if !secret && !replaced_secret {
		let set = AnsibleVariable::set(&mut conn, scope, name, Some(&value), Some(login)).await?;
		conn.record(action).await?;
		return Ok(set);
	}

	// The secret store is outside Canopy's records, so the entry stands before
	// it is written, and is settled by whether it took.
	let store = secret_store(state)?;
	let pending = audit.commit_ahead(action).await?;
	let held = replaced_secret && hold_replaced(audit, &pending, store, scope, name).await?;
	let written = if secret {
		store
			.put_secret_key(&scope.secret_name(), name, &stored(&value))
			.await
	} else {
		// A secret becoming a plain value leaves nothing behind it in the store
		// but what is held, so a later switch back never resurrects it.
		store.forget_secret_key(&scope.secret_name(), name).await
	};
	if let Err(failed) = written {
		if held {
			forget_held(store, scope, pending.id).await;
		}
		return Err(audit.effect_failed(&pending, failed).await);
	}

	let applied = async {
		let set = AnsibleVariable::set(
			&mut conn,
			scope,
			name,
			(!secret).then_some(&value),
			Some(login),
		)
		.await?;
		if held {
			HeldSecretValue::hold(&mut conn, pending.id, scope, name).await?;
		}
		conn.settle_done(&pending, None).await?;
		Ok(set)
	}
	.await;
	if applied.is_err() && held {
		forget_held(store, scope, pending.id).await;
	}
	applied
}

/// Forget a variable, value and all.
///
/// The row and its stored value go together: when the secret store refuses,
/// both stay, and when it does not answer the row stays for a retry to remove.
/// A secret's value is held for a while first, so the removal can be undone.
#[utoipa::path(
	post,
	path = "/remove",
	operation_id = "ansible_variables_remove",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = RemoveArgs,
	responses(
		(status = 200, description = "Removed", body = Removal),
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
) -> Result<Json<Removal>> {
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
		value: removed.value.clone(),
	};

	let entry_id = if removed.is_secret {
		// Forgetting the stored value is outside Canopy's records, so the entry
		// stands before it, and is settled by whether it took.
		let store = secret_store(&state)?;
		let pending = audit.commit_ahead(action).await?;
		let held = hold_replaced(&audit, &pending, store, scope, &args.name).await?;
		if let Err(failed) = store
			.forget_secret_key(&scope.secret_name(), &args.name)
			.await
		{
			if held {
				forget_held(store, scope, pending.id).await;
			}
			return Err(audit.effect_failed(&pending, failed).await);
		}
		let applied = async {
			if held {
				HeldSecretValue::hold(&mut conn, pending.id, scope, &args.name).await?;
			}
			conn.settle_done(&pending, None).await
		}
		.await;
		if applied.is_err() && held {
			forget_held(store, scope, pending.id).await;
		}
		applied?;
		pending.id
	} else {
		conn.record(action).await?
	};

	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		secret = removed.is_secret,
		"ansible variable removed"
	);
	Ok(Json(Removal { entry_id }))
}

const HISTORY_ACTIONS: [&str; 2] = [
	audit_names::AnsibleVariableSet,
	audit_names::AnsibleVariableRemoved,
];

/// The change an entry records, read from its detail.
async fn history_entry(
	conn: &mut database::diesel_async::AsyncPgConnection,
	entry: Entry,
) -> Result<HistoryEntry> {
	let detail = &entry.detail;
	let flag = |key: &str| detail.get(key).and_then(Value::as_bool).unwrap_or(false);
	let plain = |key: &str| detail.get(key).filter(|value| !value.is_null()).cloned();
	let change = if entry.action == audit_names::AnsibleVariableRemoved {
		Change::Removed
	} else {
		Change::Set
	};
	let secret = flag("secret");
	let replaced = plain("replaced");
	let replaced_secret = flag("replaced_secret");
	let value = plain("value");
	let put_away_secret = match change {
		Change::Set => replaced_secret,
		Change::Removed => secret,
	};
	let undoable = if put_away_secret {
		HeldSecretValue::get(conn, entry.id).await?.is_some()
	} else {
		match change {
			Change::Set => replaced.is_some(),
			Change::Removed => value.is_some(),
		}
	};
	Ok(HistoryEntry {
		entry_id: entry.id,
		recorded_at: entry.recorded_at,
		actor: entry.actor,
		change,
		secret,
		value,
		replaced,
		replaced_secret,
		restored_from: detail
			.get("restored_from")
			.and_then(Value::as_str)
			.and_then(|id| id.parse().ok()),
		undoable,
	})
}

/// A variable's history: each time it was set, restored, or removed, newest
/// first, as far back as the audit log's database holds.
// spec: ANSVAR#history
#[utoipa::path(
	post,
	path = "/history",
	operation_id = "ansible_variables_history",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = VariableArgs,
	responses(
		(status = 200, body = Vec<HistoryEntry>),
		(status = 400, description = "Not one scope", body = ProblemDetailsSchema),
	),
)]
pub async fn history(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<VariableArgs>,
) -> Result<Json<Vec<HistoryEntry>>> {
	let scope = VariableScope::try_from(args.scope)?;
	let mut conn = state.db.get().await?;
	let entries = database::audit::naming(
		&mut conn,
		&scope.variable_record_id(&args.name),
		&HISTORY_ACTIONS,
		None,
		200,
	)
	.await?;
	let mut out = Vec::with_capacity(entries.len());
	for entry in entries {
		out.push(history_entry(&mut conn, entry).await?);
	}
	Ok(Json(out))
}

/// The variables removed from a scope in the last week and not set there since,
/// newest first, so a removal can be undone after its offer has gone.
// spec: ANSVAR#presentation
#[utoipa::path(
	post,
	path = "/removed",
	operation_id = "ansible_variables_removed",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = RemovedArgs,
	responses(
		(status = 200, body = Vec<RemovedVariable>),
		(status = 400, description = "Not one scope", body = ProblemDetailsSchema),
	),
)]
pub async fn removed(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<RemovedArgs>,
) -> Result<Json<Vec<RemovedVariable>>> {
	let scope = VariableScope::try_from(args.scope)?;
	let mut conn = state.db.get().await?;
	let scope_id = match scope {
		VariableScope::Group { group_id } | VariableScope::Environment { group_id, .. } => group_id,
		VariableScope::Machine { machine_id } => machine_id,
	};
	let set_here: std::collections::BTreeSet<String> = AnsibleVariable::list_at(&mut conn, scope)
		.await?
		.into_iter()
		.map(|variable| variable.name)
		.collect();
	let since = Timestamp::now() - database::ansible_variables::HELD_FOR;
	let entries = database::audit::naming(
		&mut conn,
		&scope_id.to_string(),
		&[audit_names::AnsibleVariableRemoved],
		Some(since),
		500,
	)
	.await?;

	// The scope's group record also names its environments' variables, so the
	// variable's own record says which scope each removal was at.
	let prefix = format!("{}:", scope.secret_name());
	let mut seen = std::collections::BTreeSet::new();
	let mut out = Vec::new();
	for entry in entries {
		let Some(name) = entry
			.records
			.iter()
			.find_map(|record| record.id.strip_prefix(&prefix))
			.map(str::to_owned)
		else {
			continue;
		};
		if set_here.contains(&name) || !seen.insert(name.clone()) {
			continue;
		}
		out.push(RemovedVariable {
			name,
			removal: history_entry(&mut conn, entry).await?,
		});
	}
	Ok(Json(out))
}

/// Undo a change to a variable, putting back what it replaced or removed.
///
/// A plain value comes from the change's entry, and a secret from where it is
/// held for a week after the change. Restoring is a set like any other, so a
/// secret it replaces is held in turn.
// spec: ANSVAR#history
#[utoipa::path(
	post,
	path = "/restore",
	operation_id = "ansible_variables_restore",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = RestoreArgs,
	responses(
		(status = 200, body = AnsibleVariable),
		(status = 400, description = "Not one scope, a change to another variable, a change that put nothing away, or a value the catalogue no longer allows there", body = ProblemDetailsSchema),
		(status = 404, description = "No such change in the audit log's database", body = ProblemDetailsSchema),
		(status = 409, description = "The secret value it put away is no longer held", body = ProblemDetailsSchema),
		(status = 502, description = "The secret store is unavailable", body = ProblemDetailsSchema),
	),
)]
pub async fn restore(
	State(state): State<AppState>,
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<RestoreArgs>,
) -> Result<Json<AnsibleVariable>> {
	let scope = VariableScope::try_from(args.scope)?;
	let (value, secret) = {
		let mut conn = state.db.get().await?;
		let entry = database::audit::get_done(&mut conn, args.entry_id)
			.await?
			.ok_or_else(|| {
				AppError::NotFound(format!("no change {} in the audit log", args.entry_id))
			})?;
		let record_id = scope.variable_record_id(&args.name);
		if !HISTORY_ACTIONS.contains(&entry.action.as_str())
			|| !entry.records.iter().any(|record| record.id == record_id)
		{
			return Err(AppError::BadRequest(format!(
				"change {} is not one to {:?} there",
				args.entry_id, args.name
			)));
		}
		let change = history_entry(&mut conn, entry).await?;
		let put_away_secret = match change.change {
			Change::Set => change.replaced_secret,
			Change::Removed => change.secret,
		};
		if put_away_secret {
			if !change.undoable {
				return Err(AppError::Conflict(format!(
					"the secret value change {} put away was held for a week and is now forgotten; \
					 it can be recovered from the escrow",
					args.entry_id
				)));
			}
			let held = secret_store(&state)?
				.try_read_secret_keys(&scope.held_secret_name())
				.await?
				.and_then(|mut keys| keys.remove(&args.entry_id.to_string()))
				.ok_or_else(|| {
					AppError::Upstream(format!(
						"the secret value change {} put away is not in the secret store",
						args.entry_id
					))
				})?;
			(unstored(held.expose_secret()), true)
		} else {
			let put_away = match change.change {
				Change::Set => change.replaced,
				Change::Removed => change.value,
			};
			let value = put_away.ok_or_else(|| {
				AppError::BadRequest(format!(
					"change {} replaced nothing, so there is nothing to put back",
					args.entry_id
				))
			})?;
			(value, false)
		}
	};

	let set = set_value(
		&state,
		&audit,
		&admin.0.login,
		scope,
		&args.name,
		value,
		secret,
		Some(args.entry_id),
	)
	.await?;
	tracing::info!(
		login = %admin.0.login,
		scope = ?scope,
		name = %args.name,
		entry = %args.entry_id,
		"ansible variable restored"
	);
	Ok(Json(set))
}

/// A value for the secret store, which holds strings. Its JSON encoding, so a
/// value round-trips as the type it was set as.
fn stored(value: &Value) -> SecretString {
	SecretString::from(value.to_string())
}

/// A value read back from the secret store, as the JSON it was stored as, or
/// the text itself where it is not JSON.
pub(super) fn unstored(held: &str) -> Value {
	serde_json::from_str(held).unwrap_or_else(|_| Value::String(held.to_owned()))
}

/// Copy the secret value a change is about to replace or remove into the
/// scope's held Secret, under the change's entry, so the change can be undone
/// for a while. Answers whether anything was held: nothing is where the store
/// has no value under the name.
///
/// The entry is committed ahead of it, and settled failed here where the store
/// is known to have written nothing.
// spec: ANSVAR#history
async fn hold_replaced(
	audit: &Audit,
	pending: &Pending,
	store: &BackupSecrets,
	scope: VariableScope,
	name: &str,
) -> Result<bool> {
	let current = match store.try_read_secret_keys(&scope.secret_name()).await {
		Ok(keys) => keys.and_then(|mut keys| keys.remove(name)),
		Err(error) => {
			return Err(audit
				.effect_failed(pending, EffectFailed::nothing_done(error))
				.await);
		}
	};
	let Some(current) = current else {
		return Ok(false);
	};
	if let Err(failed) = store
		.put_secret_key(&scope.held_secret_name(), &pending.id.to_string(), &current)
		.await
	{
		return Err(audit.effect_failed(pending, failed).await);
	}
	Ok(true)
}

/// Forget a value held under an entry whose change did not take, so the held
/// Secret keeps nothing no row notes.
async fn forget_held(store: &BackupSecrets, scope: VariableScope, entry_id: Uuid) {
	if let Err(failed) = store
		.forget_secret_key(&scope.held_secret_name(), &entry_id.to_string())
		.await
	{
		tracing::error!(
			error = %failed.error,
			%entry_id,
			"could not forget a secret value held for a change that did not take"
		);
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

/// `managed_by_ansible` says what a run may do to one machine, and is set by a
/// switch on the machine, so a wider value would be one no switch shows.
// spec: INV#managed-machines
fn check_managed(scope: VariableScope, name: &str) -> Result<()> {
	if name == super::ansible::MANAGED && !matches!(scope, VariableScope::Machine { .. }) {
		return Err(AppError::BadRequest(format!(
			"{name:?} says whether a run acts on one machine, so it is set on a machine rather than on a group or an environment"
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
