//! Canopy as an OpenID Connect issuer: the relying parties it issues identity
//! tokens for, which machines and applications may obtain them, and what Canopy
//! has observed of its signing keys.
//!
//! Specs: `.workhorse/specs/private-server/identity-provider.md` (id `IDP`) and
//! `.workhorse/specs/public-server/identity-tokens.md` (id `IDT`).
//!
//! Who registered, granted or revoked what, and every token issued, are recorded
//! in the audit log rather than here.

use std::collections::{BTreeMap, HashMap};

use commons_errors::{AppError, Result};
use commons_types::server::rank::ServerRank;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{
	Machine,
	applications::Application,
	audit::{Record, RecordKind},
};

/// How long a key must have been served at the issuer before it signs, so that
/// relying parties' cached key sets include it by then.
// spec: IDP#signing-keys
pub const MATURITY: SignedDuration = SignedDuration::from_mins(30);

/// The bounds of a relying party's token lifetime, in minutes.
// spec: IDP#relying-parties
pub const LIFETIME_MINUTES: std::ops::RangeInclusive<i32> = 1..=60;

/// The token lifetime a relying party is registered with unless told otherwise.
pub const DEFAULT_LIFETIME_MINUTES: i32 = 5;

/// A service that trusts Canopy as an issuer.
#[derive(Debug, Clone, Serialize, Queryable, Selectable, utoipa::ToSchema)]
#[diesel(table_name = crate::schema::relying_parties)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct RelyingParty {
	/// Its identifier.
	pub id: Uuid,
	/// What operators see.
	pub name: String,
	/// What a token carries as `aud`, and what a machine names when it asks for
	/// one. Fixed once registered.
	pub audience: String,
	/// How long the tokens issued for it live, in minutes.
	pub token_lifetime_minutes: i32,
	/// When it was registered.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	#[schema(value_type = String, format = DateTime)]
	pub created_at: Timestamp,
	/// When it was last edited.
	#[schema(value_type = String, format = DateTime)]
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
}

impl RelyingParty {
	/// This relying party as a record an audit entry acted on.
	pub fn record(&self) -> Record {
		Record::new(RecordKind::RelyingParty, self.id, self.name.clone())
	}

	pub fn token_lifetime(&self) -> SignedDuration {
		SignedDuration::from_mins(self.token_lifetime_minutes.into())
	}

	/// Every relying party, by name.
	pub async fn list(conn: &mut AsyncPgConnection) -> Result<Vec<Self>> {
		use crate::schema::relying_parties::dsl;
		dsl::relying_parties
			.select(Self::as_select())
			.order((dsl::name.asc(), dsl::audience.asc()))
			.load(conn)
			.await
			.map_err(AppError::from)
	}

	pub async fn get(conn: &mut AsyncPgConnection, id: Uuid) -> Result<Self> {
		use crate::schema::relying_parties::dsl;
		dsl::relying_parties
			.find(id)
			.select(Self::as_select())
			.first(conn)
			.await
			.map_err(AppError::from)
	}

