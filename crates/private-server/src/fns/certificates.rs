//! Operator-facing name and certificate endpoints (private-server, admin SPA).
//!
//! Reads are open to any tailnet user; anything that changes what Canopy will do
//! on a server's behalf — the profile, the pause, a revocation — requires admin.
//!
//! Revocation is the one endpoint here that talks to the certificate authority
//! rather than only to the database, because an operator pressing revoke needs to
//! know whether it took. Canopy records it as revoked only once the authority has
//! accepted it, so the two never disagree.
// spec: CRT

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::acme::RevokeFor;
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::Uuid;
use commons_types::dns::{is_within, match_zone};
use database::application_certificates::{ApplicationCertificate, RevocationReason};
use database::applications::Application;
use database::{ApplicationName, ServerGroupDomain};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::fns::applications::ServerIdArgs;
use crate::fns::server_groups::GroupIdArgs;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: for_server))
		.routes(routes!(read_only: for_group))
		.routes(routes!(read_only: authority))
		.routes(routes!(write: set_profile))
		.routes(routes!(danger: pause))
		.routes(routes!(write: resume))
		.routes(routes!(danger: revoke))
		.routes(routes!(write: declare))
		.routes(routes!(write: release))
}

/// A name a server has registered, and how far Canopy has got with it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NameView {
	/// Unique identifier of the registration.
	pub id: Uuid,
	/// The name, normalised.
	pub name: String,
	/// The addresses the server asked to be reachable at.
	pub addresses: Vec<String>,
	/// The addresses Canopy has actually published. Differs from `addresses`
	/// while a change is waiting to be reconciled.
	pub published_addresses: Vec<String>,
	/// Whether the zone has caught up with what the server asked for.
	pub published: bool,
	/// When Canopy last published this name's records.
	#[schema(value_type = Option<String>)]
	pub published_at: Option<Timestamp>,
	/// Why the last publish attempt failed, if it did.
	pub last_error: Option<String>,
	/// Apex of the managed zone covering this name, or null where no configured
	/// zone does — in which case Canopy can publish nothing for it.
	pub zone: Option<String>,
}

/// A certificate Canopy holds for a server, or an order in flight.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CertificateView {
	/// Unique identifier of the certificate.
	pub id: Uuid,
	/// The single name it covers.
	pub name: String,
	/// `pending`, `issued`, `failed`, or `revoked`.
	pub state: String,
	/// The profile it was issued under — the authority's name for a lifetime.
	/// Null for one the authority offered no profile for.
	pub profile: Option<String>,
	/// When it expires. Null for an order that has produced nothing yet.
	#[schema(value_type = Option<String>)]
	pub not_after: Option<Timestamp>,
	/// How long is left, in seconds. Negative once expired, null before
	/// issuance — given alongside the instant so the UI need not compute it and
	/// the two cannot disagree.
	pub remaining_seconds: Option<i64>,
	/// When it was issued.
	#[schema(value_type = Option<String>)]
	pub issued_at: Option<Timestamp>,
	/// Whether the order in flight is extending a certificate that already
	/// issued, which tells a stalled renewal apart from one that never came up.
	pub renewing: bool,
	/// Whether the server can collect this certificate right now.
	pub collectable: bool,
	/// How urgently it needs attention: `none`, `at_risk`, or `critical`,
	/// judged against its own lifetime.
	pub risk: String,
	/// Failed attempts since the last success.
	pub attempts: i32,
	/// Why the last attempt failed, if it did.
	pub last_error: Option<String>,
	/// When an operator revoked it.
	#[schema(value_type = Option<String>)]
	pub revoked_at: Option<Timestamp>,
	/// Who revoked it.
	pub revoked_by: Option<String>,
	/// The reason given for revocation.
	pub revocation_reason: Option<String>,
	/// Hex SHA-256 of the certified key, so an operator can tell two
	/// certificates for the same name apart.
	pub key_fingerprint: String,
}

