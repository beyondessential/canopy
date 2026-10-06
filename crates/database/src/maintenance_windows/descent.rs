//! The grains a declaration can be retargeted to: whatever contains its
//! starting target, and whatever that target contains.
//!
//! Containment here is the containment suspension reads, so a grain this says
//! covers an issue is one whose window would suspend it: an application sits
//! on its machine, a machine in the environment its applications' rank names,
//! and an environment in its group. A pending machine is in no environment and
//! sits directly in its group, and so does a cluster-hosted application, which
//! has no box for an environment's window to reach.

use std::collections::{HashMap, HashSet};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::applications::Application;
use crate::issues::{Issue, Scope};
use crate::machines::Machine;
use crate::server_groups::{ServerGroup, rank_priority};

/// One target a window can be declared over.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Grain {
	Group { group_id: Uuid },
	Environment { group_id: Uuid, rank: ServerRank },
	Machine { machine_id: Uuid },
	Application { application_id: Uuid },
}

impl Grain {
	/// The grain stored as a window's target columns, where they name one.
	pub fn from_columns(
		application: Option<Uuid>,
		machine: Option<Uuid>,
		group: Option<Uuid>,
		rank: Option<ServerRank>,
	) -> Option<Self> {
		match (application, machine, group, rank) {
			(Some(application_id), ..) => Some(Self::Application { application_id }),
			(None, Some(machine_id), ..) => Some(Self::Machine { machine_id }),
			(None, None, Some(group_id), None) => Some(Self::Group { group_id }),
			(None, None, Some(group_id), Some(rank)) => Some(Self::Environment { group_id, rank }),
			(None, None, None, _) => None,
		}
	}

	/// The scope and environment rank a window over this grain is stored as.
	pub fn scope(self) -> (Scope, Option<ServerRank>) {
		match self {
			Self::Group { group_id } => (Scope::Group(group_id), None),
			Self::Environment { group_id, rank } => (Scope::Group(group_id), Some(rank)),
			Self::Machine { machine_id } => (Scope::Machine(machine_id), None),
			Self::Application { application_id } => (Scope::Application(application_id), None),
		}
	}

	/// The target columns `(application, machine, group, rank)`.
	pub fn columns(self) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>, Option<ServerRank>) {
		match self {
			Self::Group { group_id } => (None, None, Some(group_id), None),
			Self::Environment { group_id, rank } => (None, None, Some(group_id), Some(rank)),
			Self::Machine { machine_id } => (None, Some(machine_id), None, None),
			Self::Application { application_id } => (Some(application_id), None, None, None),
		}
	}

	/// The grain an issue is filed against, where a window can reach it.
	pub fn of_issue(issue: &Issue) -> Option<Self> {
		Self::from_columns(
			issue.application_id,
			issue.machine_id,
			issue.server_group_id,
			None,
		)
	}
}

/// One grain on a line of descent, in the order it is listed.
#[derive(Clone, Debug, Serialize, utoipa::ToSchema)]
pub struct DescentEntry {
	pub grain: Grain,
	/// The grain's own name: a group's, a machine's, an application's, or an
	/// environment's rank.
	pub label: String,
	/// How many of the listed grains contain this one, for indenting.
	pub depth: u8,
}

/// The grains on one target's line of descent, nested group over environment
/// over machine over application.
#[derive(Clone, Debug, Default)]
pub struct Descent {
	pub entries: Vec<DescentEntry>,
	parents: HashMap<Grain, Grain>,
}

impl Descent {
	pub fn includes(&self, grain: Grain) -> bool {
		self.entries.iter().any(|entry| entry.grain == grain)
	}

	/// Would a window over `grain` cover a check filed against `target`? It
	/// does where `grain` is the target or contains it, which is how
	/// suspension reaches a check.
	// spec: MNT#choosing-what-to-cover
	pub fn covers(&self, grain: Grain, target: Grain) -> bool {
		let mut at = Some(target);
		while let Some(node) = at {
			if node == grain {
				return true;
			}
			at = self.parents.get(&node).copied();
		}
		false
	}
}

