//! Importing an old Ansible inventory file into an environment.
//!
//! The preview and the apply run one plan, so neither the operator interface
//! nor a stale preview can apply what the server would not have proposed: the
//! interface sends the file with the operator's choices, and the server works
//! out everything else again.
// spec: ANSIMP

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::{Json, extract::State};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{audit::Audit, tailscale_auth::TailscaleAdmin};
use commons_types::{Uuid, server::rank::ServerRank};
use database::{
	Device,
	ansible_catalogue::{Catalogue, Loaded, ScopeKind},
	ansible_leases::AnsibleLease,
	ansible_variables::{AnsibleVariable, VariableScope},
	applications::Application,
	audit::{Action, ImportedVariable, Record},
	diesel_async::AsyncPgConnection,
	machines::Machine,
	server_groups::{PartialServerGroup, ServerGroup},
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use super::runs::{MANAGED, truthy};
use super::{
	ANSIBLE_HOST, Environment, EnvironmentArgs, host_of, hosts, inventory_file, reach,
	resolve_environment,
};
use crate::state::AppState;

const ANSIBLE_PORT: &str = "ansible_port";
const CLONE: &str = "tamanu_clone";

/// What a name has to contain to be guessed secret where the catalogue does not
/// say.
const SECRET_WORDS: [&str; 5] = ["salt", "token", "secret", "password", "key"];

/// What the operator has chosen about the import, with the file.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ImportArgs {
	#[serde(flatten)]
	pub environment: EnvironmentArgs,
	/// The text of the inventory file.
	pub file: String,
	/// What the operator has chosen for hosts, by the key the file gives them.
	/// A host not named here takes what Canopy proposed.
	#[serde(default)]
	pub hosts: Vec<HostChoice>,
	/// Which variables the operator has marked or unmarked as secret, where the
	/// catalogue does not decide.
	#[serde(default)]
	pub secrets: Vec<SecretChoice>,
}

/// One host's machine, chosen by the operator.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct HostChoice {
	/// The host's key in the file.
	pub host: String,
	/// The machine to import the host onto, or null to skip the host.
	#[serde(default)]
	pub machine_id: Option<Uuid>,
}

/// One variable marked or unmarked as secret by the operator.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct SecretChoice {
	/// The key of the host whose variable it is, or null for one of the
	/// environment's own.
	#[serde(default)]
	pub host: Option<String>,
	/// The variable's name.
	pub name: String,
	/// Whether it is secret.
	pub secret: bool,
}

/// An import to apply: what was previewed, with the notes as the operator left
/// them.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct ImportApplyArgs {
	#[serde(flatten)]
	pub import: ImportArgs,
	/// The notes to append to the group's, as the operator left them. Omitted
	/// to append what was proposed, and blank to append nothing.
	#[serde(default)]
	pub notes: Option<String>,
	/// The identifier of the environment's machines the preview was taken
	/// against, so an import is refused if they have changed since.
	pub members: String,
}

/// How a host stands against the environment's machines.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum HostState {
	/// It matched exactly one machine, which is proposed.
	Matched,
	/// The operator chose its machine.
	Assigned,
	/// It matched several machines, and none is chosen.
	Ambiguous,
	/// It matched none, and none is chosen.
	Unmatched,
	/// The operator skipped it.
	Skipped,
}

/// Where a variable comes from where it is not simply what the file sets.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
	/// Canopy adds it, as for every machine imported onto.
	Added,
	/// It is read from the host's key in the file.
	FromHostKey,
}

/// One variable the import would set.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ImportVariable {
	/// The variable's name.
	pub name: String,
	/// Its value, and null where it is secret, which a preview does not show.
	#[schema(value_type = Option<serde_json::Value>)]
	pub value: Option<Value>,
	/// Whether it would be set as a secret.
	pub secret: bool,
	/// Whether the catalogue decides that, so the operator cannot change it.
	pub locked: bool,
	/// Where it comes from, where that is not the file's own setting.
	pub origin: Option<Origin>,
	/// What breaks the rules for setting it by hand.
	pub problems: Vec<String>,
}

