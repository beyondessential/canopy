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
	ApplicationCertificate, ApplicationName, AskedFor, DeniedDnsName, ServerGroupDomain,
	UndeclaredDnsName, applications::Application,
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

	fn asked_for(self) -> AskedFor {
		match self {
			Self::Dns => AskedFor::Addresses,
			Self::Tls => AskedFor::Certificate,
		}
	}

	fn describe(self) -> &'static str {
		match self {
			Self::Dns => "manage its own DNS records",
			Self::Tls => "obtain its own TLS certificates",
		}
	}
}

/// The live machine an identity belongs to: CRT's first check.
///
/// An identity is the box's, not the software's, so the credential says which
/// machine is asking and nothing about which workload the request concerns.
// spec: CRT#identity-and-authorisation
async fn asking_machine(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
) -> Result<database::machines::Machine> {
	database::machines::Machine::get_by_device_id(conn, device_id)
		.await?
		.filter(|m| m.deleted_at.is_none())
		.ok_or(AppError::DeviceHasNoServer)
}

/// Resolve and authorise a request about `name` from `machine`, reporting each
/// failure distinctly.
///
/// The order is the one CRT fixes, and each step has its own problem type so a
/// misconfiguration is diagnosable from the refusal alone rather than by reading
/// the message. `name` is already normalised.
// spec: CRT#identity-and-authorisation
async fn authorise(
	conn: &mut AsyncPgConnection,
	machine: &database::machines::Machine,
	name: &str,
	named_type: Option<&ApplicationType>,
	grant: Grant,
	zones: &[ManagedZone],
) -> Result<Application> {
	// A denial is an operator's decision about this box and this DNS name, so it
	// holds however the request would otherwise resolve. The note is for
	// operators and stays in Canopy.
	// spec: CRT#denied-dns-names
	if DeniedDnsName::get(conn, machine.id, name).await?.is_some() {
		return Err(AppError::DnsNameDenied(name.to_owned()));
	}

	// 2. The one application on the machine the request is about.
	let server = resolve(conn, machine, name, named_type, grant)
		.await?
		.ok_or_else(|| AppError::DnsNameUndeclared(name.to_owned()))?;

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
	if !group_covers(conn, &server, name).await? {
		return Err(AppError::NameNotEntitled(format!(
			"{name} is not within any domain this server's group controls"
		)));
	}

	// 6. And Canopy has to be able to act on it at all.
	if match_zone(name, zones).is_none() {
		return Err(AppError::Conflict(format!(
			"no DNS zone Canopy manages covers {name}, so it can publish nothing there; this is a \
			 Canopy configuration problem rather than anything this server can fix"
		)));
	}

	Ok(server)
}

/// Keep the machine's undeclared record in step with how its request ended.
///
/// Settled on the request's final outcome rather than inside [`authorise`],
/// because declaring the DNS name, which comes after, is itself what refuses a
/// DNS name another application holds. A request accepted ends the record;
/// one refused as undeclared, wherever, is recorded.
///
/// The record is for operators, not part of the answer, so failing to keep it
/// is logged and the request's own outcome stands: an undeclared refusal stays
/// distinguishable, and a request already carried out is not reported failed.
// spec: CRT#undeclared-requests
async fn settle_undeclared<T>(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	name: &str,
	grant: Grant,
	outcome: &Result<T>,
) {
	let kept = match outcome {
		Ok(_) => UndeclaredDnsName::clear(conn, machine_id, name).await,
		Err(AppError::DnsNameUndeclared(_)) => {
			UndeclaredDnsName::record(conn, machine_id, name, grant.asked_for()).await
		}
		Err(_) => Ok(()),
	};
	if let Err(err) = kept {
		tracing::warn!(%machine_id, dns_name = name, "keeping the undeclared record failed: {err}");
	}
}