	/// The relying parties of `ids`, by identifier, failing as not found when
	/// any is not registered.
	pub async fn get_many(
		conn: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<HashMap<Uuid, Self>> {
		use crate::schema::relying_parties::dsl;
		if ids.is_empty() {
			return Ok(HashMap::new());
		}
		let found: HashMap<Uuid, Self> = dsl::relying_parties
			.filter(dsl::id.eq_any(ids))
			.select(Self::as_select())
			.load::<Self>(conn)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.map(|party| (party.id, party))
			.collect();
		if ids.iter().any(|id| !found.contains_key(id)) {
			return Err(AppError::DatabaseQuery(diesel::result::Error::NotFound));
		}
		Ok(found)
	}

	pub async fn by_audience(conn: &mut AsyncPgConnection, audience: &str) -> Result<Option<Self>> {
		use crate::schema::relying_parties::dsl;
		dsl::relying_parties
			.filter(dsl::audience.eq(audience))
			.select(Self::as_select())
			.first(conn)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Register a relying party. Two never share an audience.
	// spec: IDP#relying-parties
	pub async fn add(
		conn: &mut AsyncPgConnection,
		name: &str,
		audience: &str,
		token_lifetime_minutes: Option<i32>,
	) -> Result<Self> {
		use crate::schema::relying_parties::dsl;

		let name = valid_name(name)?;
		let audience = audience.trim();
		if audience.is_empty() {
			return Err(AppError::BadRequest(
				"a relying party needs an audience".into(),
			));
		}
		let lifetime = valid_lifetime(token_lifetime_minutes.unwrap_or(DEFAULT_LIFETIME_MINUTES))?;

		// The constraint decides, so two registering the same audience at once
		// are told apart rather than one failing on the constraint.
		diesel::insert_into(dsl::relying_parties)
			.values((
				dsl::name.eq(name),
				dsl::audience.eq(audience),
				dsl::token_lifetime_minutes.eq(lifetime),
			))
			.on_conflict(dsl::audience)
			.do_nothing()
			.returning(Self::as_select())
			.get_result(conn)
			.await
			.optional()
			.map_err(AppError::from)?
			.ok_or_else(|| {
				AppError::Conflict(format!(
					"a relying party with the audience {audience:?} is already registered"
				))
			})
	}

	/// Change a relying party's name and token lifetime, returning it as it was
	/// and as it is, or `None` when nothing it was given differs.
	// spec: IDP#relying-parties
	pub async fn edit(
		conn: &mut AsyncPgConnection,
		id: Uuid,
		name: &str,
		token_lifetime_minutes: i32,
	) -> Result<Option<(Self, Self)>> {
		use crate::schema::relying_parties::dsl;

		let name = valid_name(name)?;
		let lifetime = valid_lifetime(token_lifetime_minutes)?;
		let before: Self = dsl::relying_parties
			.find(id)
			.select(Self::as_select())
			.for_update()
			.first(conn)
			.await
			.map_err(AppError::from)?;
		if before.name == name && before.token_lifetime_minutes == lifetime {
			return Ok(None);
		}

		let after = diesel::update(dsl::relying_parties.find(id))
			.set((dsl::name.eq(name), dsl::token_lifetime_minutes.eq(lifetime)))
			.returning(Self::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)?;
		Ok(Some((before, after)))
	}

	/// Remove a relying party, returning it and the subjects whose grants for it
	/// the removal revoked.
	// spec: IDP#relying-parties
	pub async fn remove(
		conn: &mut AsyncPgConnection,
		id: Uuid,
	) -> Result<(Self, Vec<GrantSubject>)> {
		use crate::schema::{identity_grants::dsl as grants, relying_parties::dsl};

		let party: Self = dsl::relying_parties
			.find(id)
			.select(Self::as_select())
			.for_update()
			.first(conn)
			.await
			.map_err(AppError::from)?;
		let revoked = grants::identity_grants
			.filter(grants::relying_party_id.eq(id))
			.select((grants::machine_id, grants::application_id))
			.load::<(Option<Uuid>, Option<Uuid>)>(conn)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.filter_map(|(machine, application)| GrantSubject::from_columns(machine, application))
			.collect();
		diesel::delete(dsl::relying_parties.find(id))
			.execute(conn)
			.await
			.map_err(AppError::from)?;
		Ok((party, revoked))
	}

	/// How many machines, and how many applications, hold a grant for each
	/// relying party.
	pub async fn grant_counts(conn: &mut AsyncPgConnection) -> Result<HashMap<Uuid, GrantCounts>> {
		use crate::schema::identity_grants::dsl;
		let rows = dsl::identity_grants
			.group_by(dsl::relying_party_id)
			.select((
				dsl::relying_party_id,
				diesel::dsl::count(dsl::machine_id),
				diesel::dsl::count(dsl::application_id),
			))
			.load::<(Uuid, i64, i64)>(conn)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.map(|(party, machines, applications)| {
				(
					party,
					GrantCounts {
						machines,
						applications,
					},
				)
			})
			.collect())
	}
}

/// How many of each kind of subject hold a grant.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct GrantCounts {
	pub machines: i64,
	pub applications: i64,
}

fn valid_name(name: &str) -> Result<&str> {
	let name = name.trim();
	if name.is_empty() {
		return Err(AppError::BadRequest("a relying party needs a name".into()));
	}
	Ok(name)
}

fn valid_lifetime(minutes: i32) -> Result<i32> {
	if !LIFETIME_MINUTES.contains(&minutes) {
		return Err(AppError::BadRequest(format!(
			"a token lifetime is from {} minute to {} minutes, not {minutes}",
			LIFETIME_MINUTES.start(),
			LIFETIME_MINUTES.end(),
		)));
	}
	Ok(minutes)
}

/// What a grant lets obtain tokens: a machine, for tokens naming itself, or an
/// application, for tokens naming that application. Neither covers the other.
// spec: IDP#grants
#[derive(
	Debug,
	Clone,
	Copy,
	PartialEq,
	Eq,
	Hash,
	PartialOrd,
	Ord,
	Serialize,
	Deserialize,
	utoipa::ToSchema,
)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum GrantSubject {
	/// A machine, for tokens naming itself.
	Machine(Uuid),
	/// An application, for tokens naming it.
	Application(Uuid),
}

impl GrantSubject {
	fn from_columns(machine: Option<Uuid>, application: Option<Uuid>) -> Option<Self> {
		match (machine, application) {
			(Some(machine), None) => Some(Self::Machine(machine)),
			(None, Some(application)) => Some(Self::Application(application)),
			_ => None,
		}
	}

	fn to_columns(self) -> (Option<Uuid>, Option<Uuid>) {
		match self {
			Self::Machine(id) => (Some(id), None),
			Self::Application(id) => (None, Some(id)),
		}
	}

	/// This subject as a record an audit entry acted on, named as it is now.
	pub async fn record(self, conn: &mut AsyncPgConnection) -> Result<Record> {
		match self {
			Self::Machine(id) => Record::machine(conn, id).await,
			Self::Application(id) => Record::application(conn, id).await,
		}
	}

