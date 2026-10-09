//! The DNS names an application holds for certificates.
//!
//! A declaration is what a certificate request is resolved against, and what
//! tells Canopy whose a certificate is: renewal and alerting follow it, and
//! stop when it is released. It is not the order or the chain, which are
//! `application_certificates`: a declaration exists before any order and
//! outlives a release's certificates.
// spec: DNS#declared-dns-names

use commons_errors::{AppError, Result};
use commons_types::dns::normalize_domain;
use diesel::prelude::*;
use diesel::result::{DatabaseErrorKind, Error as DieselError};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::Serialize;
use uuid::Uuid;

use crate::dns_names::{DnsNameKind, held_elsewhere, holder, lost_race};

/// A DNS name an application holds for certificates.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::application_certificate_names)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct ApplicationCertificateName {
	pub id: Uuid,
	pub application_id: Uuid,
	/// Normalised: lower case, no trailing dot.
	pub name: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub created_at: Timestamp,
}

impl ApplicationCertificateName {
	/// Declare that `application_id` holds `name` for certificates, as an
	/// operator or as a certificate request resolved to it.
	///
	/// Idempotent for the application already holding it. A name another
	/// application holds, for either kind, is refused, and the refusal *names*
	/// the holder; the device-facing path maps it to the undeclared refusal so
	/// the endpoint is not a directory of what other machines serve. Declaring
	/// ends the machine's undeclared record and denial for this kind.
	// spec: DNS#declared-dns-names
	pub async fn declare(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		name: &str,
	) -> Result<Self> {
		use crate::schema::application_certificate_names::dsl;

		let name = normalize_domain(name)?;
		if let Some(held_by) = holder(db, &name).await?
			&& held_by != application_id
		{
			return Err(held_elsewhere(db, &name, held_by).await);
		}

		let row = match Self::for_name(db, &name).await? {
			Some(existing) => existing,
			None => match diesel::insert_into(dsl::application_certificate_names)
				.values((dsl::application_id.eq(application_id), dsl::name.eq(&name)))
				.returning(Self::as_select())
				.get_result(db)
				.await
			{
				Ok(row) => row,
				// Declared from elsewhere between the read and the insert, or held
				// for addresses by another application.
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
			DnsNameKind::Certificate,
		)
		.await?;
		Ok(row)
	}

	/// End an application's hold on a name for certificates, as an operator.
	///
	/// What is already in place stands: certificates held stay held and
	/// collectable until they expire. What ends is Canopy renewing and alerting
	/// on them, and the name being this application's for this kind.
	// spec: DNS#declared-dns-names
	pub async fn release(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		name: &str,
	) -> Result<()> {
		use crate::schema::application_certificate_names::dsl;

		let name = normalize_domain(name)?;
		let deleted = diesel::delete(
			dsl::application_certificate_names
				.filter(dsl::application_id.eq(application_id))
				.filter(dsl::name.eq(&name)),
		)
		.execute(db)
		.await?;
		if deleted == 0 {
			return Err(AppError::NotFound(format!(
				"{name} is not declared for certificates by this application"
			)));
		}
		Ok(())
	}

	pub async fn for_name(db: &mut AsyncPgConnection, name: &str) -> Result<Option<Self>> {
		use crate::schema::application_certificate_names::dsl;
		let name = normalize_domain(name)?;
		dsl::application_certificate_names
			.select(Self::as_select())
			.filter(dsl::name.eq(name))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// The names an application holds for certificates, by name.
	pub async fn for_application(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
	) -> Result<Vec<Self>> {
		use crate::schema::application_certificate_names::dsl;
		dsl::application_certificate_names
			.select(Self::as_select())
			.filter(dsl::application_id.eq(application_id))
			.order(dsl::name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}
}
