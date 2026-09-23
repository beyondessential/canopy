//! Device-facing name and certificate endpoints (CRT).
//!
//! A server reaches these for the two things it cannot do for itself about its
//! own public name: publishing the address records that make it resolve, and
//! obtaining a certificate for it. Both are confined to names within the domains
//! its group controls, and both need the matching grant.
//!
//! Every refusal is distinguishable by problem type rather than only by prose, so
//! an agent can tell being unentitled from being paused from lacking the grant,
//! and act accordingly without a human reading the message.
// spec: CRT

use std::net::IpAddr;

use axum::extract::State;
use axum::{Json, http::StatusCode};
use base64::Engine;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::csr::validate_csr;
use commons_servers::device_auth::ServerDevice;
use commons_types::Uuid;
use commons_types::dns::{ManagedZone, is_within, match_zone, normalize_domain};
use commons_types::server::app_type::ApplicationType;
use database::application_certificates::OrderState;
use database::diesel_async::AsyncPgConnection;
use database::{
	ApplicationCertificate, ApplicationName, ServerGroupDomain, applications::Application,
};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

/// Mounted at `/names`.
pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(public: entitlements))
		.routes(routes!(public: register_name))
}

/// Mounted at `/certificates`.
pub fn certificate_routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(public: request_certificate))
}

/// Which grant a request needs.
#[derive(Debug, Clone, Copy)]
enum Grant {
	Dns,
	Tls,
}

impl Grant {
	fn held_by(self, server: &Application) -> bool {
		match self {
			Self::Dns => server.may_manage_dns,
			Self::Tls => server.may_manage_tls,
		}
	}

	fn describe(self) -> &'static str {
		match self {
			Self::Dns => "manage its own DNS records",
			Self::Tls => "obtain its own TLS certificates",
		}
	}
}

/// The server a request is for, having passed every check in CRT's fixed order.
struct Authorised {
	server: Application,
	name: String,
}

/// Resolve and authorise a request, reporting each failure distinctly.
///
/// The order is the one CRT fixes, and each step has its own problem type so a
/// misconfiguration is diagnosable from the refusal alone rather than by reading
/// the message.
// spec: CRT#identity-and-authorisation
async fn authorise(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
	name: &str,
	grant: Grant,
	zones: &[ManagedZone],
) -> Result<Authorised> {
	let name = normalize_domain(name)?;

	// 1. An identity belonging to a live machine. An identity is the box's, not
	// the software's, so the credential says which machine is asking and
	// nothing about which workload the request concerns.
	let machine = database::machines::Machine::get_by_device_id(conn, device_id)
		.await?
		.filter(|m| m.deleted_at.is_none())
		.ok_or(AppError::DeviceHasNoServer)?;
	let on_machine = machine.applications(conn).await?;

	// 2. The application on that machine declaring the requested name. Which
	// application a request concerns is resolved from the name, not from the
	// credential, and a name is held by one application fleet-wide, so this is
	// unambiguous however many workloads the box hosts.
	//
	// A machine hosting exactly one application resolves to it even for a name
	// nothing declares yet, because there is nothing to disambiguate and the
	// agent's own registration is still how a name first gets declared. The
	// moment a box hosts two, an undeclared name is genuinely ambiguous and is
	// refused rather than guessed at.
	//
	// TRAP: this refusal must not distinguish "declared by an application
	// elsewhere" from "declared by nobody". The fleet-wide unique index makes
	// the former cheap to detect, which is exactly the temptation; reporting it
	// would turn this endpoint into a directory of what other machines serve.
	// spec: CRT#identity-and-authorisation
	let declared = ApplicationName::for_name(conn, &name).await?;
	let server = match declared {
		Some(row) => on_machine.into_iter().find(|a| a.id == row.application_id),
		None if on_machine.len() == 1 => on_machine.into_iter().next(),
		None => None,
	}
	.ok_or_else(|| {
		AppError::NameNotEntitled(format!("no application on this machine declares {name}"))
	})?;

	// 3. Paused before grants: a paused server is being looked into, and telling
	// it about a missing grant would send an operator chasing the wrong thing.
	if server.name_management_paused() {
		return Err(AppError::NameManagementPaused(format!(
			"paused since {}{}",
			server
				.name_management_paused_at
				.map(|at| at.to_string())
				.unwrap_or_else(|| "an unknown time".into()),
			server
				.name_management_pause_reason
				.as_deref()
				.map(|r| format!(": {r}"))
				.unwrap_or_default(),
		)));
	}

	// 4. The grant this request needs.
	if !grant.held_by(&server) {
		return Err(AppError::AuthInsufficientPermissions {
			required: format!(
				"permission for this server to {} (an operator grants it in Canopy)",
				grant.describe()
			),
		});
	}

	// 5. The name has to sit under a domain this application's *own* group controls.
	// A name another group controls is refused exactly as an unclaimed one is, so
	// the endpoint is not a directory of other groups' names.
	let entitled = match server.group_id {
		None => false,
		Some(group) => ServerGroupDomain::list_for_group(conn, group)
			.await?
			.iter()
			.any(|claim| is_within(&name, &claim.domain)),
	};
	if !entitled {
		return Err(AppError::NameNotEntitled(format!(
			"{name} is not within any domain this server's group controls"
		)));
	}

	// 6. And Canopy has to be able to act on it at all.
	if match_zone(&name, zones).is_none() {
		return Err(AppError::Conflict(format!(
			"no DNS zone Canopy manages covers {name}, so it can publish nothing there; this is a \
			 Canopy configuration problem rather than anything this server can fix"
		)));
	}

	Ok(Authorised { server, name })
}