	/// Each of `subjects` as a record, named as it is now, read in one query per
	/// kind of subject.
	pub async fn records(
		conn: &mut AsyncPgConnection,
		subjects: &[Self],
	) -> Result<HashMap<Self, Record>> {
		let (machines, applications): (Vec<Self>, Vec<Self>) = subjects
			.iter()
			.partition(|subject| matches!(subject, Self::Machine(_)));
		let ids = |subjects: Vec<Self>| -> Vec<Uuid> {
			subjects
				.into_iter()
				.map(|subject| match subject {
					Self::Machine(id) | Self::Application(id) => id,
				})
				.collect()
		};

		let mut out = HashMap::with_capacity(subjects.len());
		let machines = ids(machines);
		if !machines.is_empty() {
			for machine in Machine::get_by_ids(conn, &machines).await? {
				out.insert(
					Self::Machine(machine.id),
					Record::new(RecordKind::Machine, machine.id, machine.name),
				);
			}
		}
		let applications = ids(applications);
		if !applications.is_empty() {
			for application in Application::get_by_ids(conn, &applications).await? {
				out.insert(
					Self::Application(application.id),
					Record::new(
						RecordKind::Application,
						application.id,
						application.display_name(),
					),
				);
			}
		}
		if subjects.iter().any(|subject| !out.contains_key(subject)) {
			return Err(AppError::DatabaseQuery(diesel::result::Error::NotFound));
		}
		Ok(out)
	}
}

/// The grants a subject holds and the changes a [`set_grants`] made.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct GrantChanges {
	pub granted: Vec<Uuid>,
	pub revoked: Vec<Uuid>,
}

impl GrantChanges {
	pub fn is_empty(&self) -> bool {
		self.granted.is_empty() && self.revoked.is_empty()
	}
}

/// The relying parties each of `subjects` holds a grant for.
pub async fn grants_of(
	conn: &mut AsyncPgConnection,
	subjects: &[GrantSubject],
) -> Result<BTreeMap<GrantSubject, Vec<Uuid>>> {
	use crate::schema::identity_grants::dsl;

	let machines: Vec<Uuid> = subjects
		.iter()
		.filter_map(|s| match s {
			GrantSubject::Machine(id) => Some(*id),
			_ => None,
		})
		.collect();
	let applications: Vec<Uuid> = subjects
		.iter()
		.filter_map(|s| match s {
			GrantSubject::Application(id) => Some(*id),
			_ => None,
		})
		.collect();

	let rows = dsl::identity_grants
		.filter(
			dsl::machine_id
				.eq_any(machines)
				.or(dsl::application_id.eq_any(applications)),
		)
		.select((dsl::machine_id, dsl::application_id, dsl::relying_party_id))
		.load::<(Option<Uuid>, Option<Uuid>, Uuid)>(conn)
		.await
		.map_err(AppError::from)?;

	let mut out: BTreeMap<GrantSubject, Vec<Uuid>> =
		subjects.iter().map(|s| (*s, Vec::new())).collect();
	for (machine, application, party) in rows {
		if let Some(subject) = GrantSubject::from_columns(machine, application) {
			out.entry(subject).or_default().push(party);
		}
	}
	Ok(out)
}

/// Whether `subject` holds a grant for `relying_party`.
pub async fn is_granted(
	conn: &mut AsyncPgConnection,
	subject: GrantSubject,
	relying_party: Uuid,
) -> Result<bool> {
	use crate::schema::identity_grants::dsl;
	let (machine, application) = subject.to_columns();
	let query = dsl::identity_grants
		.filter(dsl::relying_party_id.eq(relying_party))
		.into_boxed();
	let query = match (machine, application) {
		(Some(machine), _) => query.filter(dsl::machine_id.eq(machine)),
		(_, Some(application)) => query.filter(dsl::application_id.eq(application)),
		_ => unreachable!("a subject has exactly one column"),
	};
	diesel::select(diesel::dsl::exists(query))
		.get_result(conn)
		.await
		.map_err(AppError::from)
}

/// Make `subject`'s grants exactly `relying_parties`, returning what it granted
/// and revoked.
///
/// The subject's row is locked first, so within a transaction two saves of one
/// subject's grants take turns: the second reads what the first left and makes
/// its own set exact. What it returns is what its own statements changed.
// spec: IDP#grants
pub async fn set_grants(
	conn: &mut AsyncPgConnection,
	subject: GrantSubject,
	relying_parties: &[Uuid],
) -> Result<GrantChanges> {
	use crate::schema::identity_grants::dsl;

	match subject {
		GrantSubject::Machine(id) => {
			Machine::get_by_id_for_update(conn, id).await?;
		}
		GrantSubject::Application(id) => {
			Application::get_by_id_for_update(conn, id).await?;
		}
	}
	let held = grants_of(conn, &[subject])
		.await?
		.remove(&subject)
		.unwrap_or_default();
	let mut wanted = relying_parties.to_vec();
	wanted.sort_unstable();
	wanted.dedup();
	let to_revoke: Vec<Uuid> = held
		.iter()
		.filter(|p| !wanted.contains(p))
		.copied()
		.collect();
	let to_grant: Vec<Uuid> = wanted
		.iter()
		.filter(|p| !held.contains(p))
		.copied()
		.collect();

	let (machine, application) = subject.to_columns();
	let mut changes = GrantChanges::default();
	if !to_revoke.is_empty() {
		let query =
			diesel::delete(dsl::identity_grants.filter(dsl::relying_party_id.eq_any(&to_revoke)));
		changes.revoked = match (machine, application) {
			(Some(machine), _) => {
				query
					.filter(dsl::machine_id.eq(machine))
					.returning(dsl::relying_party_id)
					.get_results(conn)
					.await
			}
			(_, Some(application)) => {
				query
					.filter(dsl::application_id.eq(application))
					.returning(dsl::relying_party_id)
					.get_results(conn)
					.await
			}
			_ => unreachable!("a subject has exactly one column"),
		}
		.map_err(AppError::from)?;
	}
	if !to_grant.is_empty() {
		changes.granted = diesel::insert_into(dsl::identity_grants)
			.values(
				to_grant
					.iter()
					.map(|party| {
						(
							dsl::relying_party_id.eq(*party),
							dsl::machine_id.eq(machine),
							dsl::application_id.eq(application),
						)
					})
					.collect::<Vec<_>>(),
			)
			.on_conflict_do_nothing()
			.returning(dsl::relying_party_id)
			.get_results(conn)
			.await
			.map_err(AppError::from)?;
	}
	changes.granted.sort_unstable();
	changes.revoked.sort_unstable();
	Ok(changes)
}