/// Which application on `machine` a request about `name` concerns, if exactly
/// one.
///
/// A named type the machine contradicts is refused first: one other than the
/// type of the application on it declaring the name, or one none of its
/// applications is. Then it starts from every application on the machine and
/// narrows, stopping as soon as one remains: to the one declaring the name, to
/// the type the request named, then to those holding the grant whose group
/// covers the name. A machine hosting one application is resolved before
/// anything narrows, so its requests reach the grant and domain checks and are
/// refused as what they are.
///
/// TRAP: a name declared by an application on another machine must narrow
/// exactly as a name nobody declares does. The fleet-wide unique index makes
/// the former cheap to detect, which is exactly the temptation; acting on it
/// here would let an agent tell the two apart by which refusal it gets, making
/// this endpoint a directory of what other machines serve. Declaring the name
/// is what refuses it, as undeclared, and only once every earlier check passed.
// spec: CRT#resolving-the-application
async fn resolve(
	conn: &mut AsyncPgConnection,
	machine: &database::machines::Machine,
	name: &str,
	named_type: Option<&ApplicationType>,
	grant: Grant,
) -> Result<Option<Application>> {
	let mut candidates = machine.applications(conn).await?;

	let declaring = match ApplicationName::for_name(conn, name).await? {
		Some(declared) => candidates
			.iter()
			.position(|a| a.id == declared.application_id),
		None => None,
	};

	// The machine's own business, already in its entitlements: following the
	// request quietly would serve the agent a certificate attributed to the
	// workload it said it was not, and declare the name for that workload.
	if let Some(named) = named_type {
		if let Some(at) = declaring
			&& *named != candidates[at].r#type
		{
			return Err(AppError::DnsNameTypeMismatch(format!(
				"{name} is declared by this machine's {} application, not a {named} one",
				candidates[at].r#type
			)));
		}
		if !candidates.iter().any(|a| a.r#type == *named) {
			let hosted = candidates
				.iter()
				.map(|a| a.r#type.to_string())
				.collect::<Vec<_>>();
			return Err(AppError::DnsNameTypeMismatch(if hosted.is_empty() {
				format!("this machine has no {named} application, nor any other")
			} else {
				format!(
					"this machine has no {named} application; its applications are {}",
					hosted.join(", ")
				)
			}));
		}
	}

	if let Some(at) = declaring {
		return Ok(Some(candidates.swap_remove(at)));
	}

	if candidates.len() == 1 {
		return Ok(candidates.pop());
	}

	if let Some(named) = named_type {
		candidates.retain(|a| a.r#type == *named);
		if candidates.len() == 1 {
			return Ok(candidates.pop());
		}
	}

	let mut eligible = Vec::new();
	for candidate in candidates {
		if grant.held_by(&candidate) && group_covers(conn, &candidate, name).await? {
			eligible.push(candidate);
		}
	}
	Ok(if eligible.len() == 1 {
		eligible.pop()
	} else {
		None
	})
}

/// Whether `name` lies at or beneath a domain `server`'s own group controls.
async fn group_covers(
	conn: &mut AsyncPgConnection,
	server: &Application,
	name: &str,
) -> Result<bool> {
	Ok(match server.group_id {
		None => false,
		Some(group) => ServerGroupDomain::list_for_group(conn, group)
			.await?
			.iter()
			.any(|claim| is_within(name, &claim.domain)),
	})
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
	/// The type of the application on this machine the name is for, where the
	/// machine hosts several and the agent knows which serves it. Unneeded once
	/// the name is declared, and on a machine hosting one application.
	// spec: CRT#resolving-the-application
	#[serde(default)]
	pub application_type: Option<ApplicationType>,
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
		(status = 403, description = "The server lacks the DNS grant, the name is not within its group's domains, the request resolves to no single application on the machine, or the name is denied to the machine.", body = ProblemDetailsSchema),
		(status = 409, description = "The server is paused, no managed zone covers the name, or the request names an application type this machine contradicts.", body = ProblemDetailsSchema),
		(status = 412, description = "The device is not attached to any live server.", body = ProblemDetailsSchema),
	),
)]
pub async fn register_name(
	State(state): State<AppState>,
	ServerDevice(auth): ServerDevice,
	Json(args): Json<RegisterNameArgs>,
) -> Result<Json<RegisteredName>> {
	let mut conn = state.db.get().await?;
	let machine = asking_machine(&mut conn, auth.0.id).await?;
	let name = normalize_domain(&args.name)?;

	let outcome = async {
		let server = authorise(
			&mut conn,
			&machine,
			&name,
			args.application_type.as_ref(),
			Grant::Dns,
			&state.dns_zones,
		)
		.await?;
		ApplicationName::register(&mut conn, server.id, &name, &args.addresses).await
	}
	.await;
	settle_undeclared(&mut conn, machine.id, &name, Grant::Dns, &outcome).await;
	let row = outcome?;

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
	/// The type of the application on this machine the name is for, where the
	/// machine hosts several and the agent knows which serves it. Unneeded once
	/// the name is declared, and on a machine hosting one application.
	// spec: CRT#resolving-the-application
	#[serde(default)]
	pub application_type: Option<ApplicationType>,
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
		(status = 403, description = "The server lacks the TLS grant, the name is not within its group's domains, the request resolves to no single application on the machine, or the name is denied to the machine.", body = ProblemDetailsSchema),
		(status = 409, description = "The server is paused, no managed zone covers the name, the request names an application type this machine contradicts, or the key was revoked as compromised and will not be certified again.", body = ProblemDetailsSchema),
		(status = 412, description = "The device is not attached to any live server.", body = ProblemDetailsSchema),
	),
)]
pub async fn request_certificate(
	State(state): State<AppState>,
	ServerDevice(auth): ServerDevice,
	Json(args): Json<RequestCertificateArgs>,
) -> Result<(StatusCode, Json<CertificateResponse>)> {
	let mut conn = state.db.get().await?;
	let machine = asking_machine(&mut conn, auth.0.id).await?;
	let name = normalize_domain(&args.name)?;

	let outcome = async {
		let server = authorise(
			&mut conn,
			&machine,
			&name,
			args.application_type.as_ref(),
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
		let csr = validate_csr(&der, &name)?;

		ApplicationCertificate::request(
			&mut conn,
			server.id,
			&csr.name,
			&csr.key_fingerprint,
			&csr.der,
		)
		.await
	}
	.await;
	settle_undeclared(&mut conn, machine.id, &name, Grant::Tls, &outcome).await;
	let cert = outcome?;

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
