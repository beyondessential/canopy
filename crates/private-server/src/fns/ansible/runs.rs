//! What a run is started with: the machines it acts on, the limit it is given,
//! and the playbook, tags and assignments its lease records.
//!
//! A run acts on the members of an environment that carry `managed_by_ansible`,
//! since the plugin builds an inventory of those alone. They are grouped as the
//! plugin groups them, by the products on them and the roles those play, so a
//! limit means here what it means to the run.
// spec: ANSRUN#presentation

use std::collections::{BTreeMap, BTreeSet, HashMap};

use axum::{Json, extract::State};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::audit::AuditTx;
use commons_servers::tailscale_auth::TailscaleUser;
use commons_types::Uuid;
use database::{
	Device,
	ansible_catalogue::Loaded,
	ansible_leases::AnsibleLease,
	ansible_variables::{AnsibleVariable, VariableScope},
	applications::Application,
	audit::Action,
	diesel_async::AsyncPgConnection,
	maintenance_windows::MaintenanceWindow,
	server_groups::ServerGroup,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use utoipa::ToSchema;

use super::{ANSIBLE_HOST, Environment, EnvironmentArgs, hosts, limit, reach, resolve_environment};
use crate::state::AppState;

pub(super) const MANAGED: &str = "managed_by_ansible";
const CENTRAL: &str = "tamanu-central";

/// The application type to assign a machine with nothing on it, or none.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct AssignmentArg {
	/// The machine, which has nothing on it.
	pub machine_id: Uuid,
	/// The application type the run carries it as, or null to leave it
	/// unassigned.
	#[serde(default)]
	pub application_type: Option<String>,
}

/// One machine a run acts on.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RunMember {
	/// Identifier of the machine.
	pub id: Uuid,
	/// The machine's name.
	pub name: String,
	/// The name a run's limit and output address it by.
	pub host_name: String,
	/// The Ansible groups a run puts it in, besides `all`.
	pub groups: Vec<String>,
	/// The application types on the machine. Empty for one nothing is on yet.
	pub application_types: Vec<String>,
}

/// The machines a run on an environment acts on.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RunOptions {
	/// The managed members of the environment, ordered by name.
	pub members: Vec<RunMember>,
	/// Whether the environment already holds a Tamanu central, of which it holds
	/// at most one.
	pub has_central: bool,
	/// How long, in seconds, a lease waits for its run to start before Canopy
	/// releases it.
	pub pending_for_secs: i64,
}

/// A limit to evaluate against an environment's machines.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct LimitPreviewArgs {
	#[serde(flatten)]
	pub environment: EnvironmentArgs,
	/// The limit as an Ansible host pattern.
	pub limit: String,
	/// The types assigned to machines with nothing on them, which decide the
	/// groups those machines are in.
	#[serde(default)]
	pub assignments: Vec<AssignmentArg>,
}

/// A machine a limit matches.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LimitMatch {
	/// Identifier of the machine.
	pub id: Uuid,
	/// The machine's name.
	pub name: String,
	/// The name a run addresses it by.
	pub host_name: String,
}

/// What a limit matches.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct LimitPreview {
	/// The machines it matches.
	pub matches: Vec<LimitMatch>,
	/// The groups a limit can name.
	pub groups: Vec<String>,
	/// The host names a limit can name.
	pub hosts: Vec<String>,
	/// Why the limit cannot be evaluated, where it cannot.
	pub error: Option<String>,
}

/// What a run is limited to and assigned, checked against the environment.
pub struct Checked {
	pub limited_to: Option<Vec<Uuid>>,
	/// Machine identifier to the type it is assigned.
	pub assignments: BTreeMap<Uuid, String>,
	/// The members a run acts on, for naming what was matched and assigned.
	pub members: Vec<Member>,
}

/// A machine a run acts on, with what is needed to name and group it.
pub struct Member {
	pub id: Uuid,
	pub name: String,
	pub host_name: String,
	pub groups: BTreeSet<String>,
	pub application_types: Vec<String>,
}

/// Whether a variable's value says its machine is one a run configures, as the
/// plugin reads it.
pub(super) fn truthy(value: &Value) -> bool {
	match value {
		Value::Bool(flag) => *flag,
		Value::String(text) => matches!(
			text.to_lowercase().as_str(),
			"yes" | "true" | "on" | "1" | "y" | "t"
		),
		Value::Number(number) => number.as_i64() == Some(1),
		_ => false,
	}
}

