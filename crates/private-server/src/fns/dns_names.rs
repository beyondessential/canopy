//! Operator-facing DNS name endpoints (private-server, admin SPA): the address
//! side of what an application asks Canopy to do about its own DNS names, and
//! what the two kinds share.
//!
//! Addresses and certificates are separate features that share infrastructure
//! (DNS), so each has its own module: this one serves the DNS names, and
//! `certificates` serves the certificates. What they share lives here and is
//! used by both: the machine's undeclared and denied records, which are held
//! per kind, and the pause, which is of the application rather than of either
//! kind.
//!
//! Reads are open to any tailnet user; anything that changes what Canopy will do
//! on an application's behalf requires admin.
// spec: DNS
// spec: ADR

use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::Uuid;
use commons_types::dns::{ManagedZone, is_within, match_zone};
use database::applications::Application;
use database::{ApplicationName, DnsNameKind, ServerGroupDomain};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::fns::applications::ServerIdArgs;
use crate::fns::certificates::CertificateView;
use crate::fns::server_groups::GroupIdArgs;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: for_server))
		.routes(routes!(read_only: for_group))
		.routes(routes!(read_only: for_machine))
		.routes(routes!(write: declare))
		.routes(routes!(write: release))
		.routes(routes!(write: deny))
		.routes(routes!(write: lift_denial))
		.routes(routes!(read_only: undeclared_notices))
		.routes(routes!(danger(unprotects): pause))
		.routes(routes!(write: resume))
}

/// A DNS name an application holds for addresses, and how far Canopy has got
/// with it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct NameView {
	/// Unique identifier of the registration.
	pub id: Uuid,
	/// The name, normalised.
	pub name: String,
	/// The addresses the application asked to be reachable at.
	pub addresses: Vec<String>,
	/// The addresses Canopy has actually published. Differs from `addresses`
	/// while a change is waiting to be reconciled.
	pub published_addresses: Vec<String>,
	/// Whether the zone has caught up with what the application asked for.
	pub published: bool,
	/// When Canopy last published this name's records.
	#[schema(value_type = Option<String>)]
	pub published_at: Option<Timestamp>,
	/// Why the last publish attempt failed, if it did.
	pub last_error: Option<String>,
	/// Apex of the managed zone covering this name, or null where no configured
	/// zone does, in which case Canopy can publish nothing for it.
	pub zone: Option<String>,
	/// Whether the name lies at or beneath a domain the application's group
	/// controls. An operator may declare one that does not, ahead of the group
	/// claiming its domain; nothing is published for it until then.
	// spec: DNS#on-an-application
	pub within_domains: bool,
}

fn name_view(row: ApplicationName, zones: &[ManagedZone], domains: &[String]) -> NameView {
	NameView {
		within_domains: domains.iter().any(|domain| is_within(&row.name, domain)),
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

/// Whether a DNS name's address records have caught up, or null for one declared
/// with no addresses registered, which has nothing to publish.
fn published_state(row: &ApplicationName) -> Option<bool> {
	let nothing_registered = row.addresses.is_empty() && row.published_addresses.is_empty();
	(!nothing_registered).then(|| row.is_reconciled())
}

/// That an application is paused, and who paused it and why.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct PauseView {
	/// When the pause was set.
	#[schema(value_type = Option<String>)]
	pub paused_at: Option<Timestamp>,
	/// Who set it.
	pub paused_by: Option<String>,
	/// Why it was set.
	pub reason: Option<String>,
}

/// The pause on an application, where it has one.
///
/// Shown in both of the application's sections: it suppresses the alerting that
/// would otherwise chase a certificate running out, so it has to be the thing an
/// operator sees wherever the application's DNS names or certificates are.
// spec: DNS#pausing-an-application
pub(crate) fn pause_view(application: &Application) -> Option<PauseView> {
	application.name_management_paused().then(|| PauseView {
		paused_at: application.name_management_paused_at,
		paused_by: application.name_management_paused_by.clone(),
		reason: application.name_management_pause_reason.clone(),
	})
}

/// The domains a group controls, or none for an application in no group.
pub(crate) async fn group_domains(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group_id: Option<Uuid>,
) -> Result<Vec<String>> {
	Ok(match group_id {
		Some(group) => ServerGroupDomain::list_for_group(conn, group)
			.await?
			.into_iter()
			.map(|claim| claim.domain)
			.collect(),
		None => Vec::new(),
	})
}

/// What an application's DNS names section shows.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DnsNamesView {
	/// Whether an operator has allowed this application to manage its own DNS.
	pub may_manage_dns: bool,
	/// The pause on this application, if it has one.
	pub pause: Option<PauseView>,
	/// The domains this application's group controls, so the UI can say which
	/// names are available to it at all.
	pub domains: Vec<String>,
	/// The DNS names this application holds for addresses, by name.
	pub names: Vec<NameView>,
}

