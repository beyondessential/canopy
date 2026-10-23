//! The variables a configuration run receives, at one of three scopes: a
//! group, an environment (a group at one rank), or a machine.
//!
//! A secret's value is held in canopy's secret store and never here, so
//! listing what a group carries never reads one.
// spec: ANSVAR#variables

use crate::audit::{Record, RecordKind};
use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::Serialize;
use serde_json::Value;
use uuid::Uuid;

/// One variable: where it is set, its name, and its value where it is not a
/// secret.
#[derive(Clone, Debug, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::ansible_variables)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct AnsibleVariable {
	/// Unique identifier of this variable.
	pub id: Uuid,
	/// Set for a variable belonging to a group or to one of its environments.
	pub server_group_id: Option<Uuid>,
	/// Set alongside `server_group_id` for a variable belonging to one
	/// environment rather than to the whole group.
	pub rank: Option<ServerRank>,
	/// Set for a variable belonging to one machine.
	pub machine_id: Option<Uuid>,
	/// The variable's name, unique within its scope.
	pub name: String,
	/// The value, and `None` for a secret, whose value is in the secret store.
	#[schema(value_type = Option<serde_json::Value>)]
	pub value: Option<Value>,
	/// Whether the value is a secret. Applies to the whole value.
	pub is_secret: bool,
	/// The login that last set the value.
	pub set_by: Option<String>,
	/// When the name was first set here.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When its value was last replaced.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

/// Where a variable is set. A run merges the three name-wise, a
/// machine's over its environment's over its group's.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(untagged)]
pub enum VariableScope {
	Group { group_id: Uuid },
	Environment { group_id: Uuid, rank: ServerRank },
	Machine { machine_id: Uuid },
}

impl VariableScope {
	/// The Secret this scope's secret values live under.
	///
	/// A kubernetes name is held to a DNS label, so the prefix stays short
	/// enough that the longest of these (an environment at rank `production`)
	/// fits inside 63 characters alongside a uuid.
	pub fn secret_name(self) -> String {
		match self {
			Self::Group { group_id } => format!("inv-vars-g-{group_id}"),
			Self::Environment { group_id, rank } => format!("inv-vars-{rank}-{group_id}"),
			Self::Machine { machine_id } => format!("inv-vars-m-{machine_id}"),
		}
	}

	/// The identifier of the record an entry names a variable here by, the same
	/// before and after it is set and after it is removed, so it is what a
	/// variable's history is found by.
	pub fn variable_record_id(self, name: &str) -> String {
		format!("{}{name}", self.variable_record_prefix())
	}

	/// What every variable record here begins with, so the entries of a whole
	/// scope can be found.
	pub fn variable_record_prefix(self) -> String {
		format!("{}:", self.secret_name())
	}

	/// The record every entry about a variable here names besides the variable:
	/// the group, for a group or one of its environments, or the machine.
	pub fn record_id(self) -> String {
		match self {
			Self::Group { group_id } | Self::Environment { group_id, .. } => group_id.to_string(),
			Self::Machine { machine_id } => machine_id.to_string(),
		}
	}

	/// Hold the row of the group this scope is in for the rest of the
	/// transaction, as an import into the group does, so a variable set beside
	/// an import is decided after it or before it, never between its check that
	/// nothing is set and its writes. A machine in no group holds nothing.
	pub async fn lock_group(self, conn: &mut AsyncPgConnection) -> Result<()> {
		let group = match self {
			Self::Group { group_id } | Self::Environment { group_id, .. } => Some(group_id),
			Self::Machine { machine_id } => crate::schema::machines::table
				.select(crate::schema::machines::group_id)
				.find(machine_id)
				.first::<Option<Uuid>>(conn)
				.await
				.optional()
				.map_err(AppError::from)?
				.flatten(),
		};
		if let Some(group) = group {
			crate::ansible_leases::lock_group(conn, group).await?;
		}
		Ok(())
	}
}