/// Where a subject sits, which its token's `sub` and claims carry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Placement {
	pub group_id: Uuid,
	pub rank: ServerRank,
}

/// Why a subject has no placement.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unplaced {
	/// Its machine is pending: it has no rank.
	Pending,
	/// Its machine belongs to no group.
	Ungrouped,
	/// Both.
	PendingAndUngrouped,
}

impl Unplaced {
	pub fn describe(self) -> &'static str {
		match self {
			Self::Pending => "its machine is pending and has no rank",
			Self::Ungrouped => "its machine belongs to no group",
			Self::PendingAndUngrouped => "its machine is pending and belongs to no group",
		}
	}
}

/// The machine's placement: its group and the environment it serves. An
/// application takes its machine's.
// spec: IDT#subject
pub fn placement(machine: &Machine) -> std::result::Result<Placement, Unplaced> {
	match (machine.group_id, machine.environment_rank()) {
		(Some(group_id), Some(rank)) => Ok(Placement { group_id, rank }),
		(Some(_), None) => Err(Unplaced::Pending),
		(None, Some(_)) => Err(Unplaced::Ungrouped),
		(None, None) => Err(Unplaced::PendingAndUngrouped),
	}
}

/// The structured subject a token for this machine, or for this application on
/// it, carries. `None` when a value falls outside the subject's alphabet or is
/// one of its labels, which Canopy issues no token for.
// spec: IDT#subject
pub fn subject(
	placement: &Placement,
	machine: &Machine,
	application: Option<&Application>,
) -> Option<String> {
	subject_of(
		placement,
		machine.id,
		application.map(|a| (a.r#type.to_string(), a.id)),
	)
}

fn subject_of(
	placement: &Placement,
	machine_id: Uuid,
	application: Option<(String, Uuid)>,
) -> Option<String> {
	let mut segments: Vec<(&str, String)> = vec![
		("group", placement.group_id.to_string()),
		("rank", placement.rank.to_string()),
	];
	match application {
		Some((r#type, id)) => {
			segments.push(("application", r#type));
			segments.push(("", id.to_string()));
		}
		None => segments.push(("machine", machine_id.to_string())),
	}
	if !segments
		.iter()
		.all(|(_, value)| in_alphabet(value) && !LABELS.contains(&value.as_str()))
	{
		return None;
	}

	let mut out = String::new();
	for (label, value) in segments {
		if !label.is_empty() {
			out.push_str(label);
			out.push(':');
		}
		out.push_str(&value);
		out.push(':');
	}
	Some(out)
}

/// The labels a subject's segments carry. No value is one of them, so a label's
/// text in a pattern matches only that label, wherever a wildcard before it
/// reaches.
// spec: IDT#subject
const LABELS: &[&str] = &["group", "rank", "application", "machine"];

/// Lowercase letters, digits and hyphens, so no value can contain the
/// separator.
fn in_alphabet(value: &str) -> bool {
	!value.is_empty()
		&& value
			.bytes()
			.all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

/// What Canopy has observed of a signing key.
#[derive(Debug, Clone, PartialEq, Eq, Queryable, Selectable)]
#[diesel(table_name = crate::schema::identity_keys)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct IdentityKey {
	pub kid: String,
	pub key_ref: String,
	/// When `key_ref` was last read as naming this key. Of the rows sharing a
	/// reference, the latest known is the key it names now.
	#[diesel(deserialize_as = jiff_diesel::Timestamp)]
	pub known_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub first_served_at: Option<Timestamp>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp)]
	pub first_signed_at: Option<Timestamp>,
}

impl IdentityKey {
	/// Every key Canopy has observed, by `kid`.
	pub async fn all(conn: &mut AsyncPgConnection) -> Result<HashMap<String, Self>> {
		use crate::schema::identity_keys::dsl;
		Ok(dsl::identity_keys
			.select(Self::as_select())
			.load::<Self>(conn)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.map(|key| (key.kid.clone(), key))
			.collect())
	}

	/// Of `observed`, the key `key_ref` names now.
	pub fn by_ref<'a>(observed: &'a HashMap<String, Self>, key_ref: &str) -> Option<&'a Self> {
		observed
			.values()
			.filter(|row| row.key_ref == key_ref)
			.max_by_key(|row| row.known_at)
	}

	/// Record that the configured key `key_ref` has the identifier `kid`, so
	/// it is known by it before it is ever served.
	pub async fn know(conn: &mut AsyncPgConnection, kid: &str, key_ref: &str) -> Result<()> {
		use crate::schema::identity_keys::dsl;
		diesel::insert_into(dsl::identity_keys)
			.values((dsl::kid.eq(kid), dsl::key_ref.eq(key_ref)))
			.on_conflict(dsl::kid)
			.do_update()
			.set((dsl::key_ref.eq(key_ref), dsl::known_at.eq(diesel::dsl::now)))
			.execute(conn)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Record that `kid` was observed served at the issuer. The first
	/// observation stands; later ones change nothing.
	// spec: IDP#signing-keys
	pub async fn observe_served(
		conn: &mut AsyncPgConnection,
		kid: &str,
		key_ref: &str,
		at: Timestamp,
	) -> Result<()> {
		use crate::schema::identity_keys::dsl;

		diesel::insert_into(dsl::identity_keys)
			.values((
				dsl::kid.eq(kid),
				dsl::key_ref.eq(key_ref),
				dsl::first_served_at.eq(jiff_diesel::NullableTimestamp::from(Some(at))),
			))
			.on_conflict(dsl::kid)
			.do_update()
			.set((
				dsl::key_ref.eq(key_ref),
				dsl::known_at.eq(diesel::dsl::now),
				dsl::first_served_at.eq(diesel::dsl::sql::<
					diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
				>(
					"COALESCE(identity_keys.first_served_at, excluded.first_served_at)",
				)),
			))
			.execute(conn)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Record that `kid` signed. The first signature stands; later ones change
	/// nothing.
	pub async fn mark_signed(conn: &mut AsyncPgConnection, kid: &str, key_ref: &str) -> Result<()> {
		use crate::schema::identity_keys::dsl;
		diesel::insert_into(dsl::identity_keys)
			.values((
				dsl::kid.eq(kid),
				dsl::key_ref.eq(key_ref),
				dsl::first_signed_at
					.eq(jiff_diesel::NullableTimestamp::from(Some(Timestamp::now()))),
			))
			.on_conflict(dsl::kid)
			.do_update()
			.set(dsl::first_signed_at.eq(diesel::dsl::sql::<
				diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
			>(
				"COALESCE(identity_keys.first_signed_at, excluded.first_signed_at)",
			)))
			.execute(conn)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}

/// A configured signing key: its reference in the configuration, and its `kid`
/// when its public half is known.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConfiguredKey<'a> {
	pub key_ref: &'a str,
	pub kid: Option<&'a str>,
}

impl ConfiguredKey<'_> {
	/// Whether this is the key `row` records, by `kid` where it is known and by
	/// reference where it is not.
	fn is(&self, row: &IdentityKey, observed: &HashMap<String, IdentityKey>) -> bool {
		match self.kid {
			Some(kid) => kid == row.kid,
			None => self.row(observed).is_some_and(|known| known.kid == row.kid),
		}
	}

	fn row<'r>(&self, observed: &'r HashMap<String, IdentityKey>) -> Option<&'r IdentityKey> {
		match self.kid {
			Some(kid) => observed.get(kid),
			None => IdentityKey::by_ref(observed, self.key_ref),
		}
	}
}