// ── What a server may act on ────────────────────────────────────────────────

/// What a server is entitled to do with names, and what it already holds.
#[derive(Debug, Clone, Default, Serialize, Deserialize, ToSchema)]
pub struct Entitlements {
	/// Whether this server may manage its own DNS records.
	pub may_manage_dns: bool,
	/// Whether this server may obtain its own TLS certificates.
	pub may_manage_tls: bool,
	/// Whether Canopy is currently making no new changes on this server's
	/// behalf. While true, requests are refused and an agent should wait.
	pub paused: bool,
	/// The domains this server's group controls. Any name at or beneath one of
	/// these is a name this server may act on — which is what lets an agent
	/// request a certificate before anything asks for one.
	pub domains: Vec<String>,
	/// The names this server has registered addresses for.
	pub registered_names: Vec<String>,
	/// The certificates Canopy holds for this server.
	pub certificates: Vec<HeldCertificate>,
	/// One entry per application on the asking machine.
	///
	/// An identity belongs to a machine, so an agent asks on behalf of the box
	/// and gets an answer for every workload on it. The flat fields above
	/// describe a single-application machine, which is every machine today;
	/// on a machine hosting several they are left at their defaults and this
	/// list is the answer.
	// spec: CRT#what-an-application-may-act-on
	#[serde(default)]
	pub applications: Vec<ApplicationEntitlements>,
}

/// What one application on the asking machine may act on.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct ApplicationEntitlements {
	/// The type of application these entitlements belong to.
	///
	/// A reporter correlates an entry to a workload it runs by the machine it
	/// asked as and this type. Canopy's own identifier for the application is
	/// internal and never on the wire.
	// spec: STA#push
	pub r#type: ApplicationType,
	/// Whether this application may manage its own DNS records.
	pub may_manage_dns: bool,
	/// Whether this application may obtain its own TLS certificates.
	pub may_manage_tls: bool,
	/// Whether Canopy is currently making no new changes on its behalf.
	pub paused: bool,
	/// The domains its group controls.
	pub domains: Vec<String>,
	/// The names it has registered addresses for.
	pub registered_names: Vec<String>,
	/// The certificates Canopy holds for it.
	pub certificates: Vec<HeldCertificate>,
}

/// A certificate Canopy holds for the asking server, as the server needs to see
/// it: enough to decide whether to renew, and nothing about anyone else.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct HeldCertificate {
	/// The name it covers.
	pub name: String,
	/// Hex SHA-256 of the certified key's subject public key info, so an agent
	/// can tell whether this covers a key it still holds.
	pub key_fingerprint: String,
	/// The profile it was issued under, if the authority named one.
	pub profile: Option<String>,
	/// When it expires.
	#[schema(value_type = Option<String>)]
	pub not_after: Option<Timestamp>,
	/// Whether it can still be served: not revoked, not expired. True even while
	/// a renewal is under way, the chain in hand staying valid until the new one
	/// lands.
	pub usable: bool,
	/// Whether an operator has revoked it. Stop serving it.
	pub revoked: bool,
	/// Whether the key itself is condemned, not just the certificate — the key
	/// pair has to be replaced before asking again.
	pub key_must_be_replaced: bool,
}