impl AnsibleVariable {
	/// The scope this variable is set at.
	pub fn scope(&self) -> VariableScope {
		match (self.server_group_id, self.rank, self.machine_id) {
			(Some(group_id), None, None) => VariableScope::Group { group_id },
			(Some(group_id), Some(rank), None) => VariableScope::Environment { group_id, rank },
			(None, None, Some(machine_id)) => VariableScope::Machine { machine_id },
			_ => unreachable!("ansible_variables_one_scope admits no other shape"),
		}
	}

	/// The variable, named by where it is set and what it is called, and where
	/// it is set, as the records an entry names, with the rank where the scope
	/// is an environment. A variable has the same name before and after it is
	/// set, so one that is not there yet can be named ahead of the secret store
	/// being written.
	pub async fn records(
		conn: &mut AsyncPgConnection,
		scope: VariableScope,
		name: &str,
	) -> Result<(Record, Record, Option<ServerRank>)> {
		let (scope_record, rank) = match scope {
			VariableScope::Group { group_id } => (Record::group(conn, group_id).await?, None),
			VariableScope::Environment { group_id, rank } => {
				(Record::group(conn, group_id).await?, Some(rank))
			}
			VariableScope::Machine { machine_id } => {
				(Record::machine(conn, machine_id).await?, None)
			}
		};
		let variable = Record::new(RecordKind::Variable, scope.variable_record_id(name), name);
		Ok((variable, scope_record, rank))
	}