/// Why no configured key signs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CannotSign {
	/// No key is configured.
	NoKeys,
	/// The key that would sign, `key_ref`, cannot be read, and no newer key has
	/// matured.
	Unreadable { key_ref: String },
	/// The key that last signed is not configured. Keys are placed against it
	/// by their order in the configuration, so none can be while it is absent,
	/// and signing never risks moving back to an older key.
	Unconfigured { kid: String, key_ref: String },
}

impl CannotSign {
	pub fn describe(&self) -> String {
		match self {
			Self::NoKeys => "no signing key is configured".into(),
			Self::Unreadable { key_ref } => {
				format!("the signing key {key_ref} cannot be read from the key service")
			}
			Self::Unconfigured { kid, key_ref } => format!(
				"the key that last signed, {kid} ({key_ref}), is not configured, so no other key can be placed against it"
			),
		}
	}
}

/// Where the key that last signed sits among the configured keys.
enum Floor<'a> {
	/// No key has signed.
	Unsigned,
	At(usize),
	Unconfigured(&'a IdentityKey),
}

/// The newest key to have signed is the one that first signed last, since
/// signing only moves forward. It is found by reference where its public half
/// cannot be read, so the keys older than it still retire.
fn floor<'a>(keys: &[ConfiguredKey<'_>], observed: &'a HashMap<String, IdentityKey>) -> Floor<'a> {
	let Some(latest) = observed
		.values()
		.filter(|row| row.first_signed_at.is_some())
		.max_by_key(|row| row.first_signed_at)
	else {
		return Floor::Unsigned;
	};
	match keys.iter().position(|key| key.is(latest, observed)) {
		Some(at) => Floor::At(at),
		None => Floor::Unconfigured(latest),
	}
}

