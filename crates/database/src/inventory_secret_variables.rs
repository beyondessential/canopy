//! Which secret variables an environment or an application carries.
//!
//! Only the names and who set them live here. The values are held in canopy's
//! secret store, so listing what an environment carries, and refusing a tag that
//! collides with one, never reads a value.
// spec: INV#secret-variables

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::Serialize;
use uuid::Uuid;

/// A declared secret variable: its name, the scope it is set at, and who last
/// set it.
#[derive(Clone, Debug, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::inventory_secret_variables)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct InventorySecretVariable {
	/// Unique identifier of this declaration.
	pub id: Uuid,
	/// Set with `rank` for a variable belonging to one environment.
	pub server_group_id: Option<Uuid>,
	/// The rank of the environment, set alongside `server_group_id`.
	pub rank: Option<ServerRank>,
	/// Set for a variable belonging to one application.
	pub application_id: Option<Uuid>,
	/// The variable's name, and the key its value is stored under.
	pub name: String,
	/// The login that last set the value.
	pub set_by: Option<String>,
	/// When the name was first set here.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When its value was last replaced.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

/// Where a secret variable is set: one environment, or one application.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(untagged)]
pub enum SecretScope {
	Environment { group_id: Uuid, rank: ServerRank },
	Application { application_id: Uuid },
}

impl SecretScope {
	/// The Secret this scope's values live under.
	pub fn secret_name(self) -> String {
		match self {
			Self::Environment { group_id, rank } => format!("inventory-vars-{group_id}-{rank}"),
			Self::Application { application_id } => {
				format!("inventory-vars-application-{application_id}")
			}
		}
	}
}

impl InventorySecretVariable {
	/// The scope this declaration is at.
	pub fn scope(&self) -> SecretScope {
		match (self.server_group_id, self.rank, self.application_id) {
			(Some(group_id), Some(rank), None) => SecretScope::Environment { group_id, rank },
			(_, _, Some(application_id)) => SecretScope::Application { application_id },
			_ => unreachable!("inventory_secret_variables_one_scope admits no other shape"),
		}
	}