/// Everything an application's DNS names section needs.
///
/// One call rather than several, because the parts are read together and a
/// half-loaded section would show a name without the pause that explains why it
/// is not being published.
// spec: ADR#presentation
#[utoipa::path(
	post,
	path = "/for_server",
	operation_id = "dns_names_for_server",
	tag = "dns_names",
	security(("tailscale-user" = [])),
	request_body = ServerIdArgs,
	responses(
		(status = 200, body = DnsNamesView),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn for_server(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ServerIdArgs>,
) -> Result<Json<DnsNamesView>> {
	let mut conn = state.db_read.get().await?;
	let application = Application::get_by_id(&mut conn, args.server_id).await?;

	let domains = group_domains(&mut conn, application.group_id).await?;
	let names = ApplicationName::for_server(&mut conn, args.server_id).await?;

	Ok(Json(DnsNamesView {
		may_manage_dns: application.may_manage_dns,
		pause: pause_view(&application),
		names: names
			.into_iter()
			.map(|row| name_view(row, &state.dns_zones, &domains))
			.collect(),
		domains,
	}))
}

/// One DNS name in use beneath a group's domain, for either kind.
///
/// Exactly one of `published` and `certificate` is present: the DNS names
/// section carries the first and the TLS certificates section the second.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DomainNameView {
	/// The DNS name.
	pub name: String,
	/// The application that declares it.
	pub server_id: Uuid,
	/// That application's name, for display.
	pub server_name: Option<String>,
	/// For addresses: whether the address records Canopy publishes for it are up
	/// to date, or null where it is declared with no addresses registered. Null
	/// as well in the certificates section, which carries no addresses.
	pub published: Option<bool>,
	/// For certificates: what Canopy holds for it.
	pub certificate: Option<DomainCertificateView>,
}

/// Whether a DNS name declared for certificates holds a current certificate.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DomainCertificateView {
	/// Whether a certificate Canopy holds for it is current and collectable.
	pub current: bool,
	/// How urgently the certificate expiring last needs attention: `none`,
	/// `at_risk`, or `critical`, judged against its own lifetime. Null where
	/// there is no certificate.
	pub risk: Option<String>,
	/// When that certificate expires.
	#[schema(value_type = Option<String>)]
	pub not_after: Option<Timestamp>,
}

/// The DNS names in use under one of a group's domains.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DomainHealthView {
	/// The claimed domain these DNS names sit beneath.
	pub domain: String,
	/// The DNS names in use beneath it, by name.
	pub names: Vec<DomainNameView>,
}

/// The DNS names declared for addresses under each domain a group controls, and
/// whether their records are published.
///
/// So that whether a group's DNS names are healthy is answerable from the
/// group's page, without visiting each of its applications.
// spec: DNS#on-a-group
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "dns_names_for_group",
	tag = "dns_names",
	security(("tailscale-user" = [])),
	request_body = GroupIdArgs,
	responses((status = 200, body = Vec<DomainHealthView>)),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<GroupIdArgs>,
) -> Result<Json<Vec<DomainHealthView>>> {
	let mut conn = state.db_read.get().await?;
	let claims = ServerGroupDomain::list_for_group(&mut conn, args.server_group_id).await?;
	if claims.is_empty() {
		return Ok(Json(Vec::new()));
	}

	let applications = Application::list_live_in_group(&mut conn, args.server_group_id).await?;
	let by_id = by_id(&applications);
	let rows = ApplicationName::for_applications(&mut conn, &ids(&applications))
		.await?
		.into_iter()
		.filter_map(|row| {
			let application = by_id.get(&row.application_id)?;
			Some(DomainNameView {
				published: published_state(&row),
				name: row.name,
				server_id: application.id,
				server_name: Some(application.display_name()),
				certificate: None,
			})
		})
		.collect();

	Ok(Json(by_domain(claims, rows)))
}

