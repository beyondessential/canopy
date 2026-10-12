//! Operator-facing certificate endpoints (private-server, admin SPA).
//!
//! The DNS names an application holds for addresses are the `dns_names` module's;
//! this is the certificate side, with its own declarations, undeclared records,
//! and denials. What the two share, such as the pause, is in `dns_names`.
//!
//! Reads are open to any tailnet user; anything that changes what Canopy will do
//! on a server's behalf (the profile, a revocation) requires admin.
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
use commons_types::dns::is_within;
use database::application_certificates::{ApplicationCertificate, RevocationReason};
use database::applications::Application;
use database::{ApplicationCertificateName, DnsNameKind, ServerGroupDomain};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::fns::applications::ServerIdArgs;
use crate::fns::dns_names::{
	DeclarationArgs, DeniedView, DenyArgs, DomainCertificateView, DomainHealthView, DomainNameView,
	MachineDeclaredView, MachineDnsNameArgs, MachineNamesView, PauseView, by_domain, by_id,
	group_domains, ids, machine_names, pause_view,
};
use crate::fns::server_groups::GroupIdArgs;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: for_server))
		.routes(routes!(read_only: for_group))
		.routes(routes!(read_only: authority))
		.routes(routes!(write: set_profile))
		.routes(routes!(danger(fleet, invalidates): revoke))
		.routes(routes!(write: declare))
		.routes(routes!(write: release))
		.routes(routes!(read_only: for_machine))
		.routes(routes!(write: deny))
		.routes(routes!(write: lift_denial))
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

/// A DNS name an application holds for certificates.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CertificateNameView {
	/// Unique identifier of the declaration.
	pub id: Uuid,
	/// The DNS name, normalised.
	pub name: String,
	/// Whether the DNS name lies at or beneath a domain the application's group
	/// controls. An operator may declare one that does not, ahead of the group
	/// claiming its domain; nothing is certified for it until then.
	// spec: DNS#on-an-application
	pub within_domains: bool,
}

/// What an application's TLS certificates section shows.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct CertificatesView {
	/// Whether an operator has allowed this application to obtain its own
	/// certificates.
	pub may_manage_tls: bool,
	/// The profile this application's certificates are requested under. Null
	/// means the authority's own default, which is its longest-lived.
	pub certificate_profile: Option<String>,
	/// The pause on this application, if it has one.
	pub pause: Option<PauseView>,
	/// The domains this application's group controls, so the UI can say which
	/// DNS names are available to it at all.
	pub domains: Vec<String>,
	/// The DNS names this application holds for certificates, by name.
	pub names: Vec<CertificateNameView>,
	/// Every certificate Canopy holds or has an order in flight for, newest
	/// first. A DNS name may appear more than once: a key rotation leaves the
	/// previous certificate behind until it expires.
	pub certificates: Vec<CertificateView>,
}

fn name_view(row: ApplicationCertificateName, domains: &[String]) -> CertificateNameView {
	CertificateNameView {
		within_domains: domains.iter().any(|domain| is_within(&row.name, domain)),
		id: row.id,
		name: row.name,
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

/// Everything an application's TLS certificates section needs.
///
/// One call rather than several, because the parts are read together and a
/// half-loaded section would show a certificate without the pause that explains
/// why it is not renewing.
// spec: CRT#presentation
#[utoipa::path(
	post,
	path = "/for_server",
	operation_id = "certificates_for_server",
	tag = "certificates",
	security(("tailscale-user" = [])),
	request_body = ServerIdArgs,
	responses(
		(status = 200, body = CertificatesView),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn for_server(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ServerIdArgs>,
) -> Result<Json<CertificatesView>> {
	let mut conn = state.db_read.get().await?;
	let application = Application::get_by_id(&mut conn, args.server_id).await?;

	let domains = group_domains(&mut conn, application.group_id).await?;
	let names = ApplicationCertificateName::for_application(&mut conn, args.server_id).await?;
	let certificates = ApplicationCertificate::for_server(&mut conn, args.server_id).await?;

	Ok(Json(CertificatesView {
		may_manage_tls: application.may_manage_tls,
		pause: pause_view(&application),
		certificate_profile: application.certificate_profile,
		names: names
			.into_iter()
			.map(|row| name_view(row, &domains))
			.collect(),
		domains,
		certificates: certificates.into_iter().map(certificate_view).collect(),
	}))
}

/// The DNS names declared for certificates under each domain a group controls,
/// and which of them hold a current certificate.
///
/// So that whether a group's certificates are healthy is answerable from the
/// group's page, without visiting each of its applications.
// spec: DNS#on-a-group
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

	let mut conn = state.db_read.get().await?;
	let claims = ServerGroupDomain::list_for_group(&mut conn, args.server_group_id).await?;
	if claims.is_empty() {
		return Ok(Json(Vec::new()));
	}

	let applications = Application::list_live_in_group(&mut conn, args.server_group_id).await?;
	let ids = ids(&applications);
	let by_id = by_id(&applications);
	let certificates = ApplicationCertificate::for_applications(&mut conn, &ids).await?;
	let mut rows = Vec::new();
	for declared in ApplicationCertificateName::for_applications(&mut conn, &ids).await? {
		let Some(application) = by_id.get(&declared.application_id) else {
			continue;
		};
		let mut summary = DomainCertificateView {
			current: false,
			risk: None,
			not_after: None,
		};
		for cert in certificates
			.iter()
			.filter(|c| c.application_id == declared.application_id && c.name == declared.name)
		{
			// The newest usable certificate wins where a name has more than one:
			// a key rotation leaves the old row behind, and the group's view is
			// of whether the name is covered rather than of every attempt.
			if cert.is_collectable() {
				summary.current = true;
			}
			if summary.not_after.is_none() || cert.not_after > summary.not_after {
				summary.not_after = cert.not_after;
				summary.risk = Some(
					match cert.risk() {
						Risk::None => "none",
						Risk::AtRisk => "at_risk",
						Risk::Critical => "critical",
					}
					.to_string(),
				);
			}
		}
		rows.push(DomainNameView {
			name: declared.name,
			server_id: application.id,
			server_name: Some(application.display_name()),
			published: None,
			certificate: Some(summary),
		});
	}

	Ok(Json(by_domain(claims, rows)))
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
// spec: CRT#issuance-authority
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

/// Declare that an application holds a DNS name for certificates.
///
/// A declaration is what a certificate request from the machine is resolved
/// against, so it is how a box running several workloads gets its requests
/// routed to the right one, and it is what Canopy renews and alerts for. It is
/// not an order: the application requests the certificate itself.
///
/// Declaring a DNS name the same application already holds for certificates
/// changes nothing. A DNS name another application holds, for either kind, is
/// refused, and the refusal names the holder so an operator can see what to
/// release first.
// spec: DNS#declared-dns-names
#[utoipa::path(
	post,
	path = "/declare",
	operation_id = "certificates_declare",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = DeclarationArgs,
	responses(
		(status = 200, body = CertificateNameView),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "Another application already declares this DNS name.", body = ProblemDetailsSchema),
	),
)]
pub async fn declare(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeclarationArgs>,
) -> Result<Json<CertificateNameView>> {
	let mut conn = state.db.get().await?;
	let row =
		ApplicationCertificateName::declare(&mut conn, args.application_id, &args.name).await?;
	let application = Application::get_by_id(&mut conn, args.application_id).await?;
	let domains = group_domains(&mut conn, application.group_id).await?;
	Ok(Json(name_view(row, &domains)))
}