/// A host in the file, and what the import would do with it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ImportHost {
	/// The host's key in the file.
	pub key: String,
	/// How the host stands against the environment's machines.
	pub state: HostState,
	/// The machine it would be imported onto, where it has one.
	pub machine_id: Option<Uuid>,
	/// The machines it matched, where it matched several.
	pub candidates: Vec<Uuid>,
	/// The variables it would set on its machine. None for a skipped host.
	pub variables: Vec<ImportVariable>,
}

/// A machine of the environment a host can be imported onto.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ImportMachine {
	/// The machine's identifier.
	pub id: Uuid,
	/// The machine's name.
	pub name: String,
}

/// The file says the environment is a clone, or not, against what it is.
#[derive(Debug, Clone, Copy, Serialize, ToSchema)]
pub struct CloneMismatch {
	/// Whether the file sets `tamanu_clone`.
	pub file: bool,
	/// Whether the environment is a clone.
	pub environment: bool,
}

/// Everything the import would do, for the operator to review.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ImportPreview {
	/// Identifier of the group.
	pub group_id: Uuid,
	/// Name of the group.
	pub group: String,
	/// The environment imported into.
	pub rank: ServerRank,
	/// Whether a catalogue is held, which decides which variables are secret
	/// and which are reserved.
	pub catalogue: bool,
	/// The environment's machines, which a host can be assigned to.
	pub machines: Vec<ImportMachine>,
	/// Identifies those machines, to send back when applying.
	pub members: String,
	/// The file's hosts, by key.
	pub hosts: Vec<ImportHost>,
	/// The variables set on the environment itself.
	pub environment: Vec<ImportVariable>,
	/// The reserved variables the file sets, which are left out.
	pub left_out: Vec<String>,
	/// Group variables an environment variable would override.
	pub overrides: Vec<String>,
	/// Set where the file's `tamanu_clone` disagrees with the environment.
	pub clone_mismatch: Option<CloneMismatch>,
	/// The file's comments as proposed notes for the group, blank where there
	/// are none.
	pub notes: String,
	/// What stops the import being applied, beyond a variable's own problems.
	pub problems: Vec<String>,
	/// Whether it can be applied.
	pub can_apply: bool,
}

/// What an applied import did.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ImportApplied {
	/// How many variables were set.
	pub variables: usize,
	/// How many machines they were set on.
	pub machines: usize,
	/// Whether the group's notes were added to.
	pub notes_added: bool,
}

/// A variable to write.
struct Write {
	scope: VariableScope,
	/// The machine it is set on, `None` for the environment.
	machine: Option<(Uuid, String)>,
	name: String,
	value: Value,
	secret: bool,
}

struct Plan {
	preview: ImportPreview,
	writes: Vec<Write>,
	notes: String,
}

/// One machine, as far as matching a host to it goes.
struct Candidate {
	machine: Machine,
	/// Every name or address it answers to, lowercased.
	identifiers: BTreeSet<String>,
	/// The addresses a run would reach it at without an `ansible_host`: its
	/// tailnet name in full and by its first label where a device is bound, and
	/// otherwise the hosts recorded for its applications.
	reached_at: BTreeSet<String>,
}

fn lower(text: &str) -> String {
	text.trim().trim_end_matches('.').to_lowercase()
}

/// A host key as an address and a port: `host:port` where what follows the
/// last colon is a port and nothing before it is a colon.
fn split_key(key: &str) -> (&str, Option<u16>) {
	match key.rsplit_once(':') {
		Some((host, port)) if !host.contains(':') && !host.is_empty() => match port.parse() {
			Ok(port) => (host, Some(port)),
			Err(_) => (key, None),
		},
		_ => (key, None),
	}
}

/// Whether a variable is likely a secret by what it is called: its name, or any
/// key inside its value, holds a word that suggests one.
fn looks_secret(name: &str, value: &Value) -> bool {
	fn mentions(text: &str) -> bool {
		let text = text.to_lowercase();
		SECRET_WORDS.iter().any(|word| text.contains(word))
	}
	fn within(value: &Value) -> bool {
		match value {
			Value::Object(map) => map
				.iter()
				.any(|(key, value)| mentions(key) || within(value)),
			Value::Array(items) => items.iter().any(within),
			_ => false,
		}
	}
	mentions(name) || within(value)
}