/// What a server's page shows about its names and certificates.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ApplicationNamesView {
	/// Whether an operator has allowed this server to manage its own DNS.
	pub may_manage_dns: bool,
	/// Whether an operator has allowed this server to obtain its own
	/// certificates.
	pub may_manage_tls: bool,
	/// The profile this server's certificates are requested under. Null means
	/// the authority's own default, which is its longest-lived.
	pub certificate_profile: Option<String>,
	/// Whether Canopy has been told to stop doing anything new for this server.
	pub paused: bool,
	/// When the pause was set.
	#[schema(value_type = Option<String>)]
	pub paused_at: Option<Timestamp>,
	/// Who set it.
	pub paused_by: Option<String>,
	/// Why it was set.
	pub pause_reason: Option<String>,
	/// The domains this server's group controls, so the UI can say which names
	/// are available to it at all.
	pub domains: Vec<String>,
	/// The public names this server has registered, by name.
	pub names: Vec<NameView>,
	/// Every certificate Canopy holds or has an order in flight for, newest
	/// first. A name may appear more than once — a key rotation leaves the
	/// previous certificate behind until it expires.
	pub certificates: Vec<CertificateView>,
}

fn name_view(row: ApplicationName, zones: &[commons_types::dns::ManagedZone]) -> NameView {
	NameView {
		published: row.is_reconciled(),
		addresses: row.wanted().iter().map(|a| a.to_string()).collect(),
		published_addresses: row.published().iter().map(|a| a.to_string()).collect(),
		zone: match_zone(&row.name, zones).map(|z| z.apex.clone()),
		id: row.id,
		name: row.name,
		published_at: row.published_at,
		last_error: row.last_error,
	}
}

fn certificate_view(cert: ApplicationCertificate) -> CertificateView {
	use database::application_certificates::Risk;
	CertificateView {
		remaining_seconds: cert.remaining().map(|d| d.as_secs()),
		collectable: cert.is_collectable(),
		risk: match cert.risk() {
			Risk::None => "none",
			Risk::AtRisk => "at_risk",
			Risk::Critical => "critical",
		}
		.to_string(),
		id: cert.id,
		name: cert.name,
		state: cert.state,
		profile: cert.profile,
		not_after: cert.not_after,
		issued_at: cert.issued_at,
		renewing: cert.renewing,
		attempts: cert.attempts,
		last_error: cert.last_error,
		revoked_at: cert.revoked_at,
		revoked_by: cert.revoked_by,
		revocation_reason: cert.revocation_reason,
		key_fingerprint: cert.key_fingerprint,
	}
}

/// Everything a server's page needs about its names and certificates.
///
/// One call rather than several, because the parts are read together and a
/// half-loaded panel would show a certificate without the pause that explains
/// why it is not renewing.
#[utoipa::path(
	post,
	path = "/for_server",
	operation_id = "certificates_for_server",
	tag = "certificates",
	security(("tailscale-user" = [])),
	request_body = ServerIdArgs,
	responses(
		(status = 200, body = ApplicationNamesView),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn for_server(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ServerIdArgs>,
) -> Result<Json<ApplicationNamesView>> {
	let mut conn = state.db_read.get().await?;
	let server = Application::get_by_id(&mut conn, args.server_id).await?;

	let domains = match server.group_id {
		Some(group) => ServerGroupDomain::list_for_group(&mut conn, group)
			.await?
			.into_iter()
			.map(|claim| claim.domain)
			.collect(),
		None => Vec::new(),
	};
	let names = ApplicationName::for_server(&mut conn, args.server_id).await?;
	let certificates = ApplicationCertificate::for_server(&mut conn, args.server_id).await?;

	Ok(Json(ApplicationNamesView {
		may_manage_dns: server.may_manage_dns,
		may_manage_tls: server.may_manage_tls,
		paused: server.name_management_paused(),
		certificate_profile: server.certificate_profile,
		paused_at: server.name_management_paused_at,
		paused_by: server.name_management_paused_by,
		pause_reason: server.name_management_pause_reason,
		domains,
		names: names
			.into_iter()
			.map(|row| name_view(row, &state.dns_zones))
			.collect(),
		certificates: certificates.into_iter().map(certificate_view).collect(),
	}))
}

/// One name in use beneath a group's domain, with whether it is covered.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DomainNameView {
	/// The name.
	pub name: String,
	/// The server that registered it or holds its certificate.
	pub server_id: Uuid,
	/// That server's name, for display.
	pub server_name: Option<String>,
	/// Whether the address records Canopy publishes for it are up to date. Null
	/// where the name has no registration — a certificate obtained for a name
	/// whose addresses the server publishes itself.
	pub published: Option<bool>,
	/// Whether a certificate Canopy holds for it is current and collectable.
	pub certificate: bool,
	/// How urgently that certificate needs attention: `none`, `at_risk`, or
	/// `critical`. Null where there is no certificate.
	pub risk: Option<String>,
	/// When the certificate expires.
	#[schema(value_type = Option<String>)]
	pub not_after: Option<Timestamp>,
}