	/// Every declaration Canopy holds, sorted by name.
	pub async fn list_all(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::inventory_secret_variables::dsl;

		dsl::inventory_secret_variables
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The names an environment carries, sorted.
	pub async fn list_for_environment(
		conn: &mut AsyncPgConnection,
		group_id: Uuid,
		rank: ServerRank,
	) -> Result<Vec<Self>> {
		use crate::schema::inventory_secret_variables::dsl;

		dsl::inventory_secret_variables
			.filter(dsl::server_group_id.eq(group_id))
			.filter(dsl::rank.eq(rank.to_string()))
			.filter(dsl::application_id.is_null())
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// The names the given applications carry, sorted by application then name.
	pub async fn list_for_applications(
		conn: &mut AsyncPgConnection,
		application_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::inventory_secret_variables::dsl;

		if application_ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::inventory_secret_variables
			.filter(dsl::application_id.eq_any(application_ids))
			.order((dsl::application_id.asc(), dsl::name.asc()))
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// Every declaration under a group: its environments' and those of the
	/// applications in it. Backs the collision check on a group tag write, where
	/// a name set at any rank is a collision.
	pub async fn list_under_group(
		conn: &mut AsyncPgConnection,
		group_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::{applications, inventory_secret_variables::dsl};

		let application_ids: Vec<Uuid> = applications::table
			.filter(applications::group_id.eq(group_id))
			.select(applications::id)
			.get_results(conn)
			.await
			.map_err(AppError::from)?;

		dsl::inventory_secret_variables
			.filter(
				dsl::server_group_id
					.eq(group_id)
					.or(dsl::application_id.eq_any(application_ids)),
			)
			.order(dsl::name.asc())
			.select(Self::as_select())
			.get_results(conn)
			.await
			.map_err(AppError::from)
	}

	/// Record that `name` is set at `scope`, or refresh who set it. The value
	/// itself is written to the secret store by the caller.
	pub async fn declare(
		conn: &mut AsyncPgConnection,
		scope: SecretScope,
		name: &str,
		set_by: Option<&str>,
	) -> Result<Self> {
		use crate::schema::inventory_secret_variables::dsl;

		let existing: Option<Uuid> = match scope {
			SecretScope::Environment { group_id, rank } => dsl::inventory_secret_variables
				.filter(dsl::name.eq(name))
				.filter(dsl::server_group_id.eq(group_id))
				.filter(dsl::rank.eq(rank.to_string()))
				.filter(dsl::application_id.is_null())
				.select(dsl::id)
				.first(conn)
				.await
				.optional()
				.map_err(AppError::from)?,
			SecretScope::Application { application_id } => dsl::inventory_secret_variables
				.filter(dsl::name.eq(name))
				.filter(dsl::application_id.eq(application_id))
				.select(dsl::id)
				.first(conn)
				.await
				.optional()
				.map_err(AppError::from)?,
		};

		if let Some(id) = existing {
			return diesel::update(dsl::inventory_secret_variables.find(id))
				.set((dsl::set_by.eq(set_by), dsl::updated_at.eq(diesel::dsl::now)))
				.returning(Self::as_select())
				.get_result(conn)
				.await
				.map_err(AppError::from);
		}

		let (group_id, rank, application_id) = match scope {
			SecretScope::Environment { group_id, rank } => {
				(Some(group_id), Some(rank.to_string()), None)
			}
			SecretScope::Application { application_id } => (None, None, Some(application_id)),
		};
		diesel::insert_into(dsl::inventory_secret_variables)
			.values((
				dsl::server_group_id.eq(group_id),
				dsl::rank.eq(rank),
				dsl::application_id.eq(application_id),
				dsl::name.eq(name),
				dsl::set_by.eq(set_by),
			))
			.returning(Self::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)
	}

	/// Forget a declaration. Answers whether there was one.
	pub async fn remove(
		conn: &mut AsyncPgConnection,
		scope: SecretScope,
		name: &str,
	) -> Result<bool> {
		use crate::schema::inventory_secret_variables::dsl;

		let deleted = match scope {
			SecretScope::Environment { group_id, rank } => diesel::delete(
				dsl::inventory_secret_variables
					.filter(dsl::name.eq(name))
					.filter(dsl::server_group_id.eq(group_id))
					.filter(dsl::rank.eq(rank.to_string()))
					.filter(dsl::application_id.is_null()),
			)
			.execute(conn)
			.await
			.map_err(AppError::from)?,
			SecretScope::Application { application_id } => diesel::delete(
				dsl::inventory_secret_variables
					.filter(dsl::name.eq(name))
					.filter(dsl::application_id.eq(application_id)),
			)
			.execute(conn)
			.await
			.map_err(AppError::from)?,
		};
		Ok(deleted > 0)
	}
}

/// A secret variable name a tag may not take, given where the tag is being set.
///
/// An application's tag collides with a secret of that application or of its
/// environment; a group's tag collides with a secret set anywhere under the
/// group, since the group's tags reach every environment in it.
pub async fn colliding_name(
	conn: &mut AsyncPgConnection,
	scope: TagScope,
	keys: impl Iterator<Item = &str>,
) -> Result<Option<String>> {
	let declared: Vec<InventorySecretVariable> = match scope {
		TagScope::Group { group_id } => {
			InventorySecretVariable::list_under_group(conn, group_id).await?
		}
		TagScope::Application {
			application_id,
			group_id,
			rank,
		} => {
			let mut declared =
				InventorySecretVariable::list_for_applications(conn, &[application_id]).await?;
			if let Some(group_id) = group_id {
				declared.extend(
					InventorySecretVariable::list_for_environment(conn, group_id, rank).await?,
				);
			}
			declared
		}
	};

	let names: std::collections::BTreeSet<&str> =
		declared.iter().map(|var| var.name.as_str()).collect();
	Ok(keys
		.filter(|key| names.contains(key))
		.map(str::to_owned)
		.next())
}

/// Where a tag is being written, for the collision check.
#[derive(Clone, Copy, Debug)]
pub enum TagScope {
	Group {
		group_id: Uuid,
	},
	Application {
		application_id: Uuid,
		group_id: Option<Uuid>,
		rank: ServerRank,
	},
}

/// Refuse a tag write that takes a name already set as a secret variable in
/// scope. A name is one or the other: which of the two a run received would
/// otherwise turn on the order the merge happened to take.
pub async fn reject_secret_names(
	conn: &mut AsyncPgConnection,
	scope: TagScope,
	tags: &commons_types::server::TagMap,
) -> Result<()> {
	match colliding_name(conn, scope, tags.0.keys().map(String::as_str)).await? {
		Some(name) => Err(AppError::BadRequest(format!(
			"{name:?} is set as a secret variable here, so it cannot also be a tag"
		))),
		None => Ok(()),
	}
}
