//! The identity provider's settings (IDP): the issuer and its signing keys, the
//! register of relying parties, and which machines and applications may obtain
//! identity tokens for each.
//!
//! Reads are open to any tailnet user; changes require admin. Every change is
//! recorded in the audit log, which is where who did what lives.
// spec: IDP

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::{
	audit::Audit,
	tailscale_auth::{TailscaleAdmin, TailscaleUser},
};
use database::{
	Machine,
	audit::Action,
	identity::{self, ConfiguredKey, GrantSubject, IdentityKey, KeyState, RelyingParty},
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only, silent: status))
		.routes(routes!(read_only, silent: relying_parties))
		.routes(routes!(write, audit(RelyingPartyRegistered): add_relying_party))
		.routes(routes!(write, audit(RelyingPartyEdited): edit_relying_party))
		.routes(routes!(
			danger(irreversible),
			audit(RelyingPartyRemoved, IdentityGrantRevoked): remove_relying_party
		))
		.routes(routes!(read_only, silent: machine_grants))
		.routes(routes!(write, audit(IdentityGranted, IdentityGrantRevoked): set_grants))
}

// ── Issuer and keys ─────────────────────────────────────────────────────────

/// The issuer and its signing keys.
#[derive(Debug, Serialize, ToSchema)]
pub struct IdentityStatus {
	/// The issuer, or null when none is configured.
	pub issuer: Option<String>,
	/// Where the discovery document is served, when there is an issuer.
	pub discovery_url: Option<String>,
	/// Whether this Canopy instance issues tokens: it has an issuer and a key.
	pub issues_tokens: bool,
	/// The configured keys, newest first.
	pub keys: Vec<KeyView>,
}

/// A configured signing key.
#[derive(Debug, Serialize, ToSchema)]
pub struct KeyView {
	/// Its reference in the configuration, the key service's identifier for it.
	pub key_ref: String,
	/// Its identifier, as tokens and the key set name it. Null until Canopy has
	/// read the key.
	pub kid: Option<String>,
	/// How it is presented.
	pub state: PresentedKeyState,
	/// When Canopy first observed it served at the issuer.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub first_served_at: Option<Timestamp>,
	/// When it last signed, within the audit log's ninety days.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub last_signed_at: Option<Timestamp>,
	/// How many tokens it signed in the last thirty days.
	pub signed_30d: i64,
}

/// A key as an operator sees it.
// spec: IDP#signing-keys
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, ToSchema)]
#[serde(rename_all = "snake_case")]
pub enum PresentedKeyState {
	/// The key tokens are signed with.
	Signing,
	/// Served in the key set, and not signing: waiting to sign, retired, or not
	/// yet observed served.
	Published,
	/// Older than the key signing, every token it signed expired: it can be
	/// taken out of the configuration.
	Removable,
}

impl From<KeyState> for PresentedKeyState {
	fn from(state: KeyState) -> Self {
		match state {
			KeyState::Signing => Self::Signing,
			KeyState::Removable => Self::Removable,
			KeyState::NotYetServed | KeyState::WaitingToSign | KeyState::Retired => Self::Published,
		}
	}
}

