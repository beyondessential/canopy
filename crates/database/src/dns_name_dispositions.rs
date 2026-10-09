//! What happens to a DNS name a machine asks about that cannot be resolved to
//! one of its applications (DNS): the request is recorded for an operator to
//! act on, and the operator either declares the DNS name or denies it.
//!
//! Both are per machine, since an identity is the box's and a request that
//! resolves to no application is not any one application's, and both are per
//! kind: addresses and certificates are separate features, so a machine asking
//! about one DNS name both ways has two records, and denying one leaves the
//! other allowed.
// spec: DNS#undeclared-requests
// spec: DNS#denied-dns-names

use commons_errors::{AppError, Result};
use commons_types::dns::normalize_domain;
use diesel::{prelude::*, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use uuid::Uuid;

use crate::dns_names::DnsNameKind;

/// How long an undeclared request counts without the machine asking again.
///
/// Long enough to outlast any agent's retry interval, short enough that a DNS
/// name the agent has stopped wanting drops off without anyone acting.
pub const UNDECLARED_LIFETIME: SignedDuration = SignedDuration::from_hours(24);

/// The most undeclared requests one machine can have recorded at once.
///
/// A request's DNS name is the machine's own input, and an agent serving a
/// wildcard passes through whatever server name a client offered, so the
/// records are bounded per machine. Past the bound a new DNS name is refused as
/// usual but not recorded, rather than evicting one an operator may be about to
/// act on.
pub const UNDECLARED_PER_MACHINE: i64 = 100;

/// A request a machine made about a DNS name that resolved to no single one of
/// its applications.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::undeclared_dns_names)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct UndeclaredDnsName {
	pub id: Uuid,
	pub machine_id: Uuid,
	/// Normalised: lower case, no trailing dot.
	pub dns_name: String,
	/// Whether the machine asked for addresses or for a certificate.
	#[diesel(deserialize_as = String)]
	pub kind: DnsNameKind,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub first_asked_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub last_asked_at: Timestamp,
}

/// How many undeclared requests one machine has, with what an operator needs to
/// reach it.
#[derive(Debug, Clone, Serialize, QueryableByName)]
pub struct UndeclaredOnMachine {
	#[diesel(sql_type = sql_types::Uuid)]
	pub machine_id: Uuid,
	#[diesel(sql_type = sql_types::Text)]
	pub machine_name: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
	pub group_id: Option<Uuid>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	pub group_name: Option<String>,
	/// How many of its requests are for addresses.
	#[diesel(sql_type = sql_types::BigInt)]
	pub addresses: i64,
	/// How many of its requests are for certificates.
	#[diesel(sql_type = sql_types::BigInt)]
	pub certificates: i64,
}

fn cutoff() -> Timestamp {
	Timestamp::now() - UNDECLARED_LIFETIME
}

impl UndeclaredDnsName {
	/// Record that `machine_id` asked about `dns_name` and could not be resolved.
	///
	/// A repeat ask for the same kind updates the one record, and a machine holds
	/// at most [`UNDECLARED_PER_MACHINE`] that still count, the kinds counted
	/// separately as records. The machine's row is locked
	/// for the duration, so two refusals arriving together cannot both pass the
	/// bound.
	pub async fn record(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		kind: DnsNameKind,
	) -> Result<()> {
		use crate::schema::{machines, undeclared_dns_names::dsl};
		use diesel_async::AsyncConnection;

		let dns_name = normalize_domain(dns_name)?;
		let now = jiff_diesel::Timestamp::from(Timestamp::now());
		let cutoff = jiff_diesel::Timestamp::from(cutoff());

		db.transaction::<_, AppError, _>(async |conn| {
			machines::table
				.filter(machines::id.eq(machine_id))
				.select(machines::id)
				.for_update()
				.first::<Uuid>(conn)
				.await?;

			// This machine's lapsed records go first, so one asked about again
			// after lapsing starts over rather than carrying its old first ask.
			// The rest of the table is pruned by the monitor's sweep.
			diesel::delete(
				dsl::undeclared_dns_names
					.filter(dsl::machine_id.eq(machine_id))
					.filter(dsl::last_asked_at.lt(cutoff)),
			)
			.execute(conn)
			.await?;

			let held: i64 = dsl::undeclared_dns_names
				.filter(dsl::machine_id.eq(machine_id))
				.count()
				.get_result(conn)
				.await?;
			if held >= UNDECLARED_PER_MACHINE {
				// Only a DNS name already recorded is refreshed; a new one is dropped.
				diesel::update(
					dsl::undeclared_dns_names
						.filter(dsl::machine_id.eq(machine_id))
						.filter(dsl::dns_name.eq(&dns_name))
						.filter(dsl::kind.eq(kind.as_str())),
				)
				.set(dsl::last_asked_at.eq(now))
				.execute(conn)
				.await?;
				return Ok(());
			}

			diesel::insert_into(dsl::undeclared_dns_names)
				.values((
					dsl::machine_id.eq(machine_id),
					dsl::dns_name.eq(&dns_name),
					dsl::kind.eq(kind.as_str()),
				))
				.on_conflict((dsl::machine_id, dsl::dns_name, dsl::kind))
				.do_update()
				.set(dsl::last_asked_at.eq(now))
				.execute(conn)
				.await?;
			Ok(())
		})
		.await
	}

