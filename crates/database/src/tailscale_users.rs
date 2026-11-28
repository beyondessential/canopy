//! Cached display metadata for Tailscale users (name + profile picture).
//!
//! Handlers that record "this human did X" (issue/incident resolve, notes)
//! upsert here from the request's Tailscale headers so the rest of the API
//! can render avatars without having to round-trip through the Tailscale
//! API.

use commons_errors::{AppError, Result};
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::tailscale_users)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct TailscaleUser {
	pub login: String,
	pub name: String,
	pub profile_pic: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

impl TailscaleUser {
	/// Upsert the user's display info. Called from handlers that record
	/// human actions so the latest name/pic from Tailscale headers is
	/// available to the UI.
	pub async fn upsert(
		db: &mut AsyncPgConnection,
		login: &str,
		name: &str,
		profile_pic: Option<&str>,
	) -> Result<()> {
		use diesel::sql_types::{Nullable, Text};

		// Every authenticated request refreshes these, so one that would write
		// them back unchanged writes nothing.
		diesel::sql_query(
			"INSERT INTO tailscale_users (login, name, profile_pic) VALUES ($1, $2, $3) \
			 ON CONFLICT (login) DO UPDATE \
			 SET name = EXCLUDED.name, profile_pic = EXCLUDED.profile_pic \
			 WHERE tailscale_users.name IS DISTINCT FROM EXCLUDED.name \
			 OR tailscale_users.profile_pic IS DISTINCT FROM EXCLUDED.profile_pic",
		)
		.bind::<Text, _>(login)
		.bind::<Text, _>(name)
		.bind::<Nullable<Text>, _>(profile_pic)
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	/// Bulk lookup keyed by login. Missing logins are simply omitted from
	/// the map. Used by issue/incident endpoints that embed acker/resolver
	/// display info into their responses.
	pub async fn by_logins(
		db: &mut AsyncPgConnection,
		logins: &[&str],
	) -> Result<HashMap<String, Self>> {
		use crate::schema::tailscale_users::dsl;

		if logins.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<Self> = dsl::tailscale_users
			.select(Self::as_select())
			.filter(dsl::login.eq_any(logins))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().map(|u| (u.login.clone(), u)).collect())
	}

	/// Every person Canopy knows display info for, least recently updated
	/// first.
	// spec: GRP#project-details
	pub async fn all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::tailscale_users::dsl;

		dsl::tailscale_users
			.select(Self::as_select())
			.order(dsl::updated_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// The display name Canopy knows for each of `logins`, compared ignoring
	/// case, keyed by the lowercased login. A login Canopy knows none for is
	/// absent. Where several stored logins differ only in case, the most
	/// recently updated one's name is taken.
	// spec: GRP#project-details
	pub async fn names_ignoring_case(
		db: &mut AsyncPgConnection,
		logins: &[&str],
	) -> Result<HashMap<String, String>> {
		use crate::schema::tailscale_users::dsl;

		diesel::define_sql_function!(fn lower(value: diesel::sql_types::Text) -> diesel::sql_types::Text);

		if logins.is_empty() {
			return Ok(HashMap::new());
		}
		let wanted: Vec<String> = logins.iter().map(|login| login.to_lowercase()).collect();
		let rows: Vec<(String, String)> = dsl::tailscale_users
			.select((lower(dsl::login), dsl::name))
			.filter(lower(dsl::login).eq_any(&wanted))
			.order(dsl::updated_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)?;
		// Later rows are newer, so each overwrites an older spelling's.
		Ok(rows.into_iter().collect())
	}
}