/// The names in use beneath one of a group's domains.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DomainHealthView {
	/// The claimed domain these names sit beneath.
	pub domain: String,
	/// The names in use beneath it, by name.
	pub names: Vec<DomainNameView>,
}

/// The names in use under each domain a group controls, and which of them hold a
/// current certificate.
///
/// So that whether a group's names are healthy is answerable from the
/// group's page, without visiting each of its applications.
// spec: CRT#presentation
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "certificates_for_group",
	tag = "certificates",
	security(("tailscale-user" = [])),
	request_body = GroupIdArgs,
	responses((status = 200, body = Vec<DomainHealthView>)),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<GroupIdArgs>,
) -> Result<Json<Vec<DomainHealthView>>> {
	use database::application_certificates::Risk;
	use std::collections::BTreeMap;

	let mut conn = state.db_read.get().await?;
	let claims = ServerGroupDomain::list_for_group(&mut conn, args.server_group_id).await?;
	if claims.is_empty() {
		return Ok(Json(Vec::new()));
	}

	let applications = Application::list_live_in_group(&mut conn, args.server_group_id).await?;

	// Gathered per name across both registrations and certificates: a name may
	// have one, the other, or both, and the group's view is of names rather than
	// of either table.
	let mut rows: BTreeMap<String, DomainNameView> = BTreeMap::new();
	for server in &applications {
		for row in ApplicationName::for_server(&mut conn, server.id).await? {
			rows.entry(row.name.clone())
				.or_insert_with(|| DomainNameView {
					name: row.name.clone(),
					server_id: server.id,
					server_name: Some(server.display_name()),
					published: None,
					certificate: false,
					risk: None,
					not_after: None,
				})
				.published = Some(row.is_reconciled());
		}
		for cert in ApplicationCertificate::for_server(&mut conn, server.id).await? {
			let entry = rows
				.entry(cert.name.clone())
				.or_insert_with(|| DomainNameView {
					name: cert.name.clone(),
					server_id: server.id,
					server_name: Some(server.display_name()),
					published: None,
					certificate: false,
					risk: None,
					not_after: None,
				});
			// The newest usable certificate wins where a name has more than one —
			// a key rotation leaves the old row behind, and the group's view is of
			// whether the name is covered rather than of every attempt.
			if cert.is_collectable() && !entry.certificate {
				entry.certificate = true;
			}
			if entry.not_after.is_none() || cert.not_after > entry.not_after {
				entry.not_after = cert.not_after;
				entry.risk = Some(
					match cert.risk() {
						Risk::None => "none",
						Risk::AtRisk => "at_risk",
						Risk::Critical => "critical",
					}
					.to_string(),
				);
			}
		}
	}

	Ok(Json(
		claims
			.into_iter()
			.map(|claim| DomainHealthView {
				names: rows
					.values()
					.filter(|row| is_within(&row.name, &claim.domain))
					.cloned()
					.collect(),
				domain: claim.domain,
			})
			.collect(),
	))
}

/// The certificate authority Canopy is configured to use, and whether it works.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct AuthorityView {
	/// The authority's directory URL, or null where none is configured — in
	/// which case Canopy issues no certificates at all.
	pub directory: Option<String>,
	/// The profiles the authority advertises, as it names them. Empty means it
	/// advertises none, so asking for one would be refused.
	pub profiles: Vec<String>,
	/// Whether Canopy holds a usable account at the authority. False where none
	/// is configured, or where the last attempt to use it failed.
	pub account_usable: bool,
	/// What is currently wrong, where anything is: the message from the standing
	/// self-alert, so the settings panel says the same thing as the alerting.
	pub problem: Option<String>,
}

