//! The lists of logins holding a level on the operator surface.
//!
//! Spec: `.workhorse/specs/private-server/access-levels.md` (id `ADM`).
//!
//! Admin and PM each have a list; all has none, since every tailnet user holds
//! it. A login is stored and looked up lowercased, so a list compares ignoring
//! case as the tailnet policy does.

use commons_errors::{AppError, Result};
use diesel::{
	deserialize::{self, FromSql, FromSqlRow},
	expression::AsExpression,
	pg::{Pg, PgValue},
	prelude::*,
	serialize::{self, IsNull, Output, ToSql},
};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::io::Write;

use crate::schema::sql_types::AccessLevel as AccessLevelSql;

/// A level on the operator surface. The levels are a ladder: each includes
/// everything the levels below it may do, so they order from all up to admin.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	utoipa::ToSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum AccessLevel {
	/// Every tailnet user: reads the operator surface.
	All,
	/// Project manager: also edits groups' project details.
	Pm,
	/// Administrator: everything on the operator surface.
	Admin,
}

/// A level that has a list, which is every level above all.
#[derive(
	Clone,
	Copy,
	Debug,
	PartialEq,
	Eq,
	PartialOrd,
	Ord,
	Hash,
	Serialize,
	Deserialize,
	AsExpression,
	FromSqlRow,
	utoipa::ToSchema,
)]
#[diesel(sql_type = AccessLevelSql)]
#[serde(rename_all = "snake_case")]
pub enum ListLevel {
	Pm,
	Admin,
}

impl ListLevel {
	/// Every level with a list, highest first.
	pub const ALL: [Self; 2] = [Self::Admin, Self::Pm];

	pub fn as_str(self) -> &'static str {
		match self {
			Self::Admin => "admin",
			Self::Pm => "pm",
		}
	}
}

impl From<ListLevel> for AccessLevel {
	fn from(level: ListLevel) -> Self {
		match level {
			ListLevel::Pm => Self::Pm,
			ListLevel::Admin => Self::Admin,
		}
	}
}

impl ToSql<AccessLevelSql, Pg> for ListLevel {
	fn to_sql<'b>(&'b self, out: &mut Output<'b, '_, Pg>) -> serialize::Result {
		out.write_all(self.as_str().as_bytes())?;
		Ok(IsNull::No)
	}
}

impl FromSql<AccessLevelSql, Pg> for ListLevel {
	fn from_sql(bytes: PgValue<'_>) -> deserialize::Result<Self> {
		match bytes.as_bytes() {
			b"admin" => Ok(Self::Admin),
			b"pm" => Ok(Self::Pm),
			other => {
				Err(format!("unknown access level {:?}", String::from_utf8_lossy(other)).into())
			}
		}
	}
}

/// A login as lists hold it: without surrounding space, lowercased.
pub fn normalise_login(login: &str) -> String {
	login.trim().to_lowercase()
}

/// One login on one level's list.
#[derive(Debug, Clone, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::access_list_entries)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct AccessListEntry {
	pub level: ListLevel,
	pub login: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
}

impl AccessListEntry {
	/// Every entry on every list, by level then login.
	pub async fn list(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::access_list_entries::dsl;
		dsl::access_list_entries
			.select(Self::as_select())
			.order((dsl::level, dsl::login))
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The highest level whose list carries the login, ignoring its case.
	pub async fn highest_for(db: &mut AsyncPgConnection, login: &str) -> Result<Option<ListLevel>> {
		use crate::schema::access_list_entries::dsl;
		let levels: Vec<ListLevel> = dsl::access_list_entries
			.select(dsl::level)
			.filter(dsl::login.eq(normalise_login(login)))
			.load(db)
			.await?;
		Ok(levels.into_iter().max())
	}

	/// Put a login on a level's list. Returns the new entry, or `None` where
	/// the login was already on it, which leaves the entry as it was.
	pub async fn add(
		db: &mut AsyncPgConnection,
		level: ListLevel,
		login: &str,
	) -> Result<Option<Self>> {
		use crate::schema::access_list_entries::dsl;
		let login = normalise_login(login);
		if login.is_empty() {
			return Err(AppError::BadRequest("a login is required".into()));
		}
		// A login no identity could carry would sit on the list conferring
		// nothing, and read as though someone held the level.
		match login.split_once('@') {
			Some((local, domain)) if !local.is_empty() && !domain.is_empty() => {}
			_ => {
				return Err(AppError::BadRequest(format!(
					"a login must be a tailnet login or an email address, \
					 with text either side of an @; got {login:?}"
				)));
			}
		}
		diesel::insert_into(dsl::access_list_entries)
			.values((dsl::level.eq(level), dsl::login.eq(login)))
			.on_conflict_do_nothing()
			.returning(Self::as_returning())
			.get_result(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Take a login off a level's list, ignoring its case. Returns whether it
	/// was on it.
	pub async fn remove(db: &mut AsyncPgConnection, level: ListLevel, login: &str) -> Result<bool> {
		use crate::schema::access_list_entries::dsl;
		let removed = diesel::delete(dsl::access_list_entries)
			.filter(dsl::level.eq(level))
			.filter(dsl::login.eq(normalise_login(login)))
			.execute(db)
			.await?;
		Ok(removed > 0)
	}
}