/// The groups the plugin puts a machine in: the product of each application
/// type on it and the role it plays, a type naming no role being a standalone.
///
/// This mirrors `ansible_groups` in infra's `inventory_plugins/canopy.py`, which
/// reads the product and role off the type's first hyphen. A limit is previewed
/// against these groups, so the two have to change together.
fn ansible_groups<'a>(types: impl Iterator<Item = &'a str>) -> BTreeSet<String> {
	let mut groups = BTreeSet::new();
	for application_type in types {
		match application_type.split_once('-') {
			Some((product, role)) => {
				groups.insert(product.to_owned());
				groups.insert(role.to_owned());
			}
			None => {
				groups.insert(application_type.to_owned());
				groups.insert("standalone".to_owned());
			}
		}
	}
	groups
}

/// The managed members of an environment, named and grouped as a run sees them.
/// A machine nothing is on is grouped by the type it is assigned, where it has
/// one.
pub async fn members(
	conn: &mut AsyncPgConnection,
	environment: &Environment,
	assigned: &BTreeMap<Uuid, String>,
) -> Result<Vec<Member>> {
	let Environment {
		group,
		rank,
		applications,
		machines,
		..
	} = environment;

	let device_ids: Vec<Uuid> = machines
		.iter()
		.filter_map(|machine| machine.device_id)
		.collect();
	let tailnet = Device::tailscale_names_by_ids(conn, &device_ids).await?;

	// What the machine would be reached at and whether a run configures it come
	// from plain variables: neither is ever a secret.
	let machine_ids: Vec<Uuid> = machines.iter().map(|machine| machine.id).collect();
	let mut wide: HashMap<String, Value> = HashMap::new();
	for scope in [
		VariableScope::Group { group_id: group.id },
		VariableScope::Environment {
			group_id: group.id,
			rank: *rank,
		},
	] {
		for variable in AnsibleVariable::list_at(conn, scope).await? {
			if let Some(value) = variable.value {
				wide.insert(variable.name, value);
			}
		}
	}
	let mut own: HashMap<Uuid, HashMap<String, Value>> = HashMap::new();
	for variable in AnsibleVariable::list_for_machines(conn, &machine_ids).await? {
		if let (Some(machine), Some(value)) = (variable.machine_id, variable.value) {
			own.entry(machine).or_default().insert(variable.name, value);
		}
	}

	struct Seen<'a> {
		machine: &'a database::machines::Machine,
		address: Option<String>,
		tailnet: bool,
		managed: bool,
		types: Vec<String>,
	}
	let mut seen: Vec<Seen<'_>> = Vec::new();
	for machine in machines {
		let get = |name: &str| {
			own.get(&machine.id)
				.and_then(|vars| vars.get(name))
				.or_else(|| wide.get(name))
		};
		let on_machine: Vec<&Application> = applications
			.iter()
			.filter(|application| application.machine_id == Some(machine.id))
			.collect();
		let (address, tailnet_named) = reach(
			machine,
			get(ANSIBLE_HOST).and_then(Value::as_str),
			&tailnet,
			&on_machine,
		);
		seen.push(Seen {
			machine,
			address,
			tailnet: tailnet_named,
			managed: get(MANAGED).is_some_and(truthy),
			types: on_machine
				.iter()
				.map(|application| application.r#type.to_string())
				.collect(),
		});
	}

	// Named among the whole environment as the served environment names them,
	// then narrowed to what a run acts on.
	let names = hosts::host_names(
		&seen
			.iter()
			.map(|member| hosts::Candidate {
				id: member.machine.id,
				name: &member.machine.name,
				address: member.address.as_deref(),
				tailnet: member.tailnet,
				managed: member.managed,
			})
			.collect::<Vec<_>>(),
	)?;

	let mut out: Vec<Member> = seen
		.into_iter()
		.zip(names)
		.filter(|(member, _)| member.managed)
		.map(|(member, host_name)| {
			let mut types = member.types.clone();
			if types.is_empty()
				&& let Some(assigned) = assigned.get(&member.machine.id)
			{
				types.push(assigned.clone());
			}
			Member {
				id: member.machine.id,
				name: member.machine.name.clone(),
				host_name,
				groups: ansible_groups(types.iter().map(String::as_str)),
				application_types: member.types,
			}
		})
		.collect();
	out.sort_by(|a, b| a.name.cmp(&b.name));
	Ok(out)
}