fn held(cert: &ApplicationCertificate) -> HeldCertificate {
	HeldCertificate {
		name: cert.name.clone(),
		key_fingerprint: cert.key_fingerprint.clone(),
		profile: cert.profile.clone(),
		not_after: cert.not_after,
		usable: cert.is_collectable(),
		revoked: cert.is_revoked(),
		key_must_be_replaced: cert.requires_new_key(),
	}
}

/// What this server may act on, and what it already holds.
///
/// Answers the boundary rather than making an agent discover it by being
/// refused: the domains its group controls, the grants it holds, whether it is
/// paused, and the names and certificates it already has. Enough to request a
/// certificate before anything asks for one, and to renew before expiry.
///
/// A server with no grants, or whose group controls no domain, gets an empty
/// answer rather than an error — asking what one may do is not a privileged act.
/// The same content rides on the response to a status push.
#[utoipa::path(
	get,
	path = "/entitlements",
	operation_id = "name_entitlements",
	tag = "names",
	security(("mtls-certificate" = [])),
	responses(
		(status = 200, body = Entitlements),
		(status = 412, description = "The device is not attached to any live server.", body = ProblemDetailsSchema),
	),
)]
pub async fn entitlements(
	State(state): State<AppState>,
	ServerDevice(auth): ServerDevice,
) -> Result<Json<Entitlements>> {
	let mut conn = state.db.get().await?;
	// An identity belongs to a box, so the answer is the box's: every workload
	// on it, whether that is one, several, or none.
	let machine = database::machines::Machine::get_by_device_id(&mut conn, auth.0.id)
		.await?
		.ok_or(AppError::DeviceHasNoServer)?;

	Ok(Json(
		entitlements_for(&mut conn, &machine, &state.dns_zones).await?,
	))
}

/// Build the entitlements answer for the machine `server` sits on.
///
/// Shared with the status-push response, so an agent that already reports
/// status learns of a new domain without asking. The answer carries an entry
/// per application on the box; the flat fields describe `server` itself, which
/// on a single-application machine is the whole answer.
// spec: CRT#what-an-application-may-act-on
pub async fn entitlements_for(
	conn: &mut AsyncPgConnection,
	machine: &database::machines::Machine,
	zones: &[ManagedZone],
) -> Result<Entitlements> {
	let mut applications = Vec::new();
	for application in machine.applications(conn).await? {
		applications.push(one_applications_entitlements(conn, &application, zones).await?);
	}
	// A box running exactly one workload is described by the flat fields, which
	// is what every reporter in the field reads. Running none or several, no
	// single set of them is the answer, so they stay at their defaults and the
	// list is.
	let flat = match applications.as_slice() {
		[only] => Some(only.clone()),
		_ => None,
	};
	Ok(Entitlements {
		may_manage_dns: flat.as_ref().is_some_and(|f| f.may_manage_dns),
		may_manage_tls: flat.as_ref().is_some_and(|f| f.may_manage_tls),
		paused: flat.as_ref().is_some_and(|f| f.paused),
		domains: flat.as_ref().map(|f| f.domains.clone()).unwrap_or_default(),
		registered_names: flat
			.as_ref()
			.map(|f| f.registered_names.clone())
			.unwrap_or_default(),
		certificates: flat.map(|f| f.certificates).unwrap_or_default(),
		applications,
	})
}