/// Which of the configured keys signs: the newest that has matured, never one
/// older than a key that has already signed. `keys` are the configured keys
/// oldest first; one whose public half is not known cannot sign.
///
/// The first key an instance is configured with is mature from the outset, there
/// being no earlier key for relying parties to rely on meanwhile, so it signs
/// while nothing has.
// spec: IDP#signing-keys
pub fn signing(
	keys: &[ConfiguredKey<'_>],
	observed: &HashMap<String, IdentityKey>,
	now: Timestamp,
) -> std::result::Result<usize, CannotSign> {
	let mature = |i: usize| {
		keys[i]
			.row(observed)
			.and_then(|r| r.first_served_at)
			.is_some_and(|served| served + MATURITY <= now)
	};
	let readable = |i: usize| keys[i].kid.is_some();
	let unreadable = |i: usize| CannotSign::Unreadable {
		key_ref: keys[i].key_ref.to_owned(),
	};

	match floor(keys, observed) {
		Floor::Unconfigured(latest) => Err(CannotSign::Unconfigured {
			kid: latest.kid.clone(),
			key_ref: latest.key_ref.clone(),
		}),
		Floor::At(floor) => (floor..keys.len())
			.rev()
			.find(|&i| readable(i) && (i == floor || mature(i)))
			.ok_or_else(|| unreadable(floor)),
		// Nothing has signed: the newest matured key, or the first key, which
		// is mature from the outset.
		Floor::Unsigned if keys.is_empty() => Err(CannotSign::NoKeys),
		Floor::Unsigned => (1..keys.len())
			.rev()
			.find(|&i| readable(i) && mature(i))
			.or_else(|| readable(0).then_some(0))
			.ok_or_else(|| unreadable(0)),
	}
}

/// [`signing`], without the reason none signs.
pub fn signing_index(
	keys: &[ConfiguredKey<'_>],
	observed: &HashMap<String, IdentityKey>,
	now: Timestamp,
) -> Option<usize> {
	signing(keys, observed, now).ok()
}

/// The state a configured key is in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, utoipa::ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum KeyState {
	/// Not observed served at the issuer.
	NotYetServed,
	/// Served and newer than the key signing, not yet matured.
	WaitingToSign,
	Signing,
	/// Older than the key signing, with a token it signed not yet expired.
	Retired,
	/// Older than the key signing, every token it signed expired.
	Removable,
}

impl KeyState {
	/// How it is presented: signing, published, or removable.
	pub fn presented(self) -> &'static str {
		match self {
			Self::Signing => "signing",
			Self::Removable => "removable",
			Self::NotYetServed | Self::WaitingToSign | Self::Retired => "published",
		}
	}
}

/// The state of each configured key, oldest first. `last_expiry` gives the
/// latest expiry of a token each key signed, by `kid`.
///
/// While the key that last signed is configured but cannot be read, the keys
/// older than it still retire against it. While it is not configured at all,
/// no key can be placed against it, so none is presented as retired or
/// removable.
// spec: IDP#signing-keys
pub fn key_states(
	keys: &[ConfiguredKey<'_>],
	observed: &HashMap<String, IdentityKey>,
	last_expiry: &HashMap<String, Timestamp>,
	now: Timestamp,
) -> Vec<KeyState> {
	let signing = signing_index(keys, observed, now);
	let pivot = signing.or(match floor(keys, observed) {
		Floor::At(at) => Some(at),
		Floor::Unsigned | Floor::Unconfigured(_) => None,
	});
	(0..keys.len())
		.map(|i| {
			let row = keys[i].row(observed);
			let served = row.is_some_and(|r| r.first_served_at.is_some());
			match pivot {
				Some(s) if i == s && signing == Some(s) => KeyState::Signing,
				Some(s) if i < s => {
					let live = row
						.and_then(|r| last_expiry.get(&r.kid))
						.is_some_and(|expiry| *expiry > now);
					if live {
						KeyState::Retired
					} else {
						KeyState::Removable
					}
				}
				_ if served => KeyState::WaitingToSign,
				_ => KeyState::NotYetServed,
			}
		})
		.collect()
}

/// What the audit log says each key has signed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KeyUsage {
	/// When it last signed, within the log's ninety days.
	pub last_signed_at: Option<Timestamp>,
	/// How many tokens it signed in the last thirty days.
	pub signed_30d: i64,
	/// The latest expiry of a token it signed that may still be live.
	pub latest_expiry: Option<Timestamp>,
}

/// The usage of each of `kids`, from the issuances in the audit log; a key
/// that signed nothing in the log's ninety days is absent. Only issuances that
/// took place count: a refusal is settled failed, and one committed ahead and
/// never settled is still pending.
///
/// Each figure is read for each key on its own, through the log's index on
/// what it records, so the most recent signature is found without reading
/// back past it.
// spec: IDP#signing-keys
pub async fn key_usage(
	conn: &mut AsyncPgConnection,
	kids: &[String],
) -> Result<HashMap<String, KeyUsage>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = diesel::sql_types::Text)]
		kid: String,
		#[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
		last_signed_at: Option<jiff_diesel::Timestamp>,
		#[diesel(sql_type = diesel::sql_types::BigInt)]
		signed_30d: i64,
		#[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
		latest_expiry: Option<jiff_diesel::Timestamp>,
	}

	if kids.is_empty() {
		return Ok(HashMap::new());
	}
	// A token lives for its relying party's lifetime at most, so one still live
	// was issued within that bound, and the expiry is read from those alone.
	let live_window_minutes = LIFETIME_MINUTES.end() + 1;
	let rows: Vec<Row> = diesel::sql_query(
		"SELECT k.kid, \
		        (SELECT a.recorded_at FROM audit_entries a \
		          WHERE a.action = 'identity_token_issued' \
		            AND a.recorded_at > now() - interval '90 days' \
		            AND (a.outcome IS NULL OR a.outcome = 'done') \
		            AND a.detail @> jsonb_build_object('kid', k.kid) \
		          ORDER BY a.recorded_at DESC LIMIT 1) AS last_signed_at, \
		        (SELECT count(*) FROM audit_entries a \
		          WHERE a.action = 'identity_token_issued' \
		            AND a.recorded_at > now() - interval '30 days' \
		            AND (a.outcome IS NULL OR a.outcome = 'done') \
		            AND a.detail @> jsonb_build_object('kid', k.kid)) AS signed_30d, \
		        (SELECT max((a.detail ->> 'expires_at')::timestamptz) FROM audit_entries a \
		          WHERE a.action = 'identity_token_issued' \
		            AND a.recorded_at > now() - make_interval(mins => $2) \
		            AND (a.outcome IS NULL OR a.outcome = 'done') \
		            AND a.detail @> jsonb_build_object('kid', k.kid)) AS latest_expiry \
		 FROM unnest($1::text[]) AS k(kid)",
	)
	.bind::<diesel::sql_types::Array<diesel::sql_types::Text>, _>(kids)
	.bind::<diesel::sql_types::Integer, _>(live_window_minutes)
	.load(conn)
	.await
	.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.filter_map(|row| {
			let last_signed_at = row.last_signed_at?.to_jiff();
			Some((
				row.kid,
				KeyUsage {
					last_signed_at: Some(last_signed_at),
					signed_30d: row.signed_30d,
					latest_expiry: row.latest_expiry.map(|at| at.to_jiff()),
				},
			))
		})
		.collect())
}

