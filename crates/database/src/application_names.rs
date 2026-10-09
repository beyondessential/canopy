//! DNS names an application should be reachable at, and the addresses Canopy
//! publishes for them (ADR).
//!
//! This is the address side only: the names an application holds for
//! certificates are `application_certificate_names`.
//!
//! A row holds two things: the addresses the server reported, and the addresses
//! Canopy has actually published. Keeping them apart is what lets the reconcile
//! tell whether the zone already matches the intent, and what lets Canopy
//! confine itself to records it put there — in a shared zone, a record Canopy
//! did not publish is none of its business.
// spec: ADR#registering

use std::net::IpAddr;

use commons_errors::{AppError, Result};
use commons_types::dns::normalize_domain;
use diesel::prelude::*;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use ipnet::IpNet;
use jiff::Timestamp;
use serde::Serialize;
use uuid::Uuid;

use crate::dns_names::{DnsNameKind, held_elsewhere, holder, lost_race};

/// A name one server has registered, with what Canopy has published for it.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::application_names)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ApplicationName {
	pub id: Uuid,
	/// The server that registered the name.
	pub application_id: Uuid,
	/// The name, normalised: lower case, no trailing dot.
	pub name: String,
	/// The addresses the server reported it is reachable at. Empty means the
	/// server has withdrawn the name; Canopy removes the records it published
	/// and then forgets the registration, freeing the name.
	#[schema(value_type = Vec<String>)]
	pub addresses: Vec<Option<IpNet>>,
	/// The addresses Canopy has actually published. Differs from `addresses`
	/// while a change is waiting to be reconciled.
	#[schema(value_type = Vec<String>)]
	pub published_addresses: Vec<Option<IpNet>>,
	/// When Canopy last published this name's records.
	#[serde(skip_serializing_if = "Option::is_none")]
	#[diesel(
		deserialize_as = jiff_diesel::NullableTimestamp,
		serialize_as = jiff_diesel::NullableTimestamp,
		treat_none_as_default_value = false
	)]
	pub published_at: Option<Timestamp>,
	/// Why the last publish attempt failed, if it did. Cleared on success.
	pub last_error: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

impl ApplicationName {
	/// The addresses the server asked for, as plain addresses.
	pub fn wanted(&self) -> Vec<IpAddr> {
		to_addrs(&self.addresses)
	}

	/// The addresses Canopy has published, as plain addresses.
	pub fn published(&self) -> Vec<IpAddr> {
		to_addrs(&self.published_addresses)
	}

	/// Whether the published state has caught up with what was asked for.
	pub fn is_reconciled(&self) -> bool {
		let mut wanted = self.wanted();
		let mut published = self.published();
		wanted.sort_unstable();
		published.sort_unstable();
		wanted == published
	}

	/// Whether this registration is being withdrawn: nothing wanted, and once
	/// nothing is published either the row goes.
	pub fn is_withdrawing(&self) -> bool {
		self.addresses.is_empty()
	}

	/// Declare that `application_id` holds `name` for addresses, as an operator.
	///
	/// A declaration ties a name to the software answering on it, with no
	/// addresses yet: it is what a later address registration from the machine
	/// is resolved against, which is how a box running several workloads gets its
	/// requests routed to the right one.
	///
	/// Declaring is idempotent for the application already holding the name.
	/// A name another application holds, for either kind, is refused, and the
	/// refusal *names* the holder — safe here, and not on the device-facing path,
	/// because an operator already sees the whole fleet and needs to know what to
	/// release first. Declaring ends the machine's undeclared record and denial
	/// for addresses.
	// spec: DNS#declared-dns-names
	pub async fn declare(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		name: &str,
	) -> Result<Self> {
		use crate::schema::application_names::dsl;

		let name = normalize_domain(name)?;
		if let Some(held_by) = holder(db, &name).await?
			&& held_by != application_id
		{
			return Err(held_elsewhere(db, &name, held_by).await);
		}

		let row = match Self::for_name(db, &name).await? {
			Some(existing) => existing,
			None => match diesel::insert_into(dsl::application_names)
				.values((
					dsl::application_id.eq(application_id),
					dsl::name.eq(&name),
					dsl::addresses.eq(Vec::<Option<IpNet>>::new()),
				))
				.returning(Self::as_select())
				.get_result(db)
				.await
			{
				Ok(row) => row,
				// Declared from elsewhere between the read and the insert, or held
				// for certificates by another application.
				Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
					match Self::for_name(db, &name).await? {
						Some(row) if row.application_id == application_id => row,
						_ => return Err(lost_race(db, &name).await),
					}
				}
				Err(e) => return Err(AppError::from(e)),
			},
		};

