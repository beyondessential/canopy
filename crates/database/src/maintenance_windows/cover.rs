//! What a window covers: one grain, or several machines of one environment.
//!
//! A [`Grain`] names one target on a line of descent. A window over several
//! machines has no single target, so it is its own kind of cover rather than a
//! grain: every place that matches a window to a target has to say how it
//! reads a set, instead of finding that a set was quietly read as its
//! environment.
// spec: MNT#declaring

use std::collections::HashSet;

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::{
	deserialize::{self, FromSql, FromSqlRow},
	pg::{Pg, PgValue},
	sql_types::{Array, Nullable, Uuid as SqlUuid},
};
use diesel_async::AsyncPgConnection;
use uuid::Uuid;

use super::{Grain, MaintenanceWindow};
use crate::issues::Scope;
use crate::machines::Machine;

/// A window's machine array as read back: Postgres arrays hold nulls as far as
/// diesel can tell, and a window's never does.
#[derive(Clone, Debug, Default, FromSqlRow)]
pub struct MachineIds(Vec<Uuid>);

impl FromSql<Array<Nullable<SqlUuid>>, Pg> for MachineIds {
	fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
		let ids = <Vec<Option<Uuid>> as FromSql<Array<Nullable<SqlUuid>>, Pg>>::from_sql(bytes)?;
		Ok(Self(ids.into_iter().flatten().collect()))
	}
}

impl From<MachineIds> for Vec<Uuid> {
	fn from(ids: MachineIds) -> Self {
		ids.0
	}
}

/// What a window covers: a grain, or several machines of one environment.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Cover {
	Grain(Grain),
	/// At least two machines, in order and without repeats. One machine is a
	/// window over that machine, which is a grain.
	Machines(Vec<Uuid>),
}

impl Cover {
	/// A cover of these machines: a set of several, or the one machine's grain.
	pub fn machines(ids: impl IntoIterator<Item = Uuid>) -> Result<Self> {
		let mut ids: Vec<Uuid> = ids.into_iter().collect();
		ids.sort();
		ids.dedup();
		match ids.as_slice() {
			[] => Err(AppError::BadRequest(
				"a window over machines names at least one machine".into(),
			)),
			[only] => Ok(Self::Grain(Grain::machine(*only))),
			_ => Ok(Self::Machines(ids)),
		}
	}

	/// The machines this names as machines, as against the ones an environment
	/// or a group reaches by containing them.
	pub fn named_machines(&self) -> Vec<Uuid> {
		match self {
			Self::Grain(grain) => match grain.scope() {
				Scope::Machine(id) => vec![id],
				_ => Vec::new(),
			},
			Self::Machines(ids) => ids.clone(),
		}
	}

	/// The scopes whose issues a window over this covers.
	pub fn scopes(&self) -> Vec<Scope> {
		match self {
			Self::Grain(grain) => vec![grain.scope()],
			Self::Machines(ids) => ids.iter().copied().map(Scope::Machine).collect(),
		}
	}
}

impl From<Grain> for Cover {
	fn from(grain: Grain) -> Self {
		Self::Grain(grain)
	}
}

/// The group and environment a set of machines is in, refusing machines that
/// are missing, archived, pending, or not all in one environment.
pub async fn environment_of_machines(
	db: &mut AsyncPgConnection,
	ids: &[Uuid],
) -> Result<(Uuid, ServerRank)> {
	let machines = Machine::get_by_ids(db, ids).await?;
	if machines.len() != ids.iter().collect::<HashSet<_>>().len() {
		return Err(AppError::NotFound(
			"a machine named is not in Canopy".into(),
		));
	}
	let mut environment: Option<(Uuid, ServerRank)> = None;
	for machine in &machines {
		let Some(this) = machine.group_id.zip(machine.environment_rank()) else {
			return Err(AppError::BadRequest(format!(
				"{} is in no environment, so a window cannot name it among an environment's machines",
				machine.name
			)));
		};
		match environment {
			None => environment = Some(this),
			Some(env) if env == this => {}
			Some(_) => {
				return Err(AppError::BadRequest(
					"the machines of a window are all in one environment".into(),
				));
			}
		}
	}
	environment.ok_or_else(|| AppError::BadRequest("a window over machines names a machine".into()))
}

impl MaintenanceWindow {
	/// Every machine this window names, whether alone or among others.
	pub fn named_machines(&self) -> Vec<Uuid> {
		if self.machine_ids.is_empty() {
			self.machine_id.into_iter().collect()
		} else {
			self.machine_ids.clone()
		}
	}

	/// Is this a window over several machines?
	pub fn is_machine_set(&self) -> bool {
		!self.machine_ids.is_empty()
	}

	/// What this window covers now.
	pub fn cover(&self) -> Option<Cover> {
		if self.is_machine_set() {
			let mut ids = self.machine_ids.clone();
			ids.sort();
			Some(Cover::Machines(ids))
		} else {
			self.grain().map(Cover::Grain)
		}
	}

	/// Is this the window of `cover`: over exactly that grain, or naming that
	/// machine or exactly those machines?
	pub fn is_window_of(&self, cover: &Cover) -> bool {
		match cover {
			Cover::Grain(grain) => match grain.scope() {
				Scope::Machine(id) => self.named_machines().contains(&id),
				_ => self.grain() == Some(*grain),
			},
			Cover::Machines(_) => self.cover().as_ref() == Some(cover),
		}
	}

	/// The scopes whose issues this window covers: each machine a set names, or
	/// the one target of any other window.
	pub fn scopes(&self) -> Vec<Scope> {
		match self.cover() {
			Some(cover) => cover.scopes(),
			None => Vec::new(),
		}
	}
}