/// The grains `start` can be retargeted to, itself among them: whatever
/// contains it and whatever it contains. A grain it has none of is passed
/// over, so a machine in no group lists its applications alone.
// spec: MNT#choosing-what-to-cover
pub async fn line_of_descent(db: &mut AsyncPgConnection, start: Grain) -> Result<Descent> {
	let group_id = match start {
		Grain::Group { group_id } | Grain::Environment { group_id, .. } => Some(group_id),
		Grain::Machine { machine_id } => Machine::get_by_id(db, machine_id).await?.group_id,
		Grain::Application { application_id } => {
			Application::get_by_id(db, application_id).await?.group_id
		}
	};

	let (group, machines, applications) = match group_id {
		Some(group_id) => {
			let group = ServerGroup::get_by_id(db, group_id).await?;
			let machines = Machine::list_for_group(db, group_id).await?;
			let applications = group.list_servers(db).await?;
			(Some(group), machines, applications)
		}
		None => {
			let machine_id = match start {
				Grain::Machine { machine_id } => Some(machine_id),
				Grain::Application { application_id } => {
					Application::get_by_id(db, application_id).await?.machine_id
				}
				_ => None,
			};
			let machines = match machine_id {
				Some(id) => vec![Machine::get_by_id(db, id).await?],
				None => Vec::new(),
			};
			let applications = match (machine_id, start) {
				(Some(id), _) => applications_on(db, id).await?,
				(None, Grain::Application { application_id }) => {
					vec![Application::get_by_id(db, application_id).await?]
				}
				_ => Vec::new(),
			};
			(None, machines, applications)
		}
	};

	let machine_ids: Vec<Uuid> = machines.iter().map(|machine| machine.id).collect();
	let ranks = Machine::ranks(db, &machine_ids).await?;

	// The whole tree, in listing order, with each grain's container.
	let mut tree: Vec<(Grain, String, Option<Grain>)> = Vec::new();
	let group_grain = group
		.as_ref()
		.map(|group| Grain::Group { group_id: group.id });
	if let Some(group) = &group {
		tree.push((
			Grain::Group { group_id: group.id },
			group.name.clone(),
			None,
		));
	}

	let mut environments: Vec<ServerRank> = ranks.values().copied().collect();
	if let Grain::Environment { rank, .. } = start {
		environments.push(rank);
	}
	environments.sort_by_key(|rank| rank_priority(Some(*rank)));
	environments.dedup();

	let mut by_machine: HashMap<Uuid, Vec<&Application>> = HashMap::new();
	let mut boxless: Vec<&Application> = Vec::new();
	for application in &applications {
		match application.machine_id {
			Some(machine) => by_machine.entry(machine).or_default().push(application),
			None => boxless.push(application),
		}
	}
	for list in by_machine.values_mut() {
		list.sort_by_key(|application| application.display_name());
	}

	let push_machine = |tree: &mut Vec<(Grain, String, Option<Grain>)>,
	                    machine: &Machine,
	                    parent: Option<Grain>| {
		let grain = Grain::Machine {
			machine_id: machine.id,
		};
		tree.push((grain, machine.name.clone(), parent));
		for application in by_machine.get(&machine.id).into_iter().flatten() {
			tree.push((
				Grain::Application {
					application_id: application.id,
				},
				application.display_name(),
				Some(grain),
			));
		}
	};

	if let Some(group) = &group {
		for rank in &environments {
			let environment = Grain::Environment {
				group_id: group.id,
				rank: *rank,
			};
			tree.push((environment, rank.to_string(), group_grain));
			for machine in machines.iter().filter(|m| ranks.get(&m.id) == Some(rank)) {
				push_machine(&mut tree, machine, Some(environment));
			}
		}
	}
	// A machine in no environment sits directly in its group, or heads the
	// tree where it has none.
	let in_environment = |machine: &Machine| group.is_some() && ranks.contains_key(&machine.id);
	for machine in machines.iter().filter(|m| !in_environment(m)) {
		push_machine(&mut tree, machine, group_grain);
	}
	boxless.sort_by_key(|application| application.display_name());
	for application in boxless {
		tree.push((
			Grain::Application {
				application_id: application.id,
			},
			application.display_name(),
			group_grain,
		));
	}

	let parents: HashMap<Grain, Grain> = tree
		.iter()
		.filter_map(|(grain, _, parent)| parent.map(|parent| (*grain, parent)))
		.collect();
	if !tree.iter().any(|(grain, ..)| *grain == start) {
		return Err(AppError::NotFound(
			"the target is not part of what it belongs to".into(),
		));
	}

	let mut listed: HashSet<Grain> = HashSet::new();
	let mut at = Some(start);
	while let Some(node) = at {
		listed.insert(node);
		at = parents.get(&node).copied();
	}
	for (grain, ..) in &tree {
		let mut at = Some(*grain);
		while let Some(node) = at {
			if node == start {
				listed.insert(*grain);
				break;
			}
			at = parents.get(&node).copied();
		}
	}

	let entries = tree
		.into_iter()
		.filter(|(grain, ..)| listed.contains(grain))
		.map(|(grain, label, _)| {
			let mut depth = 0;
			let mut at = parents.get(&grain).copied();
			while let Some(node) = at {
				if listed.contains(&node) {
					depth += 1;
				}
				at = parents.get(&node).copied();
			}
			DescentEntry {
				grain,
				label,
				depth,
			}
		})
		.collect();

	Ok(Descent { entries, parents })
}

async fn applications_on(db: &mut AsyncPgConnection, machine: Uuid) -> Result<Vec<Application>> {
	use crate::schema::applications::dsl;
	dsl::applications
		.select(Application::as_select())
		.filter(dsl::machine_id.eq(machine))
		.filter(dsl::deleted_at.is_null())
		.load(db)
		.await
		.map_err(AppError::from)
}
