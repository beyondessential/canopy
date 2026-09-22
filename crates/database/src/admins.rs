use commons_errors::{AppError, Result};
use diesel::{dsl::count, prelude::*};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable, Insertable, AsChangeset)]
#[diesel(table_name = crate::schema::admins)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Admin {
	pub email: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// Whether this allowlist entry carries the danger permission (see the ADM
	/// spec). Presence in this table confers administrator; this flag confers
	/// danger. The two are independent — administrator does not confer danger.
	pub danger: bool,
}

impl Admin {
	pub async fn check_email(db: &mut AsyncPgConnection, email: &str) -> Result<bool> {
		use crate::schema::admins::dsl;
		dsl::admins
			.select(count(dsl::email))
			.filter(dsl::email.eq(email))
			.first(db)
			.await
			.map_err(AppError::from)
			.map(|count: i64| count > 0)
	}

	/// Whether the allowlist grants this login the danger permission: an entry
	/// exists for it and that entry's `danger` flag is set. Administrator status
	/// (mere presence) is checked separately with [`Self::check_email`].
	pub async fn check_danger(db: &mut AsyncPgConnection, email: &str) -> Result<bool> {
		use crate::schema::admins::dsl;
		dsl::admins
			.select(count(dsl::email))
			.filter(dsl::email.eq(email))
			.filter(dsl::danger.eq(true))
			.first(db)
			.await
			.map_err(AppError::from)
			.map(|count: i64| count > 0)
	}

	/// Set the danger flag on an existing allowlist entry, returning the updated
	/// row. Granting or withdrawing danger amends the operator's entry rather
	/// than adding them to a second list. Withdrawing takes effect at once,
	/// because the permission is resolved afresh per request.
	pub async fn set_danger(db: &mut AsyncPgConnection, email: &str, danger: bool) -> Result<Self> {
		use crate::schema::admins::dsl;
		diesel::update(dsl::admins)
			.filter(dsl::email.eq(email))
			.set(dsl::danger.eq(danger))
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::admins::dsl;
		dsl::admins
			.select(Self::as_select())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Add an admin, returning the row whether it was just created or already
	/// existed.
	///
	/// `DO NOTHING` would insert no row on a conflict, and `get_result` on no
	/// rows is `NotFound` — a 404 out of an endpoint documented as idempotent.
	/// A no-op `DO UPDATE` on the same column returns the existing row instead,
	/// leaving its original `created_at` intact.
	pub async fn add(db: &mut AsyncPgConnection, email: &str) -> Result<Self> {
		use crate::schema::admins::dsl;
		diesel::insert_into(dsl::admins)
			.values(dsl::email.eq(email))
			.on_conflict(dsl::email)
			.do_update()
			.set(dsl::email.eq(email))
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn delete(db: &mut AsyncPgConnection, email: &str) -> Result<()> {
		use crate::schema::admins::dsl;
		diesel::delete(dsl::admins)
			.filter(dsl::email.eq(email))
			.execute(db)
			.await
			.map_err(AppError::from)
			.map(|_| ())
	}
}