/// The identifiers of `applications`, to read all of theirs in one query.
pub(crate) fn ids(applications: &[Application]) -> Vec<Uuid> {
	applications.iter().map(|a| a.id).collect()
}

/// `applications` by identifier, to attach a batch read's rows to their owner.
pub(crate) fn by_id(applications: &[Application]) -> HashMap<Uuid, &Application> {
	applications.iter().map(|a| (a.id, a)).collect()
}

/// Arrange a group's DNS names under the domains they sit beneath.
pub(crate) fn by_domain(
	claims: Vec<ServerGroupDomain>,
	mut rows: Vec<DomainNameView>,
) -> Vec<DomainHealthView> {
	rows.sort_by(|a, b| a.name.cmp(&b.name));
	claims
		.into_iter()
		.map(|claim| DomainHealthView {
			names: rows
				.iter()
				.filter(|row| is_within(&row.name, &claim.domain))
				.cloned()
				.collect(),
			domain: claim.domain,
		})
		.collect()
}

/// An application and the DNS name being declared for it, or released from it.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DeclarationArgs {
	/// The application that serves the DNS name.
	pub application_id: Uuid,
	/// The DNS name, in any case and with or without a trailing dot.
	pub name: String,
}

/// Declare that an application holds a DNS name for addresses.
///
/// A declaration is what an address registration from the machine is resolved
/// against, so it is how a box running several workloads gets its requests
/// routed to the right one. It carries no addresses; the application registers
/// those itself.
///
/// Declaring a DNS name the same application already holds for addresses
/// changes nothing. A DNS name another application holds, for either kind, is
/// refused, and the refusal names the holder so an operator can see what to
/// release first.
// spec: DNS#declared-dns-names
#[utoipa::path(
	post,
	path = "/declare",
	operation_id = "dns_names_declare",
	tag = "dns_names",
	security(("tailscale-admin" = [])),
	request_body = DeclarationArgs,
	responses(
		(status = 200, body = NameView),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "Another application already declares this DNS name.", body = ProblemDetailsSchema),
	),
)]
pub async fn declare(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeclarationArgs>,
) -> Result<Json<NameView>> {
	let mut conn = state.db.get().await?;
	let row = ApplicationName::declare(&mut conn, args.application_id, &args.name).await?;
	let application = Application::get_by_id(&mut conn, args.application_id).await?;
	let domains = group_domains(&mut conn, application.group_id).await?;
	Ok(Json(name_view(row, &state.dns_zones, &domains)))
}

/// End an application's hold on a DNS name for addresses.
///
/// What is already in place stands, as revoking a grant leaves it: the records
/// published stay published. What ends is Canopy treating the DNS name as this
/// application's for addresses, which frees it to be declared elsewhere once it
/// is released for certificates too.
// spec: DNS#declared-dns-names
#[utoipa::path(
	post,
	path = "/release",
	operation_id = "dns_names_release",
	tag = "dns_names",
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

// ── A machine's DNS names ───────────────────────────────────────────────────

/// One of a machine's applications, as a choice to declare a DNS name on.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MachineApplicationView {
	/// The application's identifier.
	pub id: Uuid,
	/// What to call it.
	pub name: String,
	/// Its type's slug.
	pub r#type: String,
}

/// A DNS name declared by one of a machine's applications, for the kind of the
/// section it is in.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MachineDeclaredView {
	/// The DNS name, normalised.
	pub name: String,
	/// The application declaring it.
	pub application_id: Uuid,
	/// The declaring application, for display.
	pub application_name: String,
	/// In the DNS names section: whether its address records are published, or
	/// null where it is declared with no addresses registered. Null as well in
	/// the TLS certificates section, which carries no addresses.
	pub published: Option<bool>,
	/// In the TLS certificates section: the newest certificate Canopy holds or
	/// is ordering for it, if any.
	pub certificate: Option<CertificateView>,
}

/// A request of the section's kind that the machine made and that resolved to no
/// single application.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UndeclaredView {
	/// The DNS name asked about, normalised.
	pub name: String,
	/// When the machine first asked.
	#[schema(value_type = String)]
	pub first_asked_at: Timestamp,
	/// When the machine last asked. A request not repeated for a day no longer
	/// counts.
	#[schema(value_type = String)]
	pub last_asked_at: Timestamp,
}

