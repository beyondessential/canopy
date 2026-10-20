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
use serde::Serialize;
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
		if Self::by_audience(conn, audience).await?.is_some() {
			return Err(AppError::Conflict(format!(
				"a relying party with the audience {audience:?} is already registered"
			)));
		}

		diesel::insert_into(dsl::relying_parties)
			.values((
				dsl::name.eq(name),
				dsl::audience.eq(audience),
				dsl::token_lifetime_minutes.eq(lifetime),
			))
			.returning(Self::as_select())
			.get_result(conn)
			.await
			.map_err(AppError::from)
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

	/// How many machines and applications hold a grant for each relying party.
	pub async fn grant_counts(conn: &mut AsyncPgConnection) -> Result<HashMap<Uuid, i64>> {
		use crate::schema::identity_grants::dsl;
		Ok(dsl::identity_grants
			.group_by(dsl::relying_party_id)
			.select((dsl::relying_party_id, diesel::dsl::count_star()))
			.load::<(Uuid, i64)>(conn)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.collect())
	}
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, utoipa::ToSchema)]
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
// spec: IDP#grants
pub async fn set_grants(
	conn: &mut AsyncPgConnection,
	subject: GrantSubject,
	relying_parties: &[Uuid],
) -> Result<GrantChanges> {
	use crate::schema::identity_grants::dsl;

	let held = grants_of(conn, &[subject])
		.await?
		.remove(&subject)
		.unwrap_or_default();
	let mut wanted = relying_parties.to_vec();
	wanted.sort_unstable();
	wanted.dedup();

	let changes = GrantChanges {
		granted: wanted
			.iter()
			.filter(|p| !held.contains(p))
			.copied()
			.collect(),
		revoked: held
			.iter()
			.filter(|p| !wanted.contains(p))
			.copied()
			.collect(),
	};

	let (machine, application) = subject.to_columns();
	if !changes.revoked.is_empty() {
		let query = diesel::delete(
			dsl::identity_grants.filter(dsl::relying_party_id.eq_any(&changes.revoked)),
		);
		match (machine, application) {
			(Some(machine), _) => {
				query
					.filter(dsl::machine_id.eq(machine))
					.execute(conn)
					.await
			}
			(_, Some(application)) => {
				query
					.filter(dsl::application_id.eq(application))
					.execute(conn)
					.await
			}
			_ => unreachable!("a subject has exactly one column"),
		}
		.map_err(AppError::from)?;
	}
	if !changes.granted.is_empty() {
		diesel::insert_into(dsl::identity_grants)
			.values(
				changes
					.granted
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
			.execute(conn)
			.await
			.map_err(AppError::from)?;
	}
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
/// it, carries. `None` when a value falls outside the subject's alphabet, which
/// Canopy issues no token for.
// spec: IDT#subject
pub fn subject(
	placement: &Placement,
	machine: &Machine,
	application: Option<&Application>,
) -> Option<String> {
	let group = placement.group_id.to_string();
	let rank = placement.rank.to_string();
	let mut segments: Vec<(&str, String)> = vec![("group", group), ("rank", rank)];
	match application {
		Some(application) => {
			segments.push(("application", application.r#type.to_string()));
			segments.push(("", application.id.to_string()));
		}
		None => segments.push(("machine", machine.id.to_string())),
	}
	if !segments.iter().all(|(_, value)| in_alphabet(value)) {
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

/// Lowercase letters, digits and hyphens, so no value can contain the separator
/// or be mistaken for a label.
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

	/// Record that the configured key `key_ref` has the identifier `kid`, so
	/// it is known by it before it is ever served.
	pub async fn know(conn: &mut AsyncPgConnection, kid: &str, key_ref: &str) -> Result<()> {
		use crate::schema::identity_keys::dsl;
		diesel::insert_into(dsl::identity_keys)
			.values((dsl::kid.eq(kid), dsl::key_ref.eq(key_ref)))
			.on_conflict(dsl::kid)
			.do_update()
			.set(dsl::key_ref.eq(key_ref))
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

/// Which of the configured keys signs: the newest that has matured, never one
/// older than a key that has already signed. `kids` are the configured keys
/// oldest first, `None` for one whose public half is not known.
///
/// The first key an instance is configured with is mature from the outset, there
/// being no earlier key for relying parties to rely on meanwhile, so it signs
/// while nothing has.
// spec: IDP#signing-keys
pub fn signing_index(
	kids: &[Option<&str>],
	observed: &HashMap<String, IdentityKey>,
	now: Timestamp,
) -> Option<usize> {
	let row = |i: usize| kids[i].and_then(|kid| observed.get(kid));

	// The newest key to have signed is the one that first signed last, since
	// signing only moves forward. Where it is not among the keys in hand, its
	// public half unread or it taken out of the configuration, nothing signs
	// rather than an older key.
	let floor = match observed
		.values()
		.filter_map(|r| r.first_signed_at.map(|at| (r, at)))
		.max_by_key(|(_, at)| *at)
	{
		None => None,
		Some((latest, _)) => Some(
			kids.iter()
				.position(|kid| *kid == Some(latest.kid.as_str()))?,
		),
	};
	let mature = |i: usize| {
		row(i)
			.and_then(|r| r.first_served_at)
			.is_some_and(|served| served + MATURITY <= now)
	};

	match floor {
		Some(floor) => (floor..kids.len())
			.rev()
			.find(|&i| kids[i].is_some() && (i == floor || mature(i))),
		// Nothing has signed: the newest matured key, or the first key, which
		// is mature from the outset.
		None => (1..kids.len())
			.rev()
			.find(|&i| kids[i].is_some() && mature(i))
			.or_else(|| kids.first().is_some_and(Option::is_some).then_some(0)),
	}
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
// spec: IDP#signing-keys
pub fn key_states(
	kids: &[Option<&str>],
	observed: &HashMap<String, IdentityKey>,
	last_expiry: &HashMap<String, Timestamp>,
	now: Timestamp,
) -> Vec<KeyState> {
	let signing = signing_index(kids, observed, now);
	(0..kids.len())
		.map(|i| {
			let served = kids[i]
				.and_then(|kid| observed.get(kid))
				.is_some_and(|r| r.first_served_at.is_some());
			match signing {
				Some(s) if i == s => KeyState::Signing,
				Some(s) if i < s => {
					let live = kids[i]
						.and_then(|kid| last_expiry.get(kid))
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

/// Each key's usage, by `kid`, from the issuances in the audit log. A refusal
/// carries no key, so it counts for none.
// spec: IDP#signing-keys
pub async fn key_usage(conn: &mut AsyncPgConnection) -> Result<HashMap<String, KeyUsage>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = diesel::sql_types::Text)]
		kid: String,
		#[diesel(sql_type = diesel::sql_types::Timestamptz)]
		last_signed_at: jiff_diesel::Timestamp,
		#[diesel(sql_type = diesel::sql_types::BigInt)]
		signed_30d: i64,
		#[diesel(sql_type = diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>)]
		latest_expiry: Option<jiff_diesel::Timestamp>,
	}

	// A token lives an hour at most, so one still live was issued within the
	// last hour, and the expiry is read from those alone.
	let rows: Vec<Row> = diesel::sql_query(
		"SELECT detail ->> 'kid' AS kid, \
		        max(recorded_at) AS last_signed_at, \
		        count(*) FILTER (WHERE recorded_at > now() - interval '30 days') AS signed_30d, \
		        max((detail ->> 'expires_at')::timestamptz) \
		            FILTER (WHERE recorded_at > now() - interval '61 minutes') AS latest_expiry \
		 FROM audit_entries \
		 WHERE action = 'identity_token_issued' \
		   AND recorded_at > now() - interval '90 days' \
		   AND detail ->> 'kid' IS NOT NULL \
		 GROUP BY 1",
	)
	.load(conn)
	.await
	.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.map(|row| {
			(
				row.kid,
				KeyUsage {
					last_signed_at: Some(row.last_signed_at.to_jiff()),
					signed_30d: row.signed_30d,
					latest_expiry: row.latest_expiry.map(|at| at.to_jiff()),
				},
			)
		})
		.collect())
}

/// When a token was last issued for each relying party, within the log's
/// ninety days. A relying party with none in that time is absent.
// spec: IDP#relying-parties
pub async fn last_issued(conn: &mut AsyncPgConnection) -> Result<HashMap<Uuid, Timestamp>> {
	#[derive(QueryableByName)]
	struct Row {
		#[diesel(sql_type = diesel::sql_types::Text)]
		party: String,
		#[diesel(sql_type = diesel::sql_types::Timestamptz)]
		at: jiff_diesel::Timestamp,
	}

	let rows: Vec<Row> = diesel::sql_query(
		"SELECT record ->> 'id' AS party, max(recorded_at) AS at \
		 FROM audit_entries, jsonb_array_elements(records) AS record \
		 WHERE action = 'identity_token_issued' \
		   AND recorded_at > now() - interval '90 days' \
		   AND detail ->> 'jti' IS NOT NULL \
		   AND record ->> 'kind' = 'relying_party' \
		 GROUP BY 1",
	)
	.load(conn)
	.await
	.map_err(AppError::from)?;
	Ok(rows
		.into_iter()
		.filter_map(|row| Some((row.party.parse().ok()?, row.at.to_jiff())))
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

	#[test]
	fn the_first_key_signs_before_it_is_observed() {
		assert_eq!(signing_index(&[Some("a")], &HashMap::new(), now()), Some(0));
		assert_eq!(signing_index(&[None], &HashMap::new(), now()), None);
		assert_eq!(signing_index(&[], &HashMap::new(), now()), None);
	}

	#[test]
	fn a_newer_key_signs_once_matured() {
		let kids = [Some("a"), Some("b")];
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
		let kids = [Some("a"), Some("b")];
		let observed: HashMap<_, _> = [
			key("a", Some(at(900)), Some(at(900))),
			key("b", None, Some(at(60))),
		]
		.into();
		assert_eq!(signing_index(&kids, &observed, now()), Some(1));
		// And with `b`'s public half unknown, nothing signs rather than `a`.
		assert_eq!(signing_index(&[Some("a"), None], &observed, now()), None);
		// Nor with `b` taken out of the configuration.
		assert_eq!(signing_index(&[Some("a")], &observed, now()), None);
	}

	#[test]
	fn unobserved_later_keys_wait() {
		let kids = [Some("a"), Some("b"), Some("c")];
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
		let kids = [Some("a"), Some("b"), Some("c")];
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
	fn subject_alphabet() {
		assert!(in_alphabet("tamanu-central"));
		assert!(in_alphabet("0f3c2c5e-1b2a-4c3d-9e8f-001122334455"));
		assert!(!in_alphabet("Tamanu"));
		assert!(!in_alphabet("a:b"));
		assert!(!in_alphabet("a_b"));
		assert!(!in_alphabet(""));
	}
}