/// When a token was last issued for each of `relying_parties`, within the
/// log's ninety days. A relying party with none in that time is absent, and a
/// refusal, settled failed, is not an issuance.
///
/// Read for each relying party on its own, through the log's index on the
/// records an entry acted on, so finding the latest issuance does not read
/// back past it.
// spec: IDP#relying-parties
pub async fn last_issued(
	conn: &mut AsyncPgConnection,
	relying_parties: &[Uuid],
) -> Result<HashMap<Uuid, Timestamp>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = diesel::sql_types::Uuid)]
		party: Uuid,
		#[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
		at: Option<jiff_diesel::Timestamp>,
	}

	if relying_parties.is_empty() {
		return Ok(HashMap::new());
	}
	let rows: Vec<Row> = diesel::sql_query(
		"SELECT p.party, \
		        (SELECT a.recorded_at FROM audit_entries a \
		          WHERE a.action = 'identity_token_issued' \
		            AND a.recorded_at > now() - interval '90 days' \
		            AND a.records @> jsonb_build_array(jsonb_build_object( \
		                  'kind', 'relying_party', 'id', p.party::text)) \
		            AND (a.outcome IS NULL OR a.outcome = 'done') \
		          ORDER BY a.recorded_at DESC LIMIT 1) AS at \
		 FROM unnest($1::uuid[]) AS p(party)",
	)
	.bind::<diesel::sql_types::Array<diesel::sql_types::Uuid>, _>(relying_parties)
	.load(conn)
	.await
	.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.filter_map(|row| Some((row.party, row.at?.to_jiff())))
		.collect())
}

#[cfg(test)]
mod tests {
	use super::*;

	fn key(
		kid: &str,
		served: Option<Timestamp>,
		signed: Option<Timestamp>,
	) -> (String, IdentityKey) {
		(
			kid.to_owned(),
			IdentityKey {
				kid: kid.to_owned(),
				key_ref: format!("ref-{kid}"),
				known_at: at(1000),
				first_served_at: served,
				first_signed_at: signed,
			},
		)
	}

	fn at(mins_ago: i64) -> Timestamp {
		Timestamp::from_second(1_800_000_000).unwrap() - SignedDuration::from_mins(mins_ago)
	}

	fn now() -> Timestamp {
		at(0)
	}