/// The issuer, the discovery document's address, and the signing keys.
///
/// Each key with its state and how it has been used. A key is known by its identifier once Canopy has read it and observed what
/// the issuer serves, so a key added since is shown by its reference alone.
#[utoipa::path(
	post,
	path = "/status",
	tag = "identity",
	security(("tailscale-user" = [])),
	responses((status = 200, body = IdentityStatus)),
)]
pub async fn status(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<IdentityStatus>> {
	let config = &state.identity_config;
	let mut conn = state.db_read.get().await?;
	let observed = IdentityKey::all(&mut conn).await?;
	let usage = identity::key_usage(&mut conn).await?;

	let by_ref: HashMap<&str, &IdentityKey> = observed
		.values()
		.map(|row| (row.key_ref.as_str(), row))
		.collect();
	let configured: Vec<ConfiguredKey> = config
		.keys
		.iter()
		.map(|key_ref| ConfiguredKey {
			key_ref,
			kid: by_ref.get(key_ref.as_str()).map(|row| row.kid.as_str()),
		})
		.collect();
	let expiries: HashMap<String, Timestamp> = usage
		.iter()
		.filter_map(|(kid, usage)| Some((kid.clone(), usage.latest_expiry?)))
		.collect();
	let states = identity::key_states(&configured, &observed, &expiries, Timestamp::now());

	let mut keys: Vec<KeyView> = configured
		.iter()
		.zip(states)
		.map(|(&ConfiguredKey { key_ref, kid }, state)| {
			let row = kid.and_then(|kid| observed.get(kid));
			let used = kid
				.and_then(|kid| usage.get(kid))
				.cloned()
				.unwrap_or_default();
			KeyView {
				key_ref: key_ref.to_owned(),
				kid: kid.map(str::to_owned),
				state: state.into(),
				first_served_at: row.and_then(|row| row.first_served_at),
				last_signed_at: used.last_signed_at,
				signed_30d: used.signed_30d,
			}
		})
		.collect();
	keys.reverse();

	Ok(Json(IdentityStatus {
		discovery_url: config
			.issuer
			.as_ref()
			.map(|issuer| format!("{issuer}{}", oidc::DISCOVERY_PATH)),
		issuer: config.issuer.clone(),
		issues_tokens: config.issues_tokens(),
		keys,
	}))
}

// ── Relying parties ─────────────────────────────────────────────────────────

/// A relying party in the register.
#[derive(Debug, Serialize, ToSchema)]
pub struct RelyingPartyView {
	/// Its identifier.
	pub id: Uuid,
	/// What operators see.
	pub name: String,
	/// What its tokens carry as `aud`.
	pub audience: String,
	/// How long its tokens live, in minutes.
	pub token_lifetime_minutes: i32,
	/// How many machines hold a grant for it.
	pub machine_grants: i64,
	/// How many applications hold a grant for it.
	pub application_grants: i64,
	/// When a token was last issued for it, within the audit log's ninety days.
	#[schema(value_type = Option<String>, format = DateTime)]
	pub last_issued_at: Option<Timestamp>,
}

/// The register of relying parties.
///
/// Each with its audience, token lifetime, how many machines and applications
/// hold a grant for it, and when a token was last issued for it.
#[utoipa::path(
	post,
	path = "/relying_parties",
	tag = "identity",
	security(("tailscale-user" = [])),
	responses((status = 200, body = Vec<RelyingPartyView>)),
)]
pub async fn relying_parties(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<Vec<RelyingPartyView>>> {
	let mut conn = state.db_read.get().await?;
	let parties = RelyingParty::list(&mut conn).await?;
	let counts = RelyingParty::grant_counts(&mut conn).await?;
	let last = identity::last_issued(&mut conn).await?;
	Ok(Json(
		parties
			.into_iter()
			.map(|party| RelyingPartyView {
				machine_grants: counts.get(&party.id).map_or(0, |c| c.machines),
				application_grants: counts.get(&party.id).map_or(0, |c| c.applications),
				last_issued_at: last.get(&party.id).copied(),
				id: party.id,
				name: party.name,
				audience: party.audience,
				token_lifetime_minutes: party.token_lifetime_minutes,
			})
			.collect(),
	))
}

/// A relying party to register.
#[derive(Debug, Deserialize, ToSchema)]
pub struct AddRelyingPartyArgs {
	/// What operators see.
	pub name: String,
	/// What its tokens carry as `aud`, and what a machine names when it asks
	/// for one. Fixed once registered.
	pub audience: String,
	/// Whole minutes, from one to sixty. Five when absent.
	#[serde(default)]
	pub token_lifetime_minutes: Option<i32>,
}

/// Register a relying party.
///
/// Two never share an audience. Registering is recorded in the audit log.
#[utoipa::path(
	post,
	path = "/add_relying_party",
	tag = "identity",
	security(("tailscale-admin" = [])),
	request_body = AddRelyingPartyArgs,
	responses(
		(status = 200, body = RelyingParty),
		(status = 400, body = ProblemDetailsSchema),
		(status = 409, description = "The audience is already registered.", body = ProblemDetailsSchema),
	),
)]
pub async fn add_relying_party(
	audit: Audit,
	_admin: TailscaleAdmin,
	Json(args): Json<AddRelyingPartyArgs>,
) -> Result<Json<RelyingParty>> {
	let mut conn = audit.begin().await?;
	let party = RelyingParty::add(
		&mut conn,
		&args.name,
		&args.audience,
		args.token_lifetime_minutes,
	)
	.await?;
	conn.record(Action::RelyingPartyRegistered {
		relying_party: party.record(),
		audience: party.audience.clone(),
		token_lifetime_minutes: party.token_lifetime_minutes,
	})
	.await?;
	Ok(Json(party))
}