/// A DNS name an operator has denied to the machine for the section's kind.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct DeniedView {
	/// The DNS name, normalised.
	pub name: String,
	/// The operator who denied it.
	pub denied_by: String,
	/// Why, if they said.
	pub note: Option<String>,
	/// When it was denied.
	#[schema(value_type = String)]
	pub denied_at: Timestamp,
}

pub(crate) fn denied_view(row: database::DeniedDnsName) -> DeniedView {
	DeniedView {
		name: row.dns_name,
		denied_by: row.denied_by,
		note: row.note,
		denied_at: row.created_at,
	}
}

/// Everything a machine's section of one kind needs: the same shape for both
/// kinds, so one component serves them.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct MachineNamesView {
	/// The machine's applications, to declare a DNS name on.
	pub applications: Vec<MachineApplicationView>,
	/// The DNS names its applications declare for the kind, by name.
	pub declared: Vec<MachineDeclaredView>,
	/// Its requests of the kind that resolved to no single application and still
	/// count.
	pub undeclared: Vec<UndeclaredView>,
	/// The DNS names denied to it for the kind.
	pub denied: Vec<DeniedView>,
}

/// The parts of a machine's section that do not depend on how a declared DNS
/// name is shown.
pub(crate) async fn machine_names(
	conn: &mut database::diesel_async::AsyncPgConnection,
	machine: &database::Machine,
	applications: &[Application],
	kind: DnsNameKind,
	declared: Vec<MachineDeclaredView>,
) -> Result<MachineNamesView> {
	let undeclared = database::UndeclaredDnsName::for_machine(conn, machine.id, kind)
		.await?
		.into_iter()
		.map(|row| UndeclaredView {
			name: row.dns_name,
			first_asked_at: row.first_asked_at,
			last_asked_at: row.last_asked_at,
		})
		.collect();

	let denied = database::DeniedDnsName::for_machine(conn, machine.id, kind)
		.await?
		.into_iter()
		.map(denied_view)
		.collect();

	Ok(MachineNamesView {
		applications: applications
			.iter()
			.map(|a| MachineApplicationView {
				id: a.id,
				name: a.display_name(),
				r#type: a.r#type.to_string(),
			})
			.collect(),
		declared,
		undeclared,
		denied,
	})
}

/// What a machine's DNS names section shows.
///
/// The DNS names its applications declare for addresses, the address requests it
/// made that resolved to none of them, and the DNS names denied to it for
/// addresses.
// spec: DNS#on-a-machine
#[utoipa::path(
	post,
	path = "/for_machine",
	operation_id = "dns_names_for_machine",
	tag = "dns_names",
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

	let by_id = by_id(&applications);
	let declared = ApplicationName::for_applications(&mut conn, &ids(&applications))
		.await?
		.into_iter()
		.filter_map(|row| {
			let application = by_id.get(&row.application_id)?;
			Some(MachineDeclaredView {
				published: published_state(&row),
				certificate: None,
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
			DnsNameKind::Addresses,
			declared,
		)
		.await?,
	))
}

/// Which machines have undeclared requests, optionally within one group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct UndeclaredNoticesArgs {
	/// Narrow to one group's machines. Omitted for the whole fleet.
	#[serde(default)]
	pub server_group_id: Option<Uuid>,
}

/// How many undeclared requests one machine has, of each kind.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct UndeclaredNoticeView {
	/// The machine with requests waiting on a declaration.
	pub machine_id: Uuid,
	/// The machine's name.
	pub machine_name: String,
	/// The machine's group. Null for a machine in none.
	pub group_id: Option<Uuid>,
	/// That group's name.
	pub group_name: Option<String>,
	/// How many of its address requests are waiting.
	pub addresses: i64,
	/// How many of its certificate requests are waiting.
	pub certificates: i64,
}