	/// Configured keys by `kid`, oldest first, each referenced as `ref-<kid>`;
	/// a `kid` written `!b` is configured but its public half cannot be read.
	fn cfg(kids: &[&str]) -> Vec<ConfiguredKey<'static>> {
		kids.iter()
			.map(|kid| match kid.strip_prefix('!') {
				Some(kid) => ConfiguredKey {
					key_ref: Box::leak(format!("ref-{kid}").into_boxed_str()),
					kid: None,
				},
				None => ConfiguredKey {
					key_ref: Box::leak(format!("ref-{kid}").into_boxed_str()),
					kid: Some(Box::leak(kid.to_string().into_boxed_str())),
				},
			})
			.collect()
	}

	#[test]
	fn the_first_key_signs_before_it_is_observed() {
		assert_eq!(signing(&cfg(&["a"]), &HashMap::new(), now()), Ok(0));
		assert_eq!(
			signing(&cfg(&["!a"]), &HashMap::new(), now()),
			Err(CannotSign::Unreadable {
				key_ref: "ref-a".into()
			})
		);
		assert_eq!(
			signing(&cfg(&[]), &HashMap::new(), now()),
			Err(CannotSign::NoKeys)
		);
	}

	#[test]
	fn a_newer_key_signs_once_matured() {
		let kids = cfg(&["a", "b"]);
		let young: HashMap<_, _> = [
			key("a", Some(at(600)), Some(at(600))),
			key("b", Some(at(10)), None),
		]
		.into();
		assert_eq!(signing_index(&kids, &young, now()), Some(0));
		let mature: HashMap<_, _> = [
			key("a", Some(at(600)), Some(at(600))),
			key("b", Some(at(31)), None),
		]
		.into();
		assert_eq!(signing_index(&kids, &mature, now()), Some(1));
	}

	#[test]
	fn signing_never_moves_back() {
		// `b` has signed; `a` is older, so it never signs again even if `b`'s
		// observation were somehow lost.
		let kids = cfg(&["a", "b"]);
		let observed: HashMap<_, _> = [
			key("a", Some(at(900)), Some(at(900))),
			key("b", None, Some(at(60))),
		]
		.into();
		assert_eq!(signing_index(&kids, &observed, now()), Some(1));
		// And with `b`'s public half unknown, nothing signs rather than `a`.
		assert_eq!(
			signing(&cfg(&["a", "!b"]), &observed, now()),
			Err(CannotSign::Unreadable {
				key_ref: "ref-b".into()
			})
		);
		// Nor with `b` taken out of the configuration.
		assert_eq!(
			signing(&cfg(&["a"]), &observed, now()),
			Err(CannotSign::Unconfigured {
				kid: "b".into(),
				key_ref: "ref-b".into()
			})
		);
	}

	#[test]
	fn an_unreadable_signer_is_still_placed() {
		// `b` signed and now cannot be read. `c` is newer and matured, so it
		// signs; without it nothing does, and `a` stays retired against `b`.
		let observed: HashMap<_, _> = [
			key("a", Some(at(900)), Some(at(900))),
			key("b", Some(at(600)), Some(at(600))),
			key("c", Some(at(60)), None),
		]
		.into();
		assert_eq!(
			signing_index(&cfg(&["a", "!b", "c"]), &observed, now()),
			Some(2)
		);
		let young: HashMap<_, _> = [
			key("a", Some(at(900)), Some(at(900))),
			key("b", Some(at(600)), Some(at(600))),
			key("c", Some(at(5)), None),
		]
		.into();
		assert_eq!(
			key_states(&cfg(&["a", "!b", "c"]), &young, &HashMap::new(), now()),
			[
				KeyState::Removable,
				KeyState::WaitingToSign,
				KeyState::WaitingToSign
			]
		);
	}

	#[test]
	fn unobserved_later_keys_wait() {
		let kids = cfg(&["a", "b", "c"]);
		let observed: HashMap<_, _> = [
			key("a", Some(at(900)), Some(at(900))),
			key("b", Some(at(5)), None),
		]
		.into();
		let states = key_states(&kids, &observed, &HashMap::new(), now());
		assert_eq!(
			states,
			[
				KeyState::Signing,
				KeyState::WaitingToSign,
				KeyState::NotYetServed
			]
		);
		assert_eq!(
			states.iter().map(|s| s.presented()).collect::<Vec<_>>(),
			["signing", "published", "published"]
		);
	}

	#[test]
	fn older_keys_retire_then_become_removable() {
		let kids = cfg(&["a", "b", "c"]);
		let observed: HashMap<_, _> = [
			key("a", Some(at(900)), Some(at(900))),
			key("b", Some(at(600)), Some(at(600))),
			key("c", Some(at(60)), None),
		]
		.into();
		let expiries: HashMap<_, _> =
			[("b".to_owned(), now() + SignedDuration::from_mins(3))].into();
		assert_eq!(
			key_states(&kids, &observed, &expiries, now()),
			[KeyState::Removable, KeyState::Retired, KeyState::Signing]
		);
	}

	#[test]
	fn a_key_that_never_signed_is_removable_once_a_newer_one_signs() {
		let kids = cfg(&["a", "b"]);
		let observed: HashMap<_, _> = [
			key("a", Some(at(900)), None),
			key("b", Some(at(600)), Some(at(600))),
		]
		.into();
		assert_eq!(
			key_states(&kids, &observed, &HashMap::new(), now()),
			[KeyState::Removable, KeyState::Signing]
		);
		assert_eq!(KeyState::Retired.presented(), "published");
	}

	#[test]
	fn no_subject_carries_a_value_outside_the_alphabet() {
		let placement = Placement {
			group_id: Uuid::new_v4(),
			rank: ServerRank::Production,
		};
		let (machine, application) = (Uuid::new_v4(), Uuid::new_v4());
		assert_eq!(
			subject_of(&placement, machine, None),
			Some(format!(
				"group:{}:rank:production:machine:{machine}:",
				placement.group_id
			))
		);
		assert_eq!(
			subject_of(
				&placement,
				machine,
				Some(("tamanu-central".into(), application))
			),
			Some(format!(
				"group:{}:rank:production:application:tamanu-central:{application}:",
				placement.group_id
			))
		);
		assert_eq!(
			subject_of(&placement, machine, Some(("Bad_Type".into(), application))),
			None
		);
		for label in LABELS {
			assert_eq!(
				subject_of(&placement, machine, Some(((*label).into(), application))),
				None,
				"an application type spelled as the label {label} is refused"
			);
		}
	}

	#[test]
	fn subject_alphabet() {
		assert!(in_alphabet("tamanu-central"));
		assert!(in_alphabet("0f3c2c5e-1b2a-4c3d-9e8f-001122334455"));
		assert!(!in_alphabet("Tamanu"));
		assert!(!in_alphabet("a:b"));
		assert!(!in_alphabet("a_b"));
		assert!(!in_alphabet(""));
	}
}