/// One application's own entitlements.
async fn one_applications_entitlements(
	conn: &mut AsyncPgConnection,
	server: &Application,
	zones: &[ManagedZone],
) -> Result<ApplicationEntitlements> {
	// Only domains Canopy can actually act in are offered: naming one whose zone
	// has gone would have an agent request a name that cannot be fulfilled.
	let domains: Vec<String> = match server.group_id {
		None => Vec::new(),
		Some(group) => ServerGroupDomain::list_for_group(conn, group)
			.await?
			.into_iter()
			.filter(|claim| match_zone(&claim.domain, zones).is_some())
			.map(|claim| claim.domain)
			.collect(),
	};

	let registered_names = ApplicationName::for_server(conn, server.id)
		.await?
		.into_iter()
		.map(|row| row.name)
		.collect();

	let certificates = ApplicationCertificate::for_server(conn, server.id)
		.await?
		.iter()
		.map(held)
		.collect();

	Ok(ApplicationEntitlements {
		r#type: server.r#type.clone(),
		may_manage_dns: server.may_manage_dns,
		may_manage_tls: server.may_manage_tls,
		paused: server.name_management_paused(),
		domains,
		registered_names,
		certificates,
	})
}

// ── Addresses ───────────────────────────────────────────────────────────────

/// The name a server should be reachable at, and where.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RegisterNameArgs {
	/// The name to publish records at. Must sit within a domain this server's
	/// group controls.
	pub name: String,
	/// Every external address this server is reachable at. IPv4 addresses become
	/// A records and IPv6 addresses AAAA records, replacing whatever was
	/// registered before. An empty list withdraws the name.
	#[schema(value_type = Vec<String>)]
	pub addresses: Vec<IpAddr>,
}

/// What Canopy holds for a registered name.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct RegisteredName {
	/// The name, as Canopy normalised it.
	pub name: String,
	/// The addresses Canopy will publish.
	#[schema(value_type = Vec<String>)]
	pub addresses: Vec<IpAddr>,
	/// The addresses Canopy has published so far. Differs from `addresses` until
	/// the change has been reconciled into the zone.
	#[schema(value_type = Vec<String>)]
	pub published_addresses: Vec<IpAddr>,
	/// Whether the zone has caught up with what was asked for.
	pub published: bool,
	/// Why the last publish attempt failed, if it did.
	pub last_error: Option<String>,
}

/// Register the addresses a name should resolve to.
///
/// Replaces whatever addresses were registered for the name; an empty list
/// withdraws it. Canopy publishes what it is told — it does not verify that an
/// address is really this server's, the grant being the trust boundary.
///
/// Publishing happens in the background, so the response says what Canopy will
/// publish and what it has published so far rather than waiting for the zone.
#[utoipa::path(
	post,
	path = "/register",
	operation_id = "name_register",
	tag = "names",
	security(("mtls-certificate" = [])),
	request_body = RegisterNameArgs,
	responses(
		(status = 200, body = RegisteredName),
		(status = 403, description = "The server lacks the DNS grant, or the name is not within its group's domains.", body = ProblemDetailsSchema),
		(status = 409, description = "The server is paused, another server holds the name, or no managed zone covers it.", body = ProblemDetailsSchema),
		(status = 412, description = "The device is not attached to any live server.", body = ProblemDetailsSchema),
	),
)]
pub async fn register_name(
	State(state): State<AppState>,
	ServerDevice(auth): ServerDevice,
	Json(args): Json<RegisterNameArgs>,
) -> Result<Json<RegisteredName>> {
	let mut conn = state.db.get().await?;
	let authorised = authorise(
		&mut conn,
		auth.0.id,
		&args.name,
		Grant::Dns,
		&state.dns_zones,
	)
	.await?;

	let row = ApplicationName::register(
		&mut conn,
		authorised.server.id,
		&authorised.name,
		&args.addresses,
	)
	.await?;

	Ok(Json(RegisteredName {
		name: row.name.clone(),
		addresses: row.wanted(),
		published_addresses: row.published(),
		published: row.is_reconciled(),
		last_error: row.last_error.clone(),
	}))
}

// ── Certificates ────────────────────────────────────────────────────────────

/// A request to certify a key for a name.
#[derive(Debug, Clone, Deserialize, ToSchema)]
pub struct RequestCertificateArgs {
	/// The name to certify. Must sit within a domain this server's group
	/// controls.
	pub name: String,
	/// The certificate signing request, DER, base64. Must ask for exactly `name`
	/// and nothing else — a request carrying any other name is refused rather
	/// than trimmed.
	pub csr: String,
}