/// Start the run a lease is for, where this is its first read: stamp the lease,
/// declare maintenance over what the run acts on for the holder, and record
/// both. Does nothing where the lease had started already.
// spec: ANSRUN#work-under-way
pub async fn start(
	conn: &mut AuditTx<'_>,
	lease: &AnsibleLease,
	group: &ServerGroup,
) -> Result<()> {
	let Some(started) = AnsibleLease::start(conn, lease.id).await? else {
		return Ok(());
	};
	let note = started
		.playbook
		.clone()
		.unwrap_or_else(|| started.intent.to_string());
	let windows = MaintenanceWindow::declare_for_run(
		conn,
		&started,
		started.limited_to.as_deref(),
		Some(&note),
	)
	.await?;
	let (group_record, lease_record) = super::lease_records(group, &started);
	// Each window the start opened names the lease it was declared for.
	for window in &windows.declared {
		let (record, target) = crate::fns::maintenance::window_records(conn, window).await?;
		conn.record(Action::MaintenanceDeclared {
			window: record,
			target,
			upgrade_plan: None,
			lease: Some(lease_record.clone()),
			rank: window.rank,
			expected_end: window.expected_end,
			note: window.note.clone(),
		})
		.await?;
	}
	conn.record(Action::AnsibleRunStarted {
		group: group_record,
		lease: lease_record,
		rank: started.rank,
		intent: started.intent.to_string(),
		playbook: started.playbook.clone(),
	})
	.await
}

fn assignments_of(args: &[AssignmentArg]) -> BTreeMap<Uuid, String> {
	args.iter()
		.filter_map(|arg| {
			arg.application_type
				.clone()
				.map(|application_type| (arg.machine_id, application_type))
		})
		.collect()
}

fn limit_members(members: &[Member]) -> Vec<limit::Member<'_>> {
	members
		.iter()
		.map(|member| limit::Member {
			id: member.id,
			host_name: &member.host_name,
			groups: &member.groups,
		})
		.collect()
}

