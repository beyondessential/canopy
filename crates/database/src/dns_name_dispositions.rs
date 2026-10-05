//! What happens to a DNS name a machine asks about that cannot be resolved to
//! one of its applications (CRT): the request is recorded for an operator to
//! act on, and the operator either declares the DNS name or denies it.
//!
//! Both are per machine, since an identity is the box's and a request that
//! resolves to no application is not any one application's.
// spec: CRT#undeclared-requests
// spec: CRT#denied-dns-names

use commons_errors::{AppError, Result};
use commons_types::dns::normalize_domain;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use uuid::Uuid;

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

/// What a refused request was for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum AskedFor {
	Addresses,
	Certificate,
}

impl AskedFor {
	fn as_str(self) -> &'static str {
		match self {
			Self::Addresses => "addresses",
			Self::Certificate => "certificate",
		}
	}
}

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
	/// What the latest refused request was for: `addresses` or `certificate`.
	pub asked_for: String,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub first_asked_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub last_asked_at: Timestamp,
}

/// How many undeclared requests one machine has, with what an operator needs to
/// reach it.
#[derive(Debug, Clone, Serialize)]
pub struct UndeclaredOnMachine {
	pub machine_id: Uuid,
	pub machine_name: String,
	pub group_id: Option<Uuid>,
	pub group_name: Option<String>,
	pub count: i64,
}

/// A machine's id and name, and its group's, per undeclared request.
type MachineRow = (Uuid, String, Option<Uuid>, Option<String>);

fn cutoff() -> Timestamp {
	Timestamp::now() - UNDECLARED_LIFETIME
}

impl UndeclaredDnsName {
	/// Record that `machine_id` asked about `dns_name` and could not be resolved.
	///
	/// A repeat ask updates the one record. Records not asked about within
	/// [`UNDECLARED_LIFETIME`] are pruned on the way, so the table holds only
	/// what still counts, and a machine holds at most
	/// [`UNDECLARED_PER_MACHINE`].
	pub async fn record(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		asked_for: AskedFor,
	) -> Result<()> {
		use crate::schema::undeclared_dns_names::dsl;

		let dns_name = normalize_domain(dns_name)?;
		let now = Timestamp::now();

		diesel::delete(
			dsl::undeclared_dns_names
				.filter(dsl::last_asked_at.lt(jiff_diesel::Timestamp::from(cutoff()))),
		)
		.execute(db)
		.await?;

		let held: i64 = dsl::undeclared_dns_names
			.filter(dsl::machine_id.eq(machine_id))
			.count()
			.get_result(db)
			.await?;
		if held >= UNDECLARED_PER_MACHINE {
			// Only a DNS name already recorded is refreshed; a new one is dropped.
			diesel::update(
				dsl::undeclared_dns_names
					.filter(dsl::machine_id.eq(machine_id))
					.filter(dsl::dns_name.eq(&dns_name)),
			)
			.set((
				dsl::asked_for.eq(asked_for.as_str()),
				dsl::last_asked_at.eq(jiff_diesel::Timestamp::from(now)),
			))
			.execute(db)
			.await?;
			return Ok(());
		}

		diesel::insert_into(dsl::undeclared_dns_names)
			.values((
				dsl::machine_id.eq(machine_id),
				dsl::dns_name.eq(&dns_name),
				dsl::asked_for.eq(asked_for.as_str()),
			))
			.on_conflict((dsl::machine_id, dsl::dns_name))
			.do_update()
			.set((
				dsl::asked_for.eq(asked_for.as_str()),
				dsl::last_asked_at.eq(jiff_diesel::Timestamp::from(now)),
			))
			.execute(db)
			.await?;
		Ok(())
	}

	/// Forget the record for `machine_id` and `dns_name`, if there is one.
	pub async fn clear(db: &mut AsyncPgConnection, machine_id: Uuid, dns_name: &str) -> Result<()> {
		use crate::schema::undeclared_dns_names::dsl;
		let dns_name = normalize_domain(dns_name)?;
		diesel::delete(
			dsl::undeclared_dns_names
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::dns_name.eq(dns_name)),
		)
		.execute(db)
		.await?;
		Ok(())
	}

	/// The machine's undeclared requests that still count, by DNS name.
	pub async fn for_machine(db: &mut AsyncPgConnection, machine_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::undeclared_dns_names::dsl;
		dsl::undeclared_dns_names
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::last_asked_at.ge(jiff_diesel::Timestamp::from(cutoff())))
			.order(dsl::dns_name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// How many undeclared requests each live machine has, for the notices.
	///
	/// Only machines with at least one that still counts appear. `group_id`
	/// narrows to one group's machines.
	// spec: CRT#presentation
	pub async fn counts_by_machine(
		db: &mut AsyncPgConnection,
		group_id: Option<Uuid>,
	) -> Result<Vec<UndeclaredOnMachine>> {
		use crate::schema::{machines, server_groups, undeclared_dns_names};

		let mut query = undeclared_dns_names::table
			.inner_join(machines::table)
			.left_join(server_groups::table.on(machines::group_id.eq(server_groups::id.nullable())))
			.filter(machines::deleted_at.is_null())
			.filter(undeclared_dns_names::last_asked_at.ge(jiff_diesel::Timestamp::from(cutoff())))
			.select((
				machines::id,
				machines::name,
				machines::group_id,
				server_groups::name.nullable(),
			))
			.into_boxed();
		if let Some(group) = group_id {
			query = query.filter(machines::group_id.eq(group));
		}
		let rows: Vec<MachineRow> = query.load(db).await?;

		// One row per request; a box carries few, so counting here is cheaper to
		// read than a grouped query across the join.
		let mut by_machine: Vec<UndeclaredOnMachine> = Vec::new();
		for (machine_id, machine_name, group_id, group_name) in rows {
			match by_machine.iter_mut().find(|m| m.machine_id == machine_id) {
				Some(entry) => entry.count += 1,
				None => by_machine.push(UndeclaredOnMachine {
					machine_id,
					machine_name,
					group_id,
					group_name,
					count: 1,
				}),
			}
		}
		by_machine.sort_by(|a, b| {
			(a.group_name.as_deref(), a.machine_name.as_str())
				.cmp(&(b.group_name.as_deref(), b.machine_name.as_str()))
		});
		Ok(by_machine)
	}
}