/// A relying party's new name and token lifetime.
#[derive(Debug, Deserialize, ToSchema)]
pub struct EditRelyingPartyArgs {
	/// The relying party.
	pub id: Uuid,
	/// What operators see.
	pub name: String,
	/// Whole minutes, from one to sixty.
	pub token_lifetime_minutes: i32,
}

/// Change a relying party's name and token lifetime.
///
/// A changed lifetime applies to tokens issued from then on. The audience is
/// fixed once registered. A save that changes nothing records nothing.
#[utoipa::path(
	post,
	path = "/edit_relying_party",
	tag = "identity",
	security(("tailscale-admin" = [])),
	request_body = EditRelyingPartyArgs,
	responses(
		(status = 200, body = RelyingParty),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn edit_relying_party(
	audit: Audit,
	_admin: TailscaleAdmin,
	Json(args): Json<EditRelyingPartyArgs>,
) -> Result<Json<RelyingParty>> {
	let mut conn = audit.begin().await?;
	match RelyingParty::edit(&mut conn, args.id, &args.name, args.token_lifetime_minutes).await? {
		None => {
			audit.unchanged();
			Ok(Json(RelyingParty::get(&mut conn, args.id).await?))
		}
		Some((before, after)) => {
			conn.record(Action::RelyingPartyEdited {
				relying_party: after.record(),
				name_from: before.name,
				name_to: after.name.clone(),
				token_lifetime_minutes_from: before.token_lifetime_minutes,
				token_lifetime_minutes_to: after.token_lifetime_minutes,
			})
			.await?;
			Ok(Json(after))
		}
	}
}

/// One relying party.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RelyingPartyIdArgs {
	/// The relying party.
	pub id: Uuid,
}

/// Remove a relying party.
///
/// Every grant for it is revoked, each recorded as a revocation, and a request
/// naming its audience is then refused as unknown.
#[utoipa::path(
	post,
	path = "/remove_relying_party",
	tag = "identity",
	security(("tailscale-admin" = [])),
	request_body = RelyingPartyIdArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn remove_relying_party(
	audit: Audit,
	_admin: TailscaleAdmin,
	Json(args): Json<RelyingPartyIdArgs>,
) -> Result<Json<()>> {
	let mut conn = audit.begin().await?;
	let (party, revoked) = RelyingParty::remove(&mut conn, args.id).await?;
	let relying_party = party.record();
	for subject in &revoked {
		let subject = subject.record(&mut conn).await?;
		conn.record(Action::IdentityGrantRevoked {
			subject,
			relying_party: relying_party.clone(),
		})
		.await?;
	}
	conn.record(Action::RelyingPartyRemoved {
		relying_party,
		audience: party.audience,
		grants_revoked: revoked.len(),
	})
	.await?;
	Ok(Json(()))
}

// ── Grants ──────────────────────────────────────────────────────────────────

/// One machine.
#[derive(Debug, Deserialize, ToSchema)]
pub struct IdentityMachineArgs {
	/// The machine.
	pub machine_id: Uuid,
}

/// What a machine's edit page offers for identity tokens.
#[derive(Debug, Serialize, ToSchema)]
pub struct MachineGrants {
	/// Whether the choice is offered for subjects that hold no grant: some
	/// relying party is registered, the Canopy instance issues tokens, and the
	/// machine is placed in an environment.
	pub offered: bool,
	/// Every registered relying party, to choose from.
	pub relying_parties: Vec<RelyingPartyOption>,
	/// The machine, then each of its live applications.
	pub subjects: Vec<SubjectGrants>,
}

/// A relying party a subject may be granted.
#[derive(Debug, Serialize, ToSchema)]
pub struct RelyingPartyOption {
	/// Its identifier.
	pub id: Uuid,
	/// What operators see.
	pub name: String,
	/// What its tokens carry as `aud`.
	pub audience: String,
}

/// One machine or application, and the relying parties it holds grants for.
#[derive(Debug, Serialize, ToSchema)]
pub struct SubjectGrants {
	/// The machine or application.
	pub subject: GrantSubject,
	/// The subject its tokens carry, for a relying party's rule. Null while the
	/// machine is not placed.
	pub sub: Option<String>,
	/// The relying parties it holds a grant for.
	pub granted: Vec<Uuid>,
}