		crate::dns_name_dispositions::clear_for_declaration(
			db,
			application_id,
			&name,
			DnsNameKind::Addresses,
		)
		.await?;
		Ok(row)
	}

	/// End an application's hold on a name for addresses, as an operator.
	///
	/// What is already in place stands, exactly as revoking a grant leaves it:
	/// the records published stay published. What ends is Canopy treating the
	/// name as this application's for addresses, which frees it to be declared by
	/// another once it is released for certificates too.
	// spec: DNS#declared-dns-names
	pub async fn release(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		name: &str,
	) -> Result<()> {
		use crate::schema::application_names::dsl;

		let name = normalize_domain(name)?;
		let deleted = diesel::delete(
			dsl::application_names
				.filter(dsl::application_id.eq(application_id))
				.filter(dsl::name.eq(&name)),
		)
		.execute(db)
		.await?;
		if deleted == 0 {
			return Err(AppError::NotFound(format!(
				"{name} is not declared for addresses by this application"
			)));
		}
		Ok(())
	}

	/// Register `name` for a server with the addresses it is reachable at,
	/// replacing whatever addresses were registered before.
	///
	/// An empty address list withdraws the name. Refused as undeclared when
	/// another application holds the name, word for word as a name nobody
	/// declares: a name's addresses are one application's to set at a time, and
	/// this path is device-facing.
	pub async fn register(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		name: &str,
		addresses: &[IpAddr],
	) -> Result<Self> {
		use crate::schema::application_names::dsl;

		let name = normalize_domain(name)?;
		let nets: Vec<Option<IpNet>> = addresses.iter().map(|a| Some(to_net(*a))).collect();

		if let Some(existing) = Self::for_name(db, &name).await? {
			if existing.application_id != application_id {
				return Err(AppError::DnsNameUndeclared(name));
			}
			return diesel::update(dsl::application_names.filter(dsl::id.eq(existing.id)))
				.set((
					dsl::addresses.eq(nets),
					// A fresh intent clears the previous failure: whether it
					// still applies is for the next attempt to say.
					dsl::last_error.eq::<Option<String>>(None),
				))
				.returning(Self::as_select())
				.get_result(db)
				.await
				.map_err(AppError::from);
		}

		match diesel::insert_into(dsl::application_names)
			.values((
				dsl::application_id.eq(application_id),
				dsl::name.eq(&name),
				dsl::addresses.eq(nets),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
		{
			Ok(row) => {
				crate::dns_name_dispositions::clear_for_declaration(
					db,
					application_id,
					&name,
					DnsNameKind::Addresses,
				)
				.await?;
				Ok(row)
			}
			// Another application registered the same name between the read and
			// the insert, or holds it for certificates.
			Err(DieselError::DatabaseError(DatabaseErrorKind::UniqueViolation, _)) => {
				Err(AppError::DnsNameUndeclared(name))
			}
			Err(e) => Err(AppError::from(e)),
		}
	}

	pub async fn for_name(db: &mut AsyncPgConnection, name: &str) -> Result<Option<Self>> {
		use crate::schema::application_names::dsl;
		let name = normalize_domain(name)?;
		dsl::application_names
			.select(Self::as_select())
			.filter(dsl::name.eq(name))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The names a server has registered, by name.
	pub async fn for_server(db: &mut AsyncPgConnection, application_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::application_names::dsl;
		dsl::application_names
			.select(Self::as_select())
			.filter(dsl::application_id.eq(application_id))
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Registrations whose published state doesn't match what was asked for —
	/// the reconcile's work list, oldest change first so nothing starves.
	///
	/// Skips paused applications: while a server is paused Canopy changes no record of
	/// its, though everything already published stays published.
	// spec: DNS#pausing-an-application
	pub async fn needing_publish(db: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Self>> {
		use crate::schema::{application_names, applications};
		let rows: Vec<Self> = application_names::table
			.inner_join(applications::table)
			.filter(applications::deleted_at.is_null())
			.filter(applications::name_management_paused_at.is_null())
			.select(Self::as_select())
			.order(application_names::updated_at.asc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)?;
		// The comparison is order-insensitive, which SQL's array inequality is
		// not, so the final say is here.
		Ok(rows
			.into_iter()
			.filter(|row| !row.is_reconciled())
			.collect())
	}

	/// Registrations whose last publish attempt failed and which still have not
	/// caught up — the per-server alert's work list.
	///
	/// A row that failed and has since published is not here: `record_published`
	/// clears the error, so the query needs no notion of how old a failure is.
	/// Paused applications are excluded for the same reason they are excluded from the
	/// reconcile — Canopy was told to stop changing their records, so nothing being
	/// changed is the intended outcome.
	// spec: ADR#registering
	pub async fn failing_to_publish(db: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::{application_names, applications};
		let rows: Vec<Self> = application_names::table
			.inner_join(applications::table)
			.filter(applications::deleted_at.is_null())
			.filter(applications::name_management_paused_at.is_null())
			.filter(application_names::last_error.is_not_null())
			.select(Self::as_select())
			.order(application_names::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.filter(|row| !row.is_reconciled())
			.collect())
	}

	/// Record that the published records now match `addresses`.
	pub async fn record_published(
		db: &mut AsyncPgConnection,
		id: Uuid,
		addresses: &[IpAddr],
	) -> Result<()> {
		use crate::schema::application_names::dsl;
		let nets: Vec<Option<IpNet>> = addresses.iter().map(|a| Some(to_net(*a))).collect();
		diesel::update(dsl::application_names.filter(dsl::id.eq(id)))
			.set((
				dsl::published_addresses.eq(nets),
				dsl::published_at.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))),
				dsl::last_error.eq::<Option<String>>(None),
			))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Record why publishing failed, leaving the intent in place to retry.
	pub async fn record_publish_error(
		db: &mut AsyncPgConnection,
		id: Uuid,
		error: &str,
	) -> Result<()> {
		use crate::schema::application_names::dsl;
		diesel::update(dsl::application_names.filter(dsl::id.eq(id)))
			.set(dsl::last_error.eq(Some(error)))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Forget a withdrawn registration, once its records are gone from the zone.
	/// This is what frees the name for another server.
	pub async fn forget(db: &mut AsyncPgConnection, id: Uuid) -> Result<()> {
		use crate::schema::application_names::dsl;
		diesel::delete(dsl::application_names.filter(dsl::id.eq(id)))
			.execute(db)
			.await?;
		Ok(())
	}
}

/// A bare address as the single-host network Postgres stores it as.
fn to_net(addr: IpAddr) -> IpNet {
	let prefix = if addr.is_ipv4() { 32 } else { 128 };
	IpNet::new(addr, prefix).expect("a host prefix is always valid")
}

fn to_addrs(nets: &[Option<IpNet>]) -> Vec<IpAddr> {
	nets.iter().flatten().map(|net| net.addr()).collect()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn a_host_address_round_trips() {
		let v4: IpAddr = "192.0.2.1".parse().unwrap();
		let v6: IpAddr = "2001:db8::1".parse().unwrap();
		assert_eq!(
			to_addrs(&[Some(to_net(v4)), Some(to_net(v6))]),
			vec![v4, v6]
		);
	}
}
