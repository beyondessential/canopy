//! The variables a configuration run receives, at one of three scopes: a
//! group, an environment (a group at one rank), or a machine.
//!
//! A secret's value is held in canopy's secret store and never here, so
//! listing what a group carries never reads one.
// spec: INV#inventory-variables

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
#[diesel(table_name = crate::schema::inventory_variables)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct InventoryVariable {
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

/// Where a variable is set. An inventory merges the three name-wise, a
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
}

impl InventoryVariable {
	/// The scope this variable is set at.
	pub fn scope(&self) -> VariableScope {
		match (self.server_group_id, self.rank, self.machine_id) {
			(Some(group_id), None, None) => VariableScope::Group { group_id },
			(Some(group_id), Some(rank), None) => VariableScope::Environment { group_id, rank },
			(None, None, Some(machine_id)) => VariableScope::Machine { machine_id },
			_ => unreachable!("inventory_variables_one_scope admits no other shape"),
		}
	}

	/// Every variable canopy holds, sorted by name.
	pub async fn list_all(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::inventory_variables::dsl;

		dsl::inventory_variables
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The variables at one scope, sorted by name.
	pub async fn list_at(conn: &mut AsyncPgConnection, scope: VariableScope) -> Result<Vec<Self>> {
		use crate::schema::inventory_variables::dsl;

		dsl::inventory_variables
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
		use crate::schema::inventory_variables::dsl;

		dsl::inventory_variables
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
		use crate::schema::inventory_variables::dsl;

		if machine_ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::inventory_variables
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
		use crate::schema::{inventory_variables::dsl, machines};

		let machine_ids: Vec<Uuid> = machines::table
			.filter(machines::group_id.eq(group_id))
			.filter(machines::deleted_at.is_null())
			.select(machines::id)
			.get_results(conn)
			.await
			.map_err(AppError::from)?;

		dsl::inventory_variables
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
		use crate::schema::inventory_variables::dsl;

		let existing: Option<Uuid> = Self::at(conn, scope, name).await?.map(|var| var.id);

		if let Some(id) = existing {
			return diesel::update(dsl::inventory_variables.find(id))
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
		diesel::insert_into(dsl::inventory_variables)
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
		use crate::schema::inventory_variables::dsl;

		diesel::delete(
			dsl::inventory_variables
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
			crate::schema::inventory_variables::table,
			diesel::pg::Pg,
			SqlType = diesel::sql_types::Nullable<diesel::sql_types::Bool>,
		>,
> {
	use crate::schema::inventory_variables::dsl;

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