/// Where a certificate request stands, and the chain once there is one.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CertificateResponse {
	/// The name the certificate is (or will be) for, as Canopy normalised it.
	pub name: String,
	/// `pending`, `issued`, `failed`, or `revoked`.
	pub state: String,
	/// The chain, PEM, once Canopy holds one — including while a renewal is
	/// under way, the chain in hand staying valid until the new one lands.
	pub chain: Option<String>,
	/// The profile it was issued under, if the authority named one.
	pub profile: Option<String>,
	/// When it expires.
	#[schema(value_type = Option<String>)]
	pub not_after: Option<Timestamp>,
	/// Whether the chain can be served now.
	pub usable: bool,
	/// Whether an operator revoked it. Stop serving it and ask again.
	pub revoked: bool,
	/// Whether the key must be replaced before asking again, rather than just the
	/// certificate.
	pub key_must_be_replaced: bool,
	/// Why the last attempt failed, if one did. Present while Canopy is still
	/// retrying.
	pub last_error: Option<String>,
}

fn certificate_response(cert: &ApplicationCertificate) -> CertificateResponse {
	CertificateResponse {
		name: cert.name.clone(),
		state: cert.state.clone(),
		// Only hand over a chain that is actually servable.
		chain: cert.is_collectable().then(|| cert.chain.clone()).flatten(),
		profile: cert.profile.clone(),
		not_after: cert.not_after,
		usable: cert.is_collectable(),
		revoked: cert.is_revoked(),
		key_must_be_replaced: cert.requires_new_key(),
		last_error: cert.last_error.clone(),
	}
}

/// Ask for a certificate, and collect it once there is one.
///
/// The same call does both, and is safe to repeat: a name and key Canopy already
/// holds a certificate for is answered from what it holds rather than ordering
/// again, so a server that lost its local copy costs the authority nothing. A
/// request naming a different key opens a new order.
///
/// Proving control of a name through DNS takes far longer than any client waits
/// mid-handshake, so a first request records the order and answers `pending`;
/// call again to collect. A server is expected to hold a certificate before it
/// needs one rather than to obtain one while a client waits.
#[utoipa::path(
	post,
	path = "/request",
	operation_id = "certificate_request",
	tag = "certificates",
	security(("mtls-certificate" = [])),
	request_body = RequestCertificateArgs,
	responses(
		(status = 200, description = "The order as it stands, with the chain if there is one.", body = CertificateResponse),
		(status = 400, description = "The signing request is unparseable, unsigned, asks for another name, or carries a name besides the one requested.", body = ProblemDetailsSchema),
		(status = 403, description = "The server lacks the TLS grant, or the name is not within its group's domains.", body = ProblemDetailsSchema),
		(status = 409, description = "The server is paused, no managed zone covers the name, or the key was revoked as compromised and will not be certified again.", body = ProblemDetailsSchema),
		(status = 412, description = "The device is not attached to any live server.", body = ProblemDetailsSchema),
	),
)]
pub async fn request_certificate(
	State(state): State<AppState>,
	ServerDevice(auth): ServerDevice,
	Json(args): Json<RequestCertificateArgs>,
) -> Result<(StatusCode, Json<CertificateResponse>)> {
	let mut conn = state.db.get().await?;
	let authorised = authorise(
		&mut conn,
		auth.0.id,
		&args.name,
		Grant::Tls,
		&state.dns_zones,
	)
	.await?;

	let der = base64::engine::general_purpose::STANDARD
		.decode(args.csr.trim())
		.map_err(|e| {
			AppError::BadRequest(format!("the signing request is not valid base64: {e}"))
		})?;
	// Checked against the name Canopy authorised, not the one the body asked
	// for, so a normalisation difference can't slip a different name through.
	let csr = validate_csr(&der, &authorised.name)?;

	let cert = ApplicationCertificate::request(
		&mut conn,
		authorised.server.id,
		&csr.name,
		&csr.key_fingerprint,
		&csr.der,
	)
	.await?;

	// 202 while there is nothing to collect yet, so an agent can tell "come
	// back" from "here it is" without inspecting the body.
	let status = if cert.is_collectable() {
		StatusCode::OK
	} else if cert.order_state() == OrderState::Pending {
		StatusCode::ACCEPTED
	} else {
		StatusCode::OK
	};

	Ok((status, Json(certificate_response(&cert))))
}