/// The machines with requests waiting on a declaration, of either kind.
///
/// For the notices on the group page and the Status page. Empty when there
/// are none.
// spec: DNS#notices
#[utoipa::path(
	post,
	path = "/undeclared_notices",
	operation_id = "dns_names_undeclared_notices",
	tag = "dns_names",
	security(("tailscale-user" = [])),
	request_body = UndeclaredNoticesArgs,
	responses((status = 200, body = Vec<UndeclaredNoticeView>)),
)]
pub async fn undeclared_notices(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<UndeclaredNoticesArgs>,
) -> Result<Json<Vec<UndeclaredNoticeView>>> {
	let mut conn = state.db_read.get().await?;
	let rows =
		database::UndeclaredDnsName::counts_by_machine(&mut conn, args.server_group_id).await?;
	Ok(Json(
		rows.into_iter()
			.map(|row| UndeclaredNoticeView {
				machine_id: row.machine_id,
				machine_name: row.machine_name,
				group_id: row.group_id,
				group_name: row.group_name,
				addresses: row.addresses,
				certificates: row.certificates,
			})
			.collect(),
	))
}

/// A DNS name to deny to a machine.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DenyArgs {
	/// The machine to deny it to.
	pub machine_id: Uuid,
	/// The DNS name, in any case and with or without a trailing dot.
	pub name: String,
	/// Why, optionally.
	#[serde(default)]
	pub note: Option<String>,
}

/// Deny a DNS name to a machine for addresses.
///
/// Every address request about it from that machine is then refused as denied,
/// and is not recorded, so it raises no notice. Certificate requests about it
/// are unaffected. Refused while one of the machine's applications declares the
/// DNS name for addresses.
// spec: DNS#denied-dns-names
#[utoipa::path(
	post,
	path = "/deny",
	operation_id = "dns_names_deny",
	tag = "dns_names",
	security(("tailscale-admin" = [])),
	request_body = DenyArgs,
	responses(
		(status = 200, body = DeniedView),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "One of the machine's applications declares this DNS name for addresses.", body = ProblemDetailsSchema),
	),
)]
pub async fn deny(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<DenyArgs>,
) -> Result<Json<DeniedView>> {
	let mut conn = state.db.get().await?;
	deny_kind(&mut conn, DnsNameKind::Addresses, &admin.login, args)
		.await
		.map(Json)
}

/// Deny for `kind`, shared by both kinds' modules.
pub(crate) async fn deny_kind(
	conn: &mut database::diesel_async::AsyncPgConnection,
	kind: DnsNameKind,
	denied_by: &str,
	args: DenyArgs,
) -> Result<DeniedView> {
	// A machine that does not exist is a 404 rather than a foreign-key failure.
	database::Machine::get_by_id(conn, args.machine_id).await?;
	let row = database::DeniedDnsName::deny(
		conn,
		args.machine_id,
		&args.name,
		kind,
		denied_by,
		args.note.as_deref(),
	)
	.await?;
	Ok(denied_view(row))
}

/// A machine and one DNS name.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MachineDnsNameArgs {
	/// The machine.
	pub machine_id: Uuid,
	/// The DNS name, in any case and with or without a trailing dot.
	pub name: String,
}

/// Lift a denial for addresses.
///
/// The machine's address requests about the DNS name then resolve as any
/// other's do. A denial of certificates for it stands.
// spec: DNS#denied-dns-names
#[utoipa::path(
	post,
	path = "/lift_denial",
	operation_id = "dns_names_lift_denial",
	tag = "dns_names",
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
		DnsNameKind::Addresses,
	)
	.await?;
	Ok(Json(()))
}

// ── Pausing an application ──────────────────────────────────────────────────

/// Why an application is being paused.
#[derive(Debug, Deserialize, ToSchema)]
pub struct PauseArgs {
	/// The application to pause.
	pub server_id: Uuid,
	/// Why, recorded so whoever finds the pause later knows what it was for.
	pub reason: String,
}

/// Pause an application: Canopy makes no new changes on its behalf.
///
/// Nothing already in place is withdrawn: records published stand, certificates
/// held stay held and collectable until they expire, and the group keeps
/// working exactly as it did. What stops is Canopy doing anything *new*, for
/// either kind.
///
/// A second pause leaves the first in place, so the original reason and time are
/// not overwritten by a later one.
// spec: DNS#pausing-an-application
#[utoipa::path(
	post,
	path = "/pause",
	operation_id = "dns_names_pause",
	tag = "dns_names",
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

/// Lift an application's pause. Work resumes where it left off.
///
/// Only an operator can do this: Canopy never lifts a pause itself, however long
/// it has been in place and however much is expiring under it.
// spec: DNS#pausing-an-application
#[utoipa::path(
	post,
	path = "/resume",
	operation_id = "dns_names_resume",
	tag = "dns_names",
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