fn message(error: AppError) -> String {
	match error {
		AppError::BadRequest(message) | AppError::Conflict(message) => message,
		other => other.to_string(),
	}
}

fn machine_digest(machines: &[Machine]) -> String {
	let mut ids: Vec<String> = machines
		.iter()
		.map(|machine| machine.id.to_string())
		.collect();
	ids.sort();
	database::ansible_catalogue::digest(&Value::from(ids))
}

/// Work out the whole import from the file and the operator's choices.
async fn plan(conn: &mut AsyncPgConnection, args: &ImportArgs) -> Result<Plan> {
	let environment = resolve_environment(conn, &args.environment).await?;
	let Environment {
		group,
		rank,
		applications,
		machines: members,
		at_rank,
	} = &environment;

	let catalogue_loaded = Loaded::get(conn).await?;
	let catalogue: Option<&Catalogue> = catalogue_loaded.as_ref().map(|l| &*l.catalogue);

	// An import never meets a value already set.
	let machine_ids: Vec<Uuid> = at_rank.iter().map(|machine| machine.id).collect();
	let scope = VariableScope::Environment {
		group_id: group.id,
		rank: *rank,
	};
	if !AnsibleVariable::list_at(conn, scope).await?.is_empty()
		|| !AnsibleVariable::list_for_machines(conn, &machine_ids)
			.await?
			.is_empty()
	{
		return Err(AppError::Conflict(format!(
			"the {rank} environment of {:?} already carries variables, so an inventory file \
			 is imported only into one that carries none",
			group.name
		)));
	}

	let file = inventory_file::parse(&args.file)
		.map_err(|error| AppError::BadRequest(format!("not an inventory file: {error}")))?;

	let device_ids: Vec<Uuid> = at_rank
		.iter()
		.filter_map(|machine| machine.device_id)
		.collect();
	let tailnet = Device::tailscale_names_by_ids(conn, &device_ids).await?;

	let candidates: Vec<Candidate> = at_rank
		.iter()
		.map(|machine| {
			let mut identifiers = BTreeSet::from([lower(&machine.name)]);
			let mut reached_at = BTreeSet::new();
			let bound = machine
				.device_id
				.and_then(|device| tailnet.get(&device))
				.map(|name| lower(name));
			if let Some(name) = &bound {
				reached_at.insert(name.clone());
				if let Some(first) = name.split('.').next() {
					reached_at.insert(first.to_owned());
				}
			}
			for application in applications
				.iter()
				.filter(|application| application.machine_id == Some(machine.id))
			{
				if let Some(host) = host_of(application) {
					let host = lower(&host);
					identifiers.insert(host.clone());
					if bound.is_none() {
						reached_at.insert(host);
					}
				}
			}
			identifiers.extend(reached_at.iter().cloned());
			Candidate {
				machine: machine.clone(),
				identifiers,
				reached_at,
			}
		})
		.collect();

	let by_id: HashMap<Uuid, &Candidate> = candidates
		.iter()
		.map(|candidate| (candidate.machine.id, candidate))
		.collect();
	let choices: HashMap<&str, Option<Uuid>> = args
		.hosts
		.iter()
		.map(|choice| (choice.host.as_str(), choice.machine_id))
		.collect();
	for machine in choices.values().flatten() {
		if !by_id.contains_key(machine) {
			return Err(AppError::BadRequest(format!(
				"machine {machine} is not in the {rank} environment of {:?}",
				group.name
			)));
		}
	}
	let marked: HashMap<(Option<&str>, &str), bool> = args
		.secrets
		.iter()
		.map(|choice| {
			(
				(choice.host.as_deref(), choice.name.as_str()),
				choice.secret,
			)
		})
		.collect();

	let mut left_out: BTreeSet<String> = BTreeSet::new();
	let is_reserved = |name: &str| catalogue.is_some_and(|c| c.is_reserved(name));

	// A variable as the preview shows it, and as it would be written.
	let variable = |host: Option<&str>,
	                scope_kind: ScopeKind,
	                name: &str,
	                value: &Value,
	                origin: Option<Origin>|
	 -> (ImportVariable, bool) {
		let (secret, locked) = match catalogue.and_then(|c| c.variable(name)) {
			Some(described) => (described.secret, true),
			None => (
				marked
					.get(&(host, name))
					.copied()
					.unwrap_or_else(|| looks_secret(name, value)),
				false,
			),
		};

		let mut problems = Vec::new();
		if let Err(error) = crate::fns::ansible_variables::check_name(name) {
			problems.push(message(error));
		}
		if name == ANSIBLE_HOST {
			if scope_kind != ScopeKind::Machine {
				problems.push(format!(
					"{name:?} names one machine, so it is set on a machine rather than on an environment"
				));
			} else if !value.is_string() {
				problems.push(format!(
					"{name:?} is the address a run connects to, so it is a string"
				));
			}
		}
		if let Some(catalogue) = catalogue {
			problems.extend(catalogue.violations(scope_kind, name, value));
		}
		(
			ImportVariable {
				name: name.to_owned(),
				value: (!secret).then(|| value.clone()),
				secret,
				locked,
				origin,
				problems,
			},
			secret,
		)
	};

	let mut writes: Vec<Write> = Vec::new();
	let mut problems: Vec<String> = Vec::new();

	let mut environment_variables = Vec::new();
	for (name, value) in &file.environment {
		if is_reserved(name) {
			left_out.insert(name.clone());
			continue;
		}
		let (shown, secret) = variable(None, ScopeKind::Environment, name, value, None);
		environment_variables.push(shown);
		writes.push(Write {
			scope,
			machine: None,
			name: name.clone(),
			value: value.clone(),
			secret,
		});
	}

	let file_clone = file
		.environment
		.get(CLONE)
		.into_iter()
		.chain(
			file.hosts
				.values()
				.flat_map(|host| host.own.get(CLONE).or_else(|| host.inherited.get(CLONE))),
		)
		.any(|value| value == &Value::Bool(true));
	let environment_is_clone = *rank == ServerRank::Clone;
	let clone_mismatch = (file_clone != environment_is_clone).then_some(CloneMismatch {
		file: file_clone,
		environment: environment_is_clone,
	});
	let mut hosts_out = Vec::new();
	let mut taken: BTreeMap<Uuid, Vec<&str>> = BTreeMap::new();
	// What each machine would be reached at, where the file says.
	let mut ansible_host_of: HashMap<Uuid, String> = HashMap::new();

	for (key, host) in &file.hosts {
		let effective = host.effective();
		let (key_host, key_port) = split_key(key);

		let (state, machine, found) = match choices.get(key.as_str()) {
			Some(Some(machine)) => (HostState::Assigned, Some(*machine), Vec::new()),
			Some(None) => (HostState::Skipped, None, Vec::new()),
			None => {
				let mut names: BTreeSet<String> = BTreeSet::from([lower(key), lower(key_host)]);
				for name in ["tailscale_name", "public_hostname", ANSIBLE_HOST] {
					if let Some(text) = effective.get(name).and_then(Value::as_str) {
						names.insert(lower(text));
					}
				}
				let found: Vec<Uuid> = candidates
					.iter()
					.filter(|candidate| !candidate.identifiers.is_disjoint(&names))
					.map(|candidate| candidate.machine.id)
					.collect();
				match found.as_slice() {
					[only] => (HostState::Matched, Some(*only), Vec::new()),
					[] => (HostState::Unmatched, None, Vec::new()),
					_ => (HostState::Ambiguous, None, found),
				}
			}
		};

		if state == HostState::Skipped {
			hosts_out.push(ImportHost {
				key: key.clone(),
				state,
				machine_id: None,
				candidates: Vec::new(),
				variables: Vec::new(),
			});
			continue;
		}
		if let Some(machine) = machine {
			taken.entry(machine).or_default().push(key);
		} else {
			problems.push(format!(
				"host {key:?} is neither matched to a machine nor skipped"
			));
		}
		let target = machine.and_then(|id| by_id.get(&id).copied());

		let mut vars: BTreeMap<String, (Value, Option<Origin>)> = BTreeMap::new();
		for (name, value) in &effective {
			if is_reserved(name) {
				left_out.insert(name.clone());
			} else {
				vars.insert(name.clone(), (value.clone(), None));
			}
		}
		if vars.get(MANAGED).map(|(value, _)| value) != Some(&Value::Bool(true)) {
			vars.insert(MANAGED.into(), (Value::Bool(true), Some(Origin::Added)));
		}
		// Ansible reaches a host by its key where it names no `ansible_host`.
		if !vars.contains_key(ANSIBLE_HOST) {
			let is_address_already =
				target.is_some_and(|candidate| candidate.reached_at.contains(&lower(key_host)));
			if !is_address_already {
				vars.insert(
					ANSIBLE_HOST.into(),
					(
						Value::String(key_host.to_owned()),
						Some(Origin::FromHostKey),
					),
				);
			}
		}
		if let Some(port) = key_port
			&& !host.own.contains_key(ANSIBLE_PORT)
		{
			vars.insert(
				ANSIBLE_PORT.into(),
				(Value::from(port), Some(Origin::FromHostKey)),
			);
		}

		let mut shown_vars = Vec::new();
		for (name, (value, origin)) in &vars {
			let (shown, secret) = variable(Some(key), ScopeKind::Machine, name, value, *origin);
			shown_vars.push(shown);
			if let Some(candidate) = target {
				writes.push(Write {
					scope: VariableScope::Machine {
						machine_id: candidate.machine.id,
					},
					machine: Some((candidate.machine.id, candidate.machine.name.clone())),
					name: name.clone(),
					value: value.clone(),
					secret,
				});
				if name == ANSIBLE_HOST
					&& let Some(address) = value.as_str()
				{
					ansible_host_of.insert(candidate.machine.id, address.to_owned());
				}
			}
		}
		hosts_out.push(ImportHost {
			key: key.clone(),
			state,
			machine_id: machine,
			candidates: found,
			variables: shown_vars,
		});
	}

	for (machine, keys) in &taken {
		if keys.len() > 1 {
			problems.push(format!(
				"hosts {} are all matched to machine {:?}",
				keys.iter()
					.map(|key| format!("{key:?}"))
					.collect::<Vec<_>>()
					.join(", "),
				by_id[machine].machine.name
			));
		}
	}

	// What the members would be reached at and named by once imported.
	let served: Vec<(&Machine, Vec<&Application>)> = members
		.iter()
		.map(|machine| {
			(
				machine,
				applications
					.iter()
					.filter(|application| application.machine_id == Some(machine.id))
					.collect(),
			)
		})
		.collect();
	let reaches: Vec<(Option<String>, bool)> = served
		.iter()
		.map(|(machine, on)| {
			reach(
				machine,
				ansible_host_of.get(&machine.id).map(String::as_str),
				&tailnet,
				on,
			)
		})
		.collect();
	let mut address_seen: BTreeMap<&str, &str> = BTreeMap::new();
	for ((machine, _), (address, _)) in served.iter().zip(&reaches) {
		if let Some(address) = address.as_deref()
			&& let Some(other) = address_seen.insert(address, &machine.name)
		{
			problems.push(format!(
				"machines {other:?} and {:?} would both be reached at {address:?}",
				machine.name
			));
		}
	}

	let group_scope = VariableScope::Group { group_id: group.id };
	let group_variables = AnsibleVariable::list_at(conn, group_scope).await?;

	// A machine a host is imported onto is managed; any other member is only
	// where the environment's variables or the group's make it so, since the
	// import is offered only while no machine carries a variable of its own.
	let managed_wide = file
		.environment
		.get(MANAGED)
		.filter(|_| !is_reserved(MANAGED))
		.or_else(|| {
			group_variables
				.iter()
				.find(|variable| variable.name == MANAGED)
				.and_then(|variable| variable.value.as_ref())
		})
		.is_some_and(truthy);
	if let Err(error) = hosts::host_names(
		&served
			.iter()
			.zip(&reaches)
			.map(|((machine, _), (address, tailnet))| hosts::Candidate {
				id: machine.id,
				name: &machine.name,
				address: address.as_deref(),
				tailnet: *tailnet,
				managed: taken.contains_key(&machine.id) || managed_wide,
			})
			.collect::<Vec<_>>(),
	) {
		problems.push(message(error));
	}

	let group_names: BTreeSet<String> = group_variables
		.into_iter()
		.map(|variable| variable.name)
		.collect();
	let overrides: Vec<String> = environment_variables
		.iter()
		.filter(|variable| group_names.contains(&variable.name))
		.map(|variable| variable.name.clone())
		.collect();

	let notes = if file.comments.is_empty() {
		String::new()
	} else {
		format!("## {rank}\n{}", file.comments.join("\n"))
	};

	let has_variable_problems = environment_variables
		.iter()
		.chain(hosts_out.iter().flat_map(|host| &host.variables))
		.any(|variable| !variable.problems.is_empty());
	let can_apply = problems.is_empty() && !has_variable_problems;

	Ok(Plan {
		preview: ImportPreview {
			group_id: group.id,
			group: group.name.clone(),
			rank: *rank,
			catalogue: catalogue.is_some(),
			machines: at_rank
				.iter()
				.map(|machine| ImportMachine {
					id: machine.id,
					name: machine.name.clone(),
				})
				.collect(),
			members: machine_digest(at_rank),
			hosts: hosts_out,
			environment: environment_variables,
			left_out: left_out.into_iter().collect(),
			overrides,
			clone_mismatch,
			notes: notes.clone(),
			problems,
			can_apply,
		},
		writes,
		notes,
	})
}