	/// Drop every record that no longer counts, for the monitor's sweep.
	///
	/// Reads already pass over them, so this only keeps the table to what
	/// still counts.
	// spec: DNS#undeclared-requests
	pub async fn prune(db: &mut AsyncPgConnection) -> Result<usize> {
		use crate::schema::undeclared_dns_names::dsl;
		diesel::delete(
			dsl::undeclared_dns_names
				.filter(dsl::last_asked_at.lt(jiff_diesel::Timestamp::from(cutoff()))),
		)
		.execute(db)
		.await
		.map_err(AppError::from)
	}

	/// Forget the record of `kind` for `machine_id` and `dns_name`, if there is
	/// one. The other kind's record, if any, stands.
	pub async fn clear(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		kind: DnsNameKind,
	) -> Result<()> {
		use crate::schema::undeclared_dns_names::dsl;
		let dns_name = normalize_domain(dns_name)?;
		diesel::delete(
			dsl::undeclared_dns_names
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::dns_name.eq(dns_name))
				.filter(dsl::kind.eq(kind.as_str())),
		)
		.execute(db)
		.await?;
		Ok(())
	}

	/// The machine's undeclared requests of `kind` that still count, by DNS name.
	pub async fn for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		kind: DnsNameKind,
	) -> Result<Vec<Self>> {
		use crate::schema::undeclared_dns_names::dsl;
		dsl::undeclared_dns_names
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::kind.eq(kind.as_str()))
			.filter(dsl::last_asked_at.ge(jiff_diesel::Timestamp::from(cutoff())))
			.order(dsl::dns_name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// How many undeclared requests each live machine has, of each kind, for the
	/// notices.
	///
	/// Only machines with at least one that still counts appear. `group_id`
	/// narrows to one group's machines.
	// spec: DNS#presentation
	pub async fn counts_by_machine(
		db: &mut AsyncPgConnection,
		group_id: Option<Uuid>,
	) -> Result<Vec<UndeclaredOnMachine>> {
		// Grouped by primary keys, so the machine's and group's other columns
		// come along without being grouped on.
		diesel::sql_query(
			"SELECT m.id AS machine_id, m.name AS machine_name, m.group_id, \
			        g.name AS group_name, \
				        count(*) FILTER (WHERE u.kind = 'addresses') AS addresses, \
				        count(*) FILTER (WHERE u.kind = 'certificate') AS certificates \
			 FROM undeclared_dns_names u \
			 JOIN machines m ON m.id = u.machine_id \
			 LEFT JOIN server_groups g ON g.id = m.group_id \
			 WHERE m.deleted_at IS NULL \
			   AND u.last_asked_at >= $1 \
			   AND ($2::uuid IS NULL OR m.group_id = $2) \
			 GROUP BY m.id, g.id \
			 ORDER BY g.name ASC NULLS FIRST, m.name ASC",
		)
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(cutoff()))
		.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group_id)
		.load(db)
		.await
		.map_err(AppError::from)
	}
}

/// An operator's decision that a machine is not to be served a DNS name for a
/// kind of request.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::denied_dns_names)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct DeniedDnsName {
	pub id: Uuid,
	pub machine_id: Uuid,
	/// Normalised: lower case, no trailing dot.
	pub dns_name: String,
	/// The kind of request denied. The other kind is unaffected.
	#[diesel(deserialize_as = String)]
	pub kind: DnsNameKind,
	/// The operator who denied it.
	pub denied_by: String,
	/// Why, if they said.
	pub note: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub created_at: Timestamp,
}