/// The identity token grants of a machine and its applications.
///
/// With the registered relying parties to choose from, each subject's `sub`,
/// and whether the choice is offered for a subject holding no grant.
// spec: IDP#presentation
#[utoipa::path(
	post,
	path = "/machine_grants",
	tag = "identity",
	security(("tailscale-user" = [])),
	request_body = IdentityMachineArgs,
	responses((status = 200, body = MachineGrants), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn machine_grants(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<IdentityMachineArgs>,
) -> Result<Json<MachineGrants>> {
	let mut conn = state.db_read.get().await?;
	let machine = Machine::get_by_id(&mut conn, args.machine_id).await?;
	let applications = machine.applications(&mut conn).await?;
	let parties = RelyingParty::list(&mut conn).await?;

	let mut subjects = vec![GrantSubject::Machine(machine.id)];
	subjects.extend(applications.iter().map(|a| GrantSubject::Application(a.id)));
	let mut held = identity::grants_of(&mut conn, &subjects).await?;
	let placement = identity::placement(&machine).ok();

	let sub_of = |subject: GrantSubject| {
		let placement = placement.as_ref()?;
		match subject {
			GrantSubject::Machine(_) => identity::subject(placement, &machine, None),
			GrantSubject::Application(id) => {
				let application = applications.iter().find(|a| a.id == id)?;
				identity::subject(placement, &machine, Some(application))
			}
		}
	};

	Ok(Json(MachineGrants {
		offered: !parties.is_empty()
			&& state.identity_config.issues_tokens()
			&& placement.is_some(),
		relying_parties: parties
			.into_iter()
			.map(|party| RelyingPartyOption {
				id: party.id,
				name: party.name,
				audience: party.audience,
			})
			.collect(),
		subjects: subjects
			.into_iter()
			.map(|subject| SubjectGrants {
				sub: sub_of(subject),
				granted: held.remove(&subject).unwrap_or_default(),
				subject,
			})
			.collect(),
	}))
}

/// The grants a machine or application should hold.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetGrantsArgs {
	/// The machine or application.
	pub subject: GrantSubjectArg,
	/// Every relying party the subject should hold a grant for; the rest are
	/// revoked.
	pub relying_party_ids: Vec<Uuid>,
}

/// A machine or an application, on the way in.
#[derive(Debug, Clone, Copy, Deserialize, ToSchema)]
#[serde(tag = "kind", content = "id", rename_all = "snake_case")]
pub enum GrantSubjectArg {
	/// A machine, for tokens naming itself.
	Machine(Uuid),
	/// An application, for tokens naming it.
	Application(Uuid),
}

impl From<GrantSubjectArg> for GrantSubject {
	fn from(arg: GrantSubjectArg) -> Self {
		match arg {
			GrantSubjectArg::Machine(id) => Self::Machine(id),
			GrantSubjectArg::Application(id) => Self::Application(id),
		}
	}
}

/// Set a machine's or an application's grants.
///
/// Its grants become exactly those given, each grant and revocation recorded in
/// the audit log. Saving what it already holds changes nothing.
// spec: IDP#grants
#[utoipa::path(
	post,
	path = "/set_grants",
	tag = "identity",
	security(("tailscale-admin" = [])),
	request_body = SetGrantsArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn set_grants(
	audit: Audit,
	_admin: TailscaleAdmin,
	Json(args): Json<SetGrantsArgs>,
) -> Result<Json<()>> {
	let subject = GrantSubject::from(args.subject);
	let mut conn = audit.begin().await?;
	let subject_record = subject.record(&mut conn).await?;

	let mut parties = HashMap::new();
	for id in &args.relying_party_ids {
		let party = RelyingParty::get(&mut conn, *id).await?;
		parties.insert(party.id, party);
	}

	let changes = identity::set_grants(&mut conn, subject, &args.relying_party_ids).await?;
	if changes.is_empty() {
		audit.unchanged();
		return Ok(Json(()));
	}
	for id in &changes.granted {
		conn.record(Action::IdentityGranted {
			subject: subject_record.clone(),
			relying_party: parties[id].record(),
		})
		.await?;
	}
	for id in &changes.revoked {
		let party = RelyingParty::get(&mut conn, *id).await?;
		conn.record(Action::IdentityGrantRevoked {
			subject: subject_record.clone(),
			relying_party: party.record(),
		})
		.await?;
	}
	Ok(Json(()))
}