/// End an application's hold on a DNS name for certificates.
///
/// What is already in place stands, as revoking a grant leaves it: certificates
/// held stay held until they expire. What ends is Canopy renewing them and
/// raising them as running out, and the DNS name being this application's for
/// certificates, which frees it to be declared elsewhere once it is released for
/// addresses too.
// spec: DNS#declared-dns-names
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
	ApplicationCertificateName::release(&mut conn, args.application_id, &args.name).await?;
	Ok(Json(()))
}

/// What a machine's TLS certificates section shows.
///
/// The DNS names its applications declare for certificates, the certificate
/// requests it made that resolved to none of them, and the DNS names denied to
/// it for certificates.
// spec: DNS#on-a-machine
#[utoipa::path(
	post,
	path = "/for_machine",
	operation_id = "certificates_for_machine",
	tag = "certificates",
	security(("tailscale-user" = [])),
	request_body = crate::fns::machines::MachineIdArgs,
	responses(
		(status = 200, body = MachineNamesView),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn for_machine(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<crate::fns::machines::MachineIdArgs>,
) -> Result<Json<MachineNamesView>> {
	let mut conn = state.db_read.get().await?;
	let machine = database::Machine::get_by_id(&mut conn, args.machine_id).await?;
	let applications = machine.applications(&mut conn).await?;

	let ids = ids(&applications);
	let by_id = by_id(&applications);
	let certificates = ApplicationCertificate::for_applications(&mut conn, &ids).await?;
	let declared = ApplicationCertificateName::for_applications(&mut conn, &ids)
		.await?
		.into_iter()
		.filter_map(|row| {
			let application = by_id.get(&row.application_id)?;
			// Each name's newest first, so the first match is the one in play.
			let certificate = certificates
				.iter()
				.find(|cert| cert.application_id == row.application_id && cert.name == row.name)
				.cloned()
				.map(certificate_view);
			Some(MachineDeclaredView {
				published: None,
				certificate,
				name: row.name,
				application_id: application.id,
				application_name: application.display_name(),
			})
		})
		.collect();

	Ok(Json(
		machine_names(
			&mut conn,
			&machine,
			&applications,
			DnsNameKind::Certificate,
			declared,
		)
		.await?,
	))
}

/// Deny a DNS name to a machine for certificates.
///
/// Every certificate request about it from that machine is then refused as
/// denied, and is not recorded, so it raises no notice. Address requests about
/// it are unaffected. Refused while one of the machine's applications declares
/// the DNS name for certificates.
// spec: DNS#denied-dns-names
#[utoipa::path(
	post,
	path = "/deny",
	operation_id = "certificates_deny",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = DenyArgs,
	responses(
		(status = 200, body = DeniedView),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "One of the machine's applications declares this DNS name for certificates.", body = ProblemDetailsSchema),
	),
)]
pub async fn deny(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<DenyArgs>,
) -> Result<Json<DeniedView>> {
	let mut conn = state.db.get().await?;
	crate::fns::dns_names::deny_kind(&mut conn, DnsNameKind::Certificate, &admin.login, args)
		.await
		.map(Json)
}

/// Lift a denial for certificates.
///
/// The machine's certificate requests about the DNS name then resolve as any
/// other's do. A denial of addresses for it stands.
// spec: DNS#denied-dns-names
#[utoipa::path(
	post,
	path = "/lift_denial",
	operation_id = "certificates_lift_denial",
	tag = "certificates",
	security(("tailscale-admin" = [])),
	request_body = MachineDnsNameArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn lift_denial(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MachineDnsNameArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	database::DeniedDnsName::lift(
		&mut conn,
		args.machine_id,
		&args.name,
		DnsNameKind::Certificate,
	)
	.await?;
	Ok(Json(()))
}