/// The authority, its profiles, and whether Canopy's account with it is usable.
///
/// Presented to operators because a misconfiguration of issuance shows up here
/// rather than on any one server.
// spec: CRT#presentation
#[utoipa::path(
	post,
	path = "/authority",
	operation_id = "certificates_authority",
	tag = "certificates",
	security(("tailscale-user" = [])),
	responses((status = 200, body = AuthorityView)),
)]
pub async fn authority(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<AuthorityView>> {
	use database::self_alerts::{CA_ACCOUNT_REF, CA_THROTTLED_REF, CA_UNREACHABLE_REF, current};

	let mut conn = state.db_read.get().await?;

	// The standing alerts are the truth about whether issuance works: the domains
	// pod is what actually talks to the authority, and it reports what it finds.
	let mut problem = None;
	for r#ref in [CA_UNREACHABLE_REF, CA_ACCOUNT_REF, CA_THROTTLED_REF] {
		if let Some(issue) = current(&mut conn, r#ref).await?
			&& issue.active
		{
			problem = Some(issue.message);
			break;
		}
	}

	Ok(Json(AuthorityView {
		directory: state.acme_directory.clone(),
		profiles: state
			.acme
			.as_ref()
			.map(|acme| acme.profiles())
			.unwrap_or_default(),
		account_usable: state.acme.is_some() && problem.is_none(),
		problem,
	}))
}

/// The profile a server's certificates are requested under.
#[derive(Debug, Deserialize, ToSchema)]
pub struct SetProfileArgs {
	/// The server to set.
	pub server_id: Uuid,
	/// The profile, as the authority names it, or null for the authority's own
	/// default — its longest-lived, which is what a server takes until an
	/// operator says otherwise.
	pub profile: Option<String>,
}

/// Set the profile a server's certificates are requested under.
///
/// Lifetime is a property of how an application is run rather than of Canopy, so it
/// is an operator's choice per server: a cloud-hosted application whose issuance is
/// exercised constantly can carry a short lifetime where an on-premises one that
/// may be offline for days cannot. Takes effect on the next issuance or renewal;
/// a certificate already held keeps the lifetime it was issued with.
///
/// Responds 409 for a profile the authority does not advertise.
// spec: CRT#lifetime
#[utoipa::path(
	post,
	path = "/set_profile",
	operation_id = "certificates_set_profile",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = SetProfileArgs,
	responses(
		(status = 200),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "The authority does not offer that profile.", body = ProblemDetailsSchema),
	),
)]
pub async fn set_profile(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<SetProfileArgs>,
) -> Result<Json<()>> {
	if let Some(profile) = &args.profile {
		let offered = state
			.acme
			.as_ref()
			.map(|acme| acme.profiles())
			.unwrap_or_default();
		if !offered.iter().any(|name| name == profile) {
			return Err(AppError::Conflict(format!(
				"the authority does not offer a {profile:?} profile (it offers {})",
				if offered.is_empty() {
					"none".to_string()
				} else {
					offered.join(", ")
				}
			)));
		}
	}

	let mut conn = state.db.get().await?;
	Application::set_certificate_profile(&mut conn, args.server_id, args.profile.as_deref())
		.await?;
	Ok(Json(()))
}

/// Why a server is being paused.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PauseArgs {
	/// The server to pause.
	pub server_id: Uuid,
	/// Why, recorded so whoever finds the pause later knows what it was for.
	pub reason: String,
}

/// Pause a server: Canopy makes no new changes on its behalf.
///
/// Nothing already in place is withdrawn — records published stand, certificates
/// held stay held and collectable until they expire, and the group keeps
/// working exactly as it did. What stops is Canopy doing anything *new*.
///
/// A second pause leaves the first in place, so the original reason and time are
/// not overwritten by a later one.
// spec: CRT#pausing-a-server
#[utoipa::path(
	post,
	path = "/pause",
	operation_id = "certificates_pause",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = PauseArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn pause(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<PauseArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Application::pause_name_management(&mut conn, args.server_id, Some(&admin.login), &args.reason)
		.await?;
	Ok(Json(()))
}

/// Lift a server's pause. Work resumes where it left off.
///
/// Only an operator can do this: Canopy never lifts a pause itself, however long
/// it has been in place and however much is expiring under it.
// spec: CRT#pausing-a-server
#[utoipa::path(
	post,
	path = "/resume",
	operation_id = "certificates_resume",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = ServerIdArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn resume(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ServerIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Application::resume_name_management(&mut conn, args.server_id).await?;
	Ok(Json(()))
}