	/// When a secret value last changed: a secret variable set, or a value
	/// beginning to be held after a change replaced or removed it. What the
	/// escrow watches for, so every value a secret has had reaches it.
	// spec: ANSVAR#history
	pub async fn latest_secret_change(conn: &mut AsyncPgConnection) -> Result<Option<Timestamp>> {
		#[derive(QueryableByName)]
		struct Latest {
			#[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
			at: Option<jiff_diesel::Timestamp>,
		}
		let latest: Latest = diesel::sql_query(
			"SELECT GREATEST(
				(SELECT max(updated_at) FROM ansible_variables WHERE is_secret),
				(SELECT max(created_at) FROM ansible_secret_values_held)
			) AS at",
		)
		.get_result(conn)
		.await
		.map_err(AppError::from)?;
		Ok(latest.at.map(Into::into))
	}

	/// Every variable canopy holds, sorted by name.
	pub async fn list_all(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::ansible_variables::dsl;

		dsl::ansible_variables
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The variables held as plain values whose names are among `names`, for a
	/// caller moving them somewhere else.
	pub async fn list_plain_named(
		conn: &mut AsyncPgConnection,
		names: &[String],
	) -> Result<Vec<Self>> {
		use crate::schema::ansible_variables::dsl;

		if names.is_empty() {
			return Ok(Vec::new());
		}
		dsl::ansible_variables
			.filter(dsl::is_secret.eq(false))
			.filter(dsl::value.is_not_null())
			.filter(dsl::name.eq_any(names))
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The variables at one scope, sorted by name.
	pub async fn list_at(conn: &mut AsyncPgConnection, scope: VariableScope) -> Result<Vec<Self>> {
		use crate::schema::ansible_variables::dsl;

		dsl::ansible_variables
			.filter(at_scope(scope))
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The variable of that name at one scope, where there is one.
	pub async fn at(
		conn: &mut AsyncPgConnection,
		scope: VariableScope,
		name: &str,
	) -> Result<Option<Self>> {
		use crate::schema::ansible_variables::dsl;

		dsl::ansible_variables
			.filter(dsl::name.eq(name))
			.filter(at_scope(scope))
			.select(Self::as_select())
			.first(conn)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The variables on the given machines, sorted by machine then name.
	pub async fn list_for_machines(
		conn: &mut AsyncPgConnection,
		machine_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::ansible_variables::dsl;

		if machine_ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::ansible_variables
			.filter(dsl::machine_id.eq_any(machine_ids))
			.order((dsl::machine_id.asc(), dsl::name.asc()))
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// Everything set under a group: its own, its environments', and those of
	/// the machines in it. Backs the group page.
	pub async fn list_under_group(
		conn: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::{ansible_variables::dsl, machines};

		let machine_ids: Vec<Uuid> = machines::table
			.filter(machines::group_id.eq(group_id))
			.filter(machines::deleted_at.is_null())
			.select(machines::id)
			.get_results(conn)
			.await
			.map_err(AppError::from)?;

		dsl::ansible_variables
			.filter(
				dsl::server_group_id
					.eq(group_id)
					.or(dsl::machine_id.eq_any(machine_ids)),
			)
			.order((dsl::name.asc(), dsl::rank.asc()))
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// Set or replace a variable. `value` is `None` for a secret, whose value
	/// the caller writes to the secret store.
	pub async fn set(
		conn: &mut AsyncPgConnection,
		scope: VariableScope,
		name: &str,
		value: Option<&Value>,
		set_by: Option<&str>,
	) -> Result<Self> {
		use crate::schema::ansible_variables::dsl;

		let existing: Option<Uuid> = Self::at(conn, scope, name).await?.map(|var| var.id);

		if let Some(id) = existing {
			return diesel::update(dsl::ansible_variables.find(id))
				.set((
					dsl::value.eq(value),
					dsl::is_secret.eq(value.is_none()),
					dsl::set_by.eq(set_by),
					dsl::updated_at.eq(diesel::dsl::now),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await
				.map_err(AppError::from);
		}

		let (group_id, rank, machine_id) = columns(scope);
		diesel::insert_into(dsl::ansible_variables)
			.values((
				dsl::server_group_id.eq(group_id),
				dsl::rank.eq(rank),
				dsl::machine_id.eq(machine_id),
				dsl::name.eq(name),
				dsl::value.eq(value),
				dsl::is_secret.eq(value.is_none()),
				dsl::set_by.eq(set_by),
			))
			.returning(Self::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)
	}

	/// Forget a variable. Answers with the row there was, so a caller knows
	/// whether a value in the secret store went with it.
	pub async fn remove(
		conn: &mut AsyncPgConnection,
		scope: VariableScope,
		name: &str,
	) -> Result<Option<Self>> {
		use crate::schema::ansible_variables::dsl;

		diesel::delete(
			dsl::ansible_variables
				.filter(dsl::name.eq(name))
				.filter(at_scope(scope)),
		)
		.returning(Self::as_select())
		.get_result(conn)
		.await
		.optional()
		.map_err(AppError::from)
	}
}

/// The rows at one scope. Each arm is spelled as equality and `IS NULL` so the
/// migration's partial unique indexes serve the lookup, which one predicate
/// over `IS NOT DISTINCT FROM` would leave to a sequential scan.
fn at_scope(
	scope: VariableScope,
) -> Box<
	dyn diesel::BoxableExpression<
			crate::schema::ansible_variables::table,
			diesel::pg::Pg,
			SqlType = diesel::sql_types::Nullable<diesel::sql_types::Bool>,
		>,
> {
	use crate::schema::ansible_variables::dsl;

	match scope {
		VariableScope::Group { group_id } => Box::new(
			dsl::server_group_id
				.eq(group_id)
				.and(dsl::rank.is_null())
				.and(dsl::machine_id.is_null()),
		),
		VariableScope::Environment { group_id, rank } => Box::new(
			dsl::server_group_id
				.eq(group_id)
				.and(dsl::rank.eq(rank.to_string()))
				.and(dsl::machine_id.is_null()),
		),
		VariableScope::Machine { machine_id } => Box::new(dsl::machine_id.eq(machine_id)),
	}
}

fn columns(scope: VariableScope) -> (Option<Uuid>, Option<String>, Option<Uuid>) {
	match scope {
		VariableScope::Group { group_id } => (Some(group_id), None, None),
		VariableScope::Environment { group_id, rank } => {
			(Some(group_id), Some(rank.to_string()), None)
		}
		VariableScope::Machine { machine_id } => (None, None, Some(machine_id)),
	}
}

/// How long a secret's earlier value is held after the entry that replaced or
/// removed it, so the change can be undone.
// spec: ANSVAR#history
pub const HELD_FOR: jiff::SignedDuration = jiff::SignedDuration::from_hours(7 * 24);

impl VariableScope {
	/// The Secret this scope's earlier secret values are held under, each keyed
	/// by the entry that replaced or removed it.
	pub fn held_secret_name(self) -> String {
		format!("{}-held", self.secret_name())
	}
}

/// A secret's earlier value held for a while: which entry replaced or removed
/// it, and where. The value is in the scope's held Secret, keyed by the entry.
#[derive(Clone, Debug, Queryable, Selectable)]
#[diesel(table_name = crate::schema::ansible_secret_values_held)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct HeldSecretValue {
	/// The entry that replaced or removed it.
	pub entry_id: Uuid,
	server_group_id: Option<Uuid>,
	rank: Option<ServerRank>,
	machine_id: Option<Uuid>,
	/// The variable's name.
	pub name: String,
	/// When it is forgotten.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub held_until: Timestamp,
	/// When it began to be held.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
}

impl HeldSecretValue {
	pub fn scope(&self) -> VariableScope {
		match (self.server_group_id, self.rank, self.machine_id) {
			(Some(group_id), None, None) => VariableScope::Group { group_id },
			(Some(group_id), Some(rank), None) => VariableScope::Environment { group_id, rank },
			(None, None, Some(machine_id)) => VariableScope::Machine { machine_id },
			_ => unreachable!("ansible_secret_values_held_one_scope admits no other shape"),
		}
	}

	/// Note a value now held under `entry_id`, until [`HELD_FOR`] from now.
	pub async fn hold(
		conn: &mut AsyncPgConnection,
		entry_id: Uuid,
		scope: VariableScope,
		name: &str,
	) -> Result<Self> {
		use crate::schema::ansible_secret_values_held::dsl;

		let (group_id, rank, machine_id) = columns(scope);
		let until = jiff_diesel::Timestamp::from(Timestamp::now() + HELD_FOR);
		diesel::insert_into(dsl::ansible_secret_values_held)
			.values((
				dsl::entry_id.eq(entry_id),
				dsl::server_group_id.eq(group_id),
				dsl::rank.eq(rank),
				dsl::machine_id.eq(machine_id),
				dsl::name.eq(name),
				dsl::held_until.eq(until),
			))
			.returning(Self::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)
	}

	/// The value held under an entry, while it is still held.
	pub async fn get(conn: &mut AsyncPgConnection, entry_id: Uuid) -> Result<Option<Self>> {
		use crate::schema::ansible_secret_values_held::dsl;

		dsl::ansible_secret_values_held
			.find(entry_id)
			.filter(dsl::held_until.gt(diesel::dsl::now))
			.select(Self::as_select())
			.first(conn)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every value still held, for the escrow.
	pub async fn list_all(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::ansible_secret_values_held::dsl;

		dsl::ansible_secret_values_held
			.order(dsl::created_at.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The values whose hold has run out, to be forgotten.
	pub async fn due(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::ansible_secret_values_held::dsl;

		dsl::ansible_secret_values_held
			.filter(dsl::held_until.le(diesel::dsl::now))
			.order(dsl::held_until.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// Stop noting a value, once it has been forgotten.
	pub async fn release(conn: &mut AsyncPgConnection, entry_id: Uuid) -> Result<()> {
		use crate::schema::ansible_secret_values_held::dsl;

		diesel::delete(dsl::ansible_secret_values_held.find(entry_id))
			.execute(conn)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}