impl DeniedDnsName {
	/// Deny `dns_name` to `machine_id` for `kind`, ending its undeclared record
	/// of that kind.
	///
	/// Refused while one of the machine's applications declares the DNS name for
	/// that kind, since the two decisions contradict each other and the
	/// declaration has to be released first. The other kind is untouched.
	/// Denying again replaces who denied it and the note.
	pub async fn deny(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		kind: DnsNameKind,
		denied_by: &str,
		note: Option<&str>,
	) -> Result<Self> {
		use crate::schema::denied_dns_names::dsl;

		let dns_name = normalize_domain(dns_name)?;
		let note = note.map(str::trim).filter(|n| !n.is_empty());

		if let Some(declared_by) = declared_for_kind(db, &dns_name, kind).await? {
			let app = crate::applications::Application::get_by_id(db, declared_by).await?;
			if app.machine_id == Some(machine_id) && app.deleted_at.is_none() {
				return Err(AppError::Conflict(format!(
					"{dns_name} is declared for {} by {} on this machine; release it there before denying it",
					kind.noun(),
					app.name.clone().unwrap_or_else(|| app.r#type.label())
				)));
			}
		}

		let row = diesel::insert_into(dsl::denied_dns_names)
			.values((
				dsl::machine_id.eq(machine_id),
				dsl::dns_name.eq(&dns_name),
				dsl::kind.eq(kind.as_str()),
				dsl::denied_by.eq(denied_by),
				dsl::note.eq(note),
			))
			.on_conflict((dsl::machine_id, dsl::dns_name, dsl::kind))
			.do_update()
			.set((
				dsl::denied_by.eq(denied_by),
				dsl::note.eq(note),
				dsl::created_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await?;

		UndeclaredDnsName::clear(db, machine_id, &dns_name, kind).await?;
		Ok(row)
	}

	/// Lift a denial of `kind`. `404` when there is none.
	pub async fn lift(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		kind: DnsNameKind,
	) -> Result<()> {
		use crate::schema::denied_dns_names::dsl;
		let dns_name = normalize_domain(dns_name)?;
		let deleted = diesel::delete(
			dsl::denied_dns_names
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::dns_name.eq(&dns_name))
				.filter(dsl::kind.eq(kind.as_str())),
		)
		.execute(db)
		.await?;
		if deleted == 0 {
			return Err(AppError::NotFound(format!(
				"{dns_name} is not denied to this machine for {}",
				kind.noun()
			)));
		}
		Ok(())
	}

	/// The denial of `kind` for `machine_id` and `dns_name`, if there is one.
	pub async fn get(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		kind: DnsNameKind,
	) -> Result<Option<Self>> {
		use crate::schema::denied_dns_names::dsl;
		let dns_name = normalize_domain(dns_name)?;
		dsl::denied_dns_names
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::dns_name.eq(dns_name))
			.filter(dsl::kind.eq(kind.as_str()))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every DNS name denied to the machine for `kind`, by DNS name.
	pub async fn for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		kind: DnsNameKind,
	) -> Result<Vec<Self>> {
		use crate::schema::denied_dns_names::dsl;
		dsl::denied_dns_names
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::kind.eq(kind.as_str()))
			.order(dsl::dns_name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}
}

/// The application declaring `dns_name` for `kind`, if any.
async fn declared_for_kind(
	db: &mut AsyncPgConnection,
	dns_name: &str,
	kind: DnsNameKind,
) -> Result<Option<Uuid>> {
	Ok(match kind {
		DnsNameKind::Addresses => crate::ApplicationName::for_name(db, dns_name)
			.await?
			.map(|row| row.application_id),
		DnsNameKind::Certificate => crate::ApplicationCertificateName::for_name(db, dns_name)
			.await?
			.map(|row| row.application_id),
	})
}

/// End both dispositions of `kind` for a DNS name on the machine an application
/// runs on, because the DNS name has just been declared for that kind for that
/// application.
///
/// A declaration is the opposite decision to a denial and the answer an
/// undeclared request was waiting for, so neither survives it. Only the kind
/// declared is ended: a denial or record of the other kind stands. An
/// application with no machine (one hosted by a cluster) has neither to end.
// spec: DNS#denied-dns-names
pub async fn clear_for_declaration(
	db: &mut AsyncPgConnection,
	application_id: Uuid,
	dns_name: &str,
	kind: DnsNameKind,
) -> Result<()> {
	use crate::schema::{applications, denied_dns_names, undeclared_dns_names};

	let dns_name = normalize_domain(dns_name)?;
	let machine: Option<Option<Uuid>> = applications::table
		.filter(applications::id.eq(application_id))
		.select(applications::machine_id)
		.first(db)
		.await
		.optional()?;
	let Some(Some(machine_id)) = machine else {
		return Ok(());
	};

	diesel::delete(
		undeclared_dns_names::table
			.filter(undeclared_dns_names::machine_id.eq(machine_id))
			.filter(undeclared_dns_names::dns_name.eq(&dns_name))
			.filter(undeclared_dns_names::kind.eq(kind.as_str())),
	)
	.execute(db)
	.await?;
	diesel::delete(
		denied_dns_names::table
			.filter(denied_dns_names::machine_id.eq(machine_id))
			.filter(denied_dns_names::dns_name.eq(&dns_name))
			.filter(denied_dns_names::kind.eq(kind.as_str())),
	)
	.execute(db)
	.await?;
	Ok(())
}