/// Which certificate to revoke, and why.
#[derive(Debug, Deserialize, ToSchema)]
#[schema(as = CertificateRevokeArgs)]
pub struct RevokeArgs {
	/// The certificate to revoke.
	pub id: Uuid,
	/// The reason to give the authority. `key_compromise` additionally bars that
	/// key from ever being certified again, for any name by any server.
	pub reason: RevocationReason,
}

/// Revoke a certificate Canopy holds.
///
/// Canopy holds the account that obtained it, which is authority enough; the
/// server's private key is not needed and is not asked for. The authority is told
/// first and Canopy records the revocation only once it has accepted, so the two
/// never disagree — a 502 means nothing was revoked and the operator can try
/// again.
///
/// Revoking pauses the server, without being asked. Revocation and re-issuance
/// would otherwise chase each other: a key revoked as compromised has its
/// replacement requested within minutes by an agent doing exactly what it was
/// built to do, and if the key leaked because the host was compromised, that
/// replacement hands the same attacker a fresh certificate.
///
/// Cannot be undone: a revoked certificate stays revoked, and the remedy is a new
/// one.
// spec: CRT#revocation
#[utoipa::path(
	post,
	path = "/revoke",
	operation_id = "certificates_revoke",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = RevokeArgs,
	responses(
		(status = 200),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "There is no chain to revoke, or it is revoked already.", body = ProblemDetailsSchema),
		(status = 502, description = "The authority would not accept the revocation; nothing was changed.", body = ProblemDetailsSchema),
	),
)]
pub async fn revoke(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<RevokeArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	let cert = ApplicationCertificate::get(&mut conn, args.id).await?;

	let Some(chain) = cert.chain.as_deref() else {
		return Err(AppError::Conflict(format!(
			"there is no certificate for {} to revoke yet — the order has produced nothing",
			cert.name
		)));
	};
	if cert.is_revoked() {
		return Err(AppError::Conflict(format!(
			"the certificate for {} is already revoked",
			cert.name
		)));
	}

	let Some(acme) = state.acme.as_ref() else {
		return Err(AppError::Upstream(
			"Canopy has no certificate authority configured, so it cannot revoke this certificate. \
			 The certificate stays as it is."
				.into(),
		));
	};

	// The authority first. Recording a revocation the authority did not accept
	// would leave Canopy refusing to serve a certificate that clients still trust
	// — the worst of both.
	acme.revoke(chain, RevokeFor::from_stored(args.reason.as_str()))
		.await?;

	ApplicationCertificate::record_revoked(&mut conn, args.id, args.reason, Some(&admin.login))
		.await?;
	Ok(Json(()))
}

/// An application and the name being declared for it, or released from it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DeclarationArgs {
	/// The application that serves the name.
	pub application_id: Uuid,
	/// The name, in any case and with or without a trailing dot.
	pub name: String,
}

/// Declare that an application serves a name.
///
/// A declaration is what an address registration or a certificate request from
/// the machine is resolved against, so it is how a box running several workloads
/// gets its requests routed to the right one. It carries no addresses; the
/// application registers those itself.
///
/// Declaring a name the same application already holds changes nothing. A name
/// another application holds is refused, and the refusal names the holder so an
/// operator can see what to release first.
// spec: CRT#declared-names
#[utoipa::path(
	post,
	path = "/declare",
	operation_id = "certificates_declare",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = DeclarationArgs,
	responses(
		(status = 200, body = NameView),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "Another application already declares this name.", body = ProblemDetailsSchema),
	),
)]
pub async fn declare(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeclarationArgs>,
) -> Result<Json<NameView>> {
	let mut conn = state.db.get().await?;
	let row = ApplicationName::declare(&mut conn, args.application_id, &args.name).await?;
	Ok(Json(name_view(row, &state.dns_zones)))
}

/// End an application's hold on a name.
///
/// What is already in place stands, as revoking a grant leaves it: the records
/// published stay published and the certificates held stay held until they
/// expire. What ends is Canopy treating the name as this application's, which
/// frees it to be declared elsewhere.
// spec: CRT#declared-names
#[utoipa::path(
	post,
	path = "/release",
	operation_id = "certificates_release",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = DeclarationArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn release(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeclarationArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	ApplicationName::release(&mut conn, args.application_id, &args.name).await?;
	Ok(Json(()))
}