/// Check what a run is started with against the environment and the catalogue,
/// refusing what could not run: a playbook the catalogue does not list, tags it
/// does not take, a limit that matches nothing or that Canopy cannot evaluate,
/// and assignments that name a machine with something on it, a type the
/// playbook does not configure, or a second Tamanu central.
// spec: ANSRUN#presentation
pub async fn check(
	conn: &mut AsyncPgConnection,
	environment: &Environment,
	args: &super::TakeLeaseArgs,
) -> Result<Checked> {
	let intent = args.intent;
	let playbook = args.playbook.as_deref();
	let limit = args.limit.as_deref();
	let (tags, skip_tags, assignments) = (&args.tags, &args.skip_tags, &args.assignments);
	let loaded = Loaded::get(conn).await?;
	// A playbook is known, with the intent it runs as and the tags it takes,
	// only from the catalogue, so one is not run before Canopy has read one.
	let chosen = match (playbook, &loaded) {
		(Some(name), Some(loaded)) => {
			Some(loaded.catalogue.playbook(name).cloned().ok_or_else(|| {
				AppError::BadRequest(format!("the catalogue lists no playbook {name:?}"))
			})?)
		}
		(Some(_), None) => {
			return Err(AppError::BadRequest(
				"Canopy has read no catalogue yet, so it knows no playbook to run".into(),
			));
		}
		(None, _) => None,
	};
	// The catalogue says what a playbook is run as, and an upgrade is gated on a
	// plan, so a run cannot be labelled as the other to go around it.
	if let Some(chosen) = &chosen
		&& chosen.intent != intent
	{
		return Err(AppError::BadRequest(format!(
			"{} is run as {}, not as {intent}",
			chosen.name, chosen.intent
		)));
	}
	// The catalogue lists every tag a playbook takes, so one it lists none for
	// takes none: `--tags` there would run only its `always` tasks and configure
	// nothing.
	if let Some(chosen) = &chosen {
		for tag in tags.iter().chain(skip_tags) {
			if !chosen.tags.contains(tag) {
				return Err(AppError::BadRequest(if chosen.tags.is_empty() {
					format!("{} takes no tags", chosen.name)
				} else {
					format!("{tag:?} is not a tag {} takes", chosen.name)
				}));
			}
		}
	}

	let assigned = assignments_of(assignments);
	let members = members(conn, environment, &assigned).await?;

	// A run started for a playbook, or limited, acts on the managed members; with
	// none it would hold the environment shut and act on nothing.
	if (playbook.is_some() || limit.is_some()) && members.is_empty() {
		return Err(AppError::BadRequest(
			"no machine in this environment carries managed_by_ansible, so a run would act on nothing"
				.into(),
		));
	}

	let environment_has_central = environment
		.applications
		.iter()
		.any(|application| application.r#type.to_string() == CENTRAL);
	for (machine, application_type) in &assigned {
		let Some(member) = members.iter().find(|member| member.id == *machine) else {
			return Err(AppError::BadRequest(format!(
				"machine {machine} is not one this run acts on"
			)));
		};
		if !member.application_types.is_empty() {
			return Err(AppError::BadRequest(format!(
				"{:?} already has an application on it, so it is not assigned a type",
				member.name
			)));
		}
		if let Some(chosen) = &chosen
			&& let Some(configures) = &chosen.applications
			&& !configures.contains(application_type)
		{
			return Err(AppError::BadRequest(format!(
				"{} does not configure {application_type}",
				chosen.name
			)));
		}
	}
	let centrals = assigned.values().filter(|t| t.as_str() == CENTRAL).count();
	if centrals > 1 || (centrals == 1 && environment_has_central) {
		return Err(AppError::BadRequest(
			"an environment holds at most one Tamanu central".into(),
		));
	}

	let limited_to = match limit {
		None => None,
		Some(pattern) => {
			let matched = limit::matching(pattern, &limit_members(&members))
				.map_err(|unsupported| AppError::BadRequest(unsupported.0))?;
			if matched.is_empty() {
				return Err(AppError::BadRequest(format!(
					"the limit {pattern:?} matches no machine in this environment"
				)));
			}
			Some(matched)
		}
	};

	Ok(Checked {
		limited_to,
		assignments: assigned,
		members,
	})
}

/// The machines a run on an environment acts on.
///
/// Only the members that carry `managed_by_ansible` are listed, since the
/// plugin builds an inventory of those alone, each named and grouped as a run
/// sees it. Says whether the environment already holds a Tamanu central, of
/// which it holds at most one.
#[utoipa::path(
	post,
	path = "/run_options",
	operation_id = "ansible_run_options",
	tag = "ansible",
	security(("tailscale-user" = [])),
	request_body = EnvironmentArgs,
	responses(
		(status = 200, body = RunOptions),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "Archived, empty, ambiguously named, or two machines would share a host name", body = ProblemDetailsSchema),
	),
)]
pub async fn run_options(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<EnvironmentArgs>,
) -> Result<Json<RunOptions>> {
	let mut conn = state.db.get().await?;
	let environment = resolve_environment(&mut conn, &args).await?;
	let members = members(&mut conn, &environment, &BTreeMap::new()).await?;
	Ok(Json(RunOptions {
		has_central: environment
			.applications
			.iter()
			.any(|application| application.r#type.to_string() == CENTRAL),
		pending_for_secs: database::ansible_leases::PENDING_FOR.as_secs(),
		members: members
			.into_iter()
			.map(|member| RunMember {
				id: member.id,
				name: member.name,
				host_name: member.host_name,
				groups: member.groups.into_iter().collect(),
				application_types: member.application_types,
			})
			.collect(),
	}))
}

/// The machines a limit matches, as typed, so an operator sees what a run would
/// act on before starting it.
///
/// The limit is evaluated as the subset of Ansible host patterns that names
/// hosts and groups: `:` or `,` separated, `*` and `?` globs, `!` to exclude and
/// `&` to intersect. A pattern outside that subset is reported rather than
/// previewed wrongly.
#[utoipa::path(
	post,
	path = "/limit_preview",
	operation_id = "ansible_limit_preview",
	tag = "ansible",
	security(("tailscale-user" = [])),
	request_body = LimitPreviewArgs,
	responses(
		(status = 200, body = LimitPreview),
		(status = 404, description = "No such server group", body = ProblemDetailsSchema),
		(status = 409, description = "Archived, empty, ambiguously named, or two machines would share a host name", body = ProblemDetailsSchema),
	),
)]
pub async fn limit_preview(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<LimitPreviewArgs>,
) -> Result<Json<LimitPreview>> {
	let mut conn = state.db.get().await?;
	let environment = resolve_environment(&mut conn, &args.environment).await?;
	let members = members(&mut conn, &environment, &assignments_of(&args.assignments)).await?;
	let view = limit_members(&members);
	let (groups, hosts) = limit::pickable(&view);

	let (matches, error) = match limit::matching(&args.limit, &view) {
		Ok(ids) => (
			ids.into_iter()
				.filter_map(|id| members.iter().find(|member| member.id == id))
				.map(|member| LimitMatch {
					id: member.id,
					name: member.name.clone(),
					host_name: member.host_name.clone(),
				})
				.collect(),
			None,
		),
		Err(unsupported) => (Vec::new(), Some(unsupported.0)),
	};
	Ok(Json(LimitPreview {
		matches,
		groups,
		hosts,
		error,
	}))
}