/// An operator's decision that a machine is not to be served a DNS name.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::denied_dns_names)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct DeniedDnsName {
	pub id: Uuid,
	pub machine_id: Uuid,
	/// Normalised: lower case, no trailing dot.
	pub dns_name: String,
	/// The operator who denied it.
	pub denied_by: String,
	/// Why, if they said.
	pub note: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String)]
	pub created_at: Timestamp,
}

impl DeniedDnsName {
	/// Deny `dns_name` to `machine_id`, ending its undeclared record.
	///
	/// Refused while one of the machine's applications declares the DNS name,
	/// since the two decisions contradict each other and the declaration has to
	/// be released first. Denying again replaces who denied it and the note.
	pub async fn deny(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
		denied_by: &str,
		note: Option<&str>,
	) -> Result<Self> {
		use crate::schema::denied_dns_names::dsl;

		let dns_name = normalize_domain(dns_name)?;
		let note = note.map(str::trim).filter(|n| !n.is_empty());

		if let Some(declared) = crate::ApplicationName::for_name(db, &dns_name).await? {
			let app =
				crate::applications::Application::get_by_id(db, declared.application_id).await?;
			if app.machine_id == Some(machine_id) && app.deleted_at.is_none() {
				return Err(AppError::Conflict(format!(
					"{dns_name} is declared by {} on this machine; release it there before denying it",
					app.name.clone().unwrap_or_else(|| app.r#type.label())
				)));
			}
		}

		let row = diesel::insert_into(dsl::denied_dns_names)
			.values((
				dsl::machine_id.eq(machine_id),
				dsl::dns_name.eq(&dns_name),
				dsl::denied_by.eq(denied_by),
				dsl::note.eq(note),
			))
			.on_conflict((dsl::machine_id, dsl::dns_name))
			.do_update()
			.set((
				dsl::denied_by.eq(denied_by),
				dsl::note.eq(note),
				dsl::created_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await?;

		UndeclaredDnsName::clear(db, machine_id, &dns_name).await?;
		Ok(row)
	}

	/// Lift a denial. `404` when there is none.
	pub async fn lift(db: &mut AsyncPgConnection, machine_id: Uuid, dns_name: &str) -> Result<()> {
		use crate::schema::denied_dns_names::dsl;
		let dns_name = normalize_domain(dns_name)?;
		let deleted = diesel::delete(
			dsl::denied_dns_names
				.filter(dsl::machine_id.eq(machine_id))
				.filter(dsl::dns_name.eq(&dns_name)),
		)
		.execute(db)
		.await?;
		if deleted == 0 {
			return Err(AppError::NotFound(format!(
				"{dns_name} is not denied to this machine"
			)));
		}
		Ok(())
	}

	/// The denial for `machine_id` and `dns_name`, if there is one.
	pub async fn get(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		dns_name: &str,
	) -> Result<Option<Self>> {
		use crate::schema::denied_dns_names::dsl;
		let dns_name = normalize_domain(dns_name)?;
		dsl::denied_dns_names
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine_id))
			.filter(dsl::dns_name.eq(dns_name))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Every DNS name denied to the machine, by DNS name.
	pub async fn for_machine(db: &mut AsyncPgConnection, machine_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::denied_dns_names::dsl;
		dsl::denied_dns_names
			.select(Self::as_select())
			.filter(dsl::machine_id.eq(machine_id))
			.order(dsl::dns_name.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}
}

/// End both dispositions for a DNS name on the machine an application runs on,
/// because the DNS name has just been declared for that application.
///
/// A declaration is the opposite decision to a denial and the answer an
/// undeclared request was waiting for, so neither survives it. An application
/// with no machine (one hosted by a cluster) has neither to end.
// spec: CRT#denied-dns-names
pub async fn clear_for_declaration(
	db: &mut AsyncPgConnection,
	application_id: Uuid,
	dns_name: &str,
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
			.filter(undeclared_dns_names::dns_name.eq(&dns_name)),
	)
	.execute(db)
	.await?;
	diesel::delete(
		denied_dns_names::table
			.filter(denied_dns_names::machine_id.eq(machine_id))
			.filter(denied_dns_names::dns_name.eq(&dns_name)),
	)
	.execute(db)
	.await?;
	Ok(())
}