/// Show what importing a file would do, changing nothing.
///
/// Reads the file, proposes the machine each host is imported onto, and lists
/// every variable that would be set, where, and whether as a secret.
#[utoipa::path(
	post,
	path = "/import_preview",
	operation_id = "ansible_import_preview",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = ImportArgs,
	responses(
		(status = 200, body = ImportPreview),
		(status = 400, description = "Not an inventory file, or a machine that is not in the environment", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "The environment already carries variables, is archived, or is ambiguous", body = ProblemDetailsSchema),
	),
)]
pub async fn import_preview(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ImportArgs>,
) -> Result<Json<ImportPreview>> {
	let mut conn = state.db.get().await?;
	Ok(Json(plan(&mut conn, &args).await?.preview))
}

/// Apply an import.
///
/// The plan is worked out again against the environment as it stands. An
/// import that cannot be applied, or whose machines have changed since the
/// preview, is refused whole; one that applies sets every variable, writes
/// every secret to the secret store, and appends the notes, or does none of it.
#[utoipa::path(
	post,
	path = "/import_apply",
	operation_id = "ansible_import_apply",
	tag = "ansible",
	security(("tailscale-admin" = [])),
	request_body = ImportApplyArgs,
	responses(
		(status = 200, body = ImportApplied),
		(status = 400, description = "Not an inventory file, or an import that breaks the rules for setting variables", body = ProblemDetailsSchema),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "The environment carries variables, its machines changed since the preview, or a host is not matched or skipped", body = ProblemDetailsSchema),
		(status = 502, description = "The secret store is unavailable", body = ProblemDetailsSchema),
	),
)]
pub async fn import_apply(
	State(state): State<AppState>,
	audit: Audit,
	admin: TailscaleAdmin,
	Json(args): Json<ImportApplyArgs>,
) -> Result<Json<ImportApplied>> {
	let mut conn = audit.begin().await?;
	let resolved = resolve_environment(&mut conn, &args.import.environment).await?;
	// Held for the rest of the transaction, so two imports, or an import and a
	// run's lease, are decided one after the other.
	database::ansible_leases::lock_group(&mut conn, resolved.group.id).await?;

	// What an environment serves changes under a run that holds it, so an import
	// waits for the run to end.
	if let Some(held) = AnsibleLease::open_for(&mut conn, resolved.group.id, resolved.rank).await?
		&& held.holds_at(jiff::Timestamp::now())
	{
		return Err(AppError::Conflict(format!(
			"a run holds this environment, so nothing is imported into it meanwhile: {}",
			held.held_by_another()
		)));
	}

	let Plan {
		preview,
		writes,
		notes,
	} = plan(&mut conn, &args.import).await?;

	if preview.members != args.members {
		return Err(AppError::Conflict(
			"the environment's machines have changed since the preview; review it again".into(),
		));
	}
	if !preview.can_apply {
		let reason = preview
			.problems
			.first()
			.cloned()
			.or_else(|| {
				preview
					.environment
					.iter()
					.chain(preview.hosts.iter().flat_map(|host| &host.variables))
					.find_map(|variable| variable.problems.first().cloned())
			})
			.unwrap_or_else(|| "the import cannot be applied".into());
		return Err(AppError::Conflict(format!(
			"the import cannot be applied: {reason}"
		)));
	}

	let notes = args
		.notes
		.as_deref()
		.map(str::trim)
		.map(str::to_owned)
		.unwrap_or(notes);
	let notes_added = !notes.is_empty();

	let group = ServerGroup::get_by_id(&mut conn, preview.group_id).await?;
	let mut machine_records: Vec<Record> = Vec::new();
	let mut seen = BTreeSet::new();
	for write in &writes {
		if let Some((id, _)) = &write.machine
			&& seen.insert(*id)
		{
			machine_records.push(Record::machine(&mut conn, *id).await?);
		}
	}
	let action = Action::AnsibleInventoryImported {
		group: Record::group(&mut conn, group.id).await?,
		machines: machine_records,
		rank: preview.rank,
		variables: writes
			.iter()
			.map(|write| ImportedVariable {
				name: write.name.clone(),
				machine: write.machine.as_ref().map(|(_, name)| name.clone()),
				secret: write.secret,
			})
			.collect(),
		notes_added,
	};

	let secrets: Vec<&Write> = writes.iter().filter(|write| write.secret).collect();
	let pending = if secrets.is_empty() {
		None
	} else {
		// The secret store is outside Canopy's records, so the entry stands
		// before it is written, and is settled by whether the import took.
		let pending = audit.commit_ahead(action.clone()).await?;
		let store = crate::fns::ansible_variables::secret_store(&state)?;
		let mut written: Vec<&Write> = Vec::new();
		for write in &secrets {
			let stored = algae_cli::passphrases::SecretString::from(write.value.to_string());
			if let Err(failed) = store
				.put_secret_key(&write.scope.secret_name(), &write.name, &stored)
				.await
			{
				forget(store, &written).await;
				// A write that failed part-way may have left its own value
				// behind, so it is forgotten too.
				forget(store, &[*write]).await;
				return Err(audit.effect_failed(&pending, failed).await);
			}
			written.push(write);
		}
		Some(pending)
	};

	let applied = async {
		for write in &writes {
			AnsibleVariable::set(
				&mut conn,
				write.scope,
				&write.name,
				(!write.secret).then_some(&write.value),
				Some(&admin.0.login),
			)
			.await?;
		}
		if notes_added {
			let joined = if group.notes.trim().is_empty() {
				notes.clone()
			} else {
				format!("{}\n\n{notes}", group.notes.trim_end())
			};
			ServerGroup::update(
				&mut conn,
				group.id,
				PartialServerGroup {
					name: None,
					notes: Some(joined),
					tags: None,
					slack_open_delay: None,
					slack_close_delay: None,
				},
			)
			.await?;
		}
		match &pending {
			Some(pending) => conn.settle_done(pending, None).await,
			None => conn.record(action).await,
		}
	}
	.await;

	if let Err(error) = applied {
		// The transaction rolls back, so the secrets it wrote are forgotten
		// and the entry is settled as the import not having happened.
		if let Some(pending) = &pending {
			if let Ok(store) = crate::fns::ansible_variables::secret_store(&state) {
				forget(store, &secrets).await;
			}
			if let Err(settle) = audit.settle_failed(pending, None).await {
				tracing::error!(%settle, "could not settle a failed import");
			}
		}
		return Err(error);
	}

	tracing::info!(
		login = %admin.0.login,
		group = %group.name,
		rank = %preview.rank,
		variables = writes.len(),
		"ansible inventory imported"
	);
	Ok(Json(ImportApplied {
		variables: writes.len(),
		machines: seen.len(),
		notes_added,
	}))
}

/// Forget secrets an import wrote, so a failed import leaves nothing in the
/// store that no variable row points at.
async fn forget(store: &commons_servers::backup_secrets::BackupSecrets, writes: &[&Write]) {
	for write in writes {
		if let Err(failed) = store
			.forget_secret_key(&write.scope.secret_name(), &write.name)
			.await
		{
			tracing::error!(
				name = %write.name,
				error = %failed.error,
				"could not forget a secret a failed import wrote"
			);
		}
	}
}
