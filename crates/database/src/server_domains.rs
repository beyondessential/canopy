//! Application group domains (DOM): the DNS names each group controls.
//!
//! A claim is exclusive within Canopy — no two claims overlap, so at most one
//! group controls any given name — but says nothing about the wider DNS, where
//! the zone holding it is shared with other groups and with names Canopy doesn't
//! manage at all.
//!
//! The zones Canopy may write in are the Canopy instance's own configuration rather than
//! stored state (see [`commons_types::dns::ManagedZone`]), so a claim is checked
//! against them at claim time and re-matched on read: a claim whose zone has
//! left the configuration stays claimed, and reads report it as unmatched.
// spec: DOM

use commons_errors::{AppError, Result};
use commons_types::dns::{ManagedZone, match_zone, normalize_domain};
use diesel::prelude::*;
use diesel::result::Error as DieselError;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::Serialize;
use uuid::Uuid;

/// Arbitrary constant, stable across releases: claims serialise on this so the
/// no-overlap check can't be raced by a concurrent claim. Unlike the exact
/// duplicate, which the unique index catches, an overlap is a read of
/// neighbouring rows and needs the claim path to be one writer at a time.
const CLAIM_LOCK: i64 = 818_723_002;

/// A domain a server group controls, holding every name beneath it.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::server_group_domains)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ServerGroupDomain {
	/// Unique identifier of this claim.
	pub id: Uuid,
	/// The group that controls the domain.
	pub group_id: Uuid,
	/// The domain, normalised: lower case, no trailing dot.
	pub domain: String,
	/// The operator who claimed it. `None` if not recorded.
	pub created_by: Option<String>,
	/// When it was claimed.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When the claim was last modified.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

/// A claim no configured zone covers, with its group named for reporting.
#[derive(Debug, Clone, Serialize)]
pub struct UnzonedDomain {
	pub group_id: Uuid,
	pub group_name: String,
	pub domain: String,
}

