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