impl ServerGroupDomain {
	/// Claim `domain` for a group.
	///
	/// The name is normalised, then has to sit at or under one of `zones`
	/// (`400` otherwise — Canopy could create no records for it), and must
	/// overlap no existing claim, its own group's included (`409`, naming the
	/// claim in the way).
	pub async fn claim(
		db: &mut AsyncPgConnection,
		group_id: Uuid,
		domain: &str,
		created_by: Option<String>,
		zones: &[ManagedZone],
	) -> Result<Self> {
		use crate::schema::server_group_domains::dsl;
		use diesel_async::AsyncConnection;

		let domain = normalize_domain(domain)?;
		if match_zone(&domain, zones).is_none() {
			return Err(AppError::BadRequest(if zones.is_empty() {
				format!(
					"cannot claim {domain}: Canopy has no managed DNS zones configured, so it \
					 can write no records at all"
				)
			} else {
				format!(
					"cannot claim {domain}: it is not within any of Canopy's managed DNS zones \
					 ({})",
					zones
						.iter()
						.map(|z| z.apex.as_str())
						.collect::<Vec<_>>()
						.join(", ")
				)
			}));
		}

		db.transaction::<_, AppError, _>(async |conn| {
			diesel::sql_query(format!("SELECT pg_advisory_xact_lock({CLAIM_LOCK})"))
				.execute(conn)
				.await?;

			if let Some(clash) = overlapping(conn, &domain).await? {
				return Err(AppError::Conflict(if clash.group_id == group_id {
					format!(
						"{domain} overlaps {} which this group already claims",
						clash.domain
					)
				} else {
					let owner = crate::server_groups::ServerGroup::get_by_id(conn, clash.group_id)
						.await?
						.name;
					format!("{domain} overlaps {}, claimed by {owner}", clash.domain)
				}));
			}

			diesel::insert_into(dsl::server_group_domains)
				.values((
					dsl::group_id.eq(group_id),
					dsl::domain.eq(&domain),
					dsl::created_by.eq(created_by),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await
				.map_err(AppError::from)
		})
		.await
	}

	/// The domains a group claims, longest-held first.
	pub async fn list_for_group(db: &mut AsyncPgConnection, group_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::server_group_domains::dsl;
		dsl::server_group_domains
			.select(Self::as_select())
			.filter(dsl::group_id.eq(group_id))
			.order(dsl::created_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Every live group's claims that no configured zone covers, by domain —
	/// the groups now depending on a domain outside Canopy's reach.
	///
	/// Zone matching is longest-suffix against a list that lives in
	/// configuration rather than in the database, so the filtering happens here
	/// rather than in SQL. The claim table is small (one row per domain a group
	/// controls) and this runs on the monitor's sweep cadence.
	///
	/// Archived groups are left out: their claims are kept, but a group
	/// that has been put away is not something to alert an operator about.
	pub async fn unzoned(
		db: &mut AsyncPgConnection,
		zones: &[ManagedZone],
	) -> Result<Vec<UnzonedDomain>> {
		use crate::schema::{server_group_domains, server_groups};

		let rows: Vec<(Uuid, String, String)> = server_group_domains::table
			.inner_join(server_groups::table)
			.filter(server_groups::deleted_at.is_null())
			.select((
				server_group_domains::group_id,
				server_groups::name,
				server_group_domains::domain,
			))
			.order(server_group_domains::domain.asc())
			.load(db)
			.await
			.map_err(AppError::from)?;

		Ok(rows
			.into_iter()
			.filter(|(_, _, domain)| match_zone(domain, zones).is_none())
			.map(|(group_id, group_name, domain)| UnzonedDomain {
				group_id,
				group_name,
				domain,
			})
			.collect())
	}

	/// Every claim in the fleet, by domain.
	pub async fn list_all(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::server_group_domains::dsl;
		dsl::server_group_domains
			.select(Self::as_select())
			.order(dsl::domain.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Whether any group anywhere controls a domain.
	///
	/// The question behind it is whether this Canopy instance uses name management at
	/// all: a claim can only be made against a configured zone, but one made
	/// before a zone was withdrawn outlives it, so "no zones" alone is not the
	/// same as "not in use".
	// spec: DOM#permission-for-an-application-to-manage-its-own-dns-names
	pub async fn any_claimed(db: &mut AsyncPgConnection) -> Result<bool> {
		use crate::schema::server_group_domains::dsl;
		use diesel::dsl::{exists, select};
		select(exists(dsl::server_group_domains.select(dsl::id)))
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get(db: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::server_group_domains::dsl;
		dsl::server_group_domains
			.select(Self::as_select())
			.filter(dsl::id.eq(id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)?
			.ok_or(AppError::DatabaseQuery(DieselError::NotFound))
	}

	/// Release a claim, ending the group's control of every name beneath it.
	/// `404` if there is no such claim.
	pub async fn release(db: &mut AsyncPgConnection, id: Uuid) -> Result<()> {
		use crate::schema::server_group_domains::dsl;
		let deleted = diesel::delete(dsl::server_group_domains.filter(dsl::id.eq(id)))
			.execute(db)
			.await?;
		if deleted == 0 {
			return Err(AppError::DatabaseQuery(DieselError::NotFound));
		}
		Ok(())
	}

	/// The claim covering `name`, if any: the longest claim `name` sits at or
	/// under. This is what authorises a server to act on a name — the group of
	/// the returned claim is the only group that controls it.
	pub async fn controlling(db: &mut AsyncPgConnection, name: &str) -> Result<Option<Self>> {
		use crate::schema::server_group_domains::dsl;
		let name = normalize_domain(name)?;
		let mut found: Vec<Self> = dsl::server_group_domains
			.select(Self::as_select())
			.filter(dsl::domain.eq_any(ancestors(&name)))
			.load(db)
			.await
			.map_err(AppError::from)?;
		// At most one claim can match, claims being non-overlapping, but sort
		// defensively rather than trusting the invariant with authorisation.
		found.sort_by_key(|row| std::cmp::Reverse(row.domain.len()));
		Ok(found.into_iter().next())
	}
}

/// `name` itself and each of its parents down to the two-label suffix — the set
/// of claims that would cover `name`. Hits the unique index on `domain`, unlike
/// a suffix match.
fn ancestors(name: &str) -> Vec<String> {
	let labels: Vec<&str> = name.split('.').collect();
	(0..labels.len().saturating_sub(1))
		.map(|skip| labels[skip..].join("."))
		.collect()
}

/// The existing claim overlapping `domain`, if any: one at or above it (an
/// exact or ancestor match) or one beneath it (a suffix match). `domain` must be
/// normalised, which is what makes it safe to interpolate into `LIKE` — a
/// normalised name holds only letters, digits, hyphens, and dots, so it carries
/// no pattern metacharacters.
async fn overlapping(
	db: &mut AsyncPgConnection,
	domain: &str,
) -> Result<Option<ServerGroupDomain>> {
	use crate::schema::server_group_domains::dsl;
	dsl::server_group_domains
		.select(ServerGroupDomain::as_select())
		.filter(
			dsl::domain
				.eq_any(ancestors(domain))
				.or(dsl::domain.like(format!("%.{domain}"))),
		)
		.first(db)
		.await
		.optional()
		.map_err(AppError::from)
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn ancestors_stop_at_two_labels() {
		assert_eq!(
			ancestors("a.b.tamanu.app"),
			vec!["a.b.tamanu.app", "b.tamanu.app", "tamanu.app"]
		);
		assert_eq!(ancestors("tamanu.app"), vec!["tamanu.app"]);
	}
}
