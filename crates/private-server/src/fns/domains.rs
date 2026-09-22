//! Operator-facing group-domain endpoints (private-server, admin SPA).
//!
//! Thin wrappers over `database::server_domains`, plus a read of the managed
//! zones Canopy is configured with. Reads are open to any tailnet user;
//! claiming and releasing require admin.
// spec: DOM

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::{TailscaleAdmin, TailscaleUser};
use commons_types::Uuid;
use commons_types::dns::{ManagedZone, match_zone};
use database::ServerGroupDomain;
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::fns::server_groups::GroupIdArgs;
use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: zones))
		.routes(routes!(read_only: for_group))
		.routes(routes!(read_only: grant_availability))
		.routes(routes!(write: claim))
		.routes(routes!(danger: release))
}

/// A DNS zone Canopy can write records in.
///
/// Zones come from the Canopy instance's own configuration rather than from operator
/// state: they are what the infrastructure has granted Canopy write access to,
/// and they bound which domains a group can be given.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ManagedZoneView {
	/// The zone's apex domain, for example `tamanu.app`.
	pub apex: String,
	/// The identifier the DNS provider knows this zone by.
	pub provider_zone_id: String,
}

/// A domain a group controls, with the managed zone it resolves to.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GroupDomainView {
	/// Unique identifier of the claim.
	pub id: Uuid,
	/// The group that controls the domain.
	pub group_id: Uuid,
	/// The domain, normalised to lower case without a trailing dot.
	pub domain: String,
	/// Apex of the managed zone this domain resolves to — the longest
	/// configured apex it sits within. Null when no configured zone covers it,
	/// which means the zone has left Canopy's configuration since the claim was
	/// made: the claim stands and still excludes others, but Canopy will act on
	/// no name beneath it until the zone is restored or the claim released.
	pub zone: Option<String>,
	/// Login of the operator who claimed it, if recorded.
	pub created_by: Option<String>,
	/// When it was claimed.
	#[schema(value_type = String)]
	pub created_at: Timestamp,
}

fn to_view(row: ServerGroupDomain, zones: &[ManagedZone]) -> GroupDomainView {
	GroupDomainView {
		zone: match_zone(&row.domain, zones).map(|z| z.apex.clone()),
		id: row.id,
		group_id: row.group_id,
		domain: row.domain,
		created_by: row.created_by,
		created_at: row.created_at,
	}
}

/// List the managed DNS zones Canopy is configured with.
///
/// An operator claiming a domain for a group needs these to know which names
/// are claimable at all: a claim has to sit at or under one of these apexes.
/// An empty list means Canopy has been given no zones, so no domain can be
/// claimed until the Canopy instance's configuration provides one.
#[utoipa::path(
	post,
	path = "/zones",
	operation_id = "domains_zones",
	tag = "domains",
	security(("tailscale-user" = [])),
	responses((status = 200, body = Vec<ManagedZoneView>)),
)]
pub async fn zones(
	State(state): State<AppState>,
	_user: TailscaleUser,
) -> Result<Json<Vec<ManagedZoneView>>> {
	Ok(Json(
		state
			.dns_zones
			.iter()
			.map(|zone| ManagedZoneView {
				apex: zone.apex.clone(),
				provider_zone_id: zone.provider_zone_id.clone(),
			})
			.collect(),
	))
}

/// List the domains a group controls.
///
/// Each carries the managed zone it resolves to, or null for a claim no
/// configured zone matches.
#[utoipa::path(
	post,
	path = "/for_group",
	operation_id = "domains_for_group",
	tag = "domains",
	security(("tailscale-user" = [])),
	request_body = GroupIdArgs,
	responses((status = 200, body = Vec<GroupDomainView>)),
)]
pub async fn for_group(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<GroupIdArgs>,
) -> Result<Json<Vec<GroupDomainView>>> {
	let mut conn = state.db_read.get().await?;
	let rows = ServerGroupDomain::list_for_group(&mut conn, args.server_group_id).await?;
	Ok(Json(
		rows.into_iter()
			.map(|row| to_view(row, &state.dns_zones))
			.collect(),
	))
}

/// Where a group stands with respect to granting its applications name management.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GrantAvailabilityView {
	/// What an operator can do with the two grants right now:
	///
	/// - `unconfigured` — Canopy has no managed zones and no group anywhere
	///   controls a domain, so name management is not in use in this Canopy instance.
	///   Granting it would do nothing and there is nothing an operator can do
	///   about that from here; it becomes available once the infrastructure
	///   provides a zone.
	/// - `no_group_domain` — name management is in use, but this group controls no
	///   domain, so a grant would authorise the server over no name. Claim a
	///   domain for the group first.
	/// - `available` — the group controls at least one domain, and a grant takes
	///   effect over the names beneath it.
	pub state: String,
	/// The domains this group controls, so the UI can name what a grant would
	/// cover. Empty unless `state` is `available`.
	pub group_domains: Vec<String>,
}

/// Identifies a group, or none.
#[derive(Debug, Deserialize, ToSchema)]
pub struct MaybeGroupIdArgs {
	/// The group to ask about. Null for a server with no group, which can hold
	/// no domain and so no useful grant.
	pub server_group_id: Option<Uuid>,
}

/// Whether granting a server name management would mean anything yet.
///
/// The two grants are only ever exercised over names beneath a domain the
/// server's group controls, so offering them where no domain is controlled — or
/// where the Canopy instance has no zones at all — presents a control that cannot do
/// anything. The rule lives here rather than in the UI so there is one answer
/// to it.
// spec: DOM#permission-for-a-server-to-manage-its-own-names
#[utoipa::path(
	post,
	path = "/grant_availability",
	operation_id = "domains_grant_availability",
	tag = "domains",
	security(("tailscale-user" = [])),
	request_body = MaybeGroupIdArgs,
	responses((status = 200, body = GrantAvailabilityView)),
)]
pub async fn grant_availability(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<MaybeGroupIdArgs>,
) -> Result<Json<GrantAvailabilityView>> {
	let mut conn = state.db_read.get().await?;

	let group_domains: Vec<String> = match args.server_group_id {
		Some(group) => ServerGroupDomain::list_for_group(&mut conn, group)
			.await?
			.into_iter()
			.map(|claim| claim.domain)
			.collect(),
		None => Vec::new(),
	};

	if !group_domains.is_empty() {
		return Ok(Json(GrantAvailabilityView {
			state: "available".into(),
			group_domains,
		}));
	}

	// Nothing configured and nothing claimed anywhere: the feature is not in use
	// here. Asked in that order because the zone list is already in memory.
	let in_use = !state.dns_zones.is_empty() || ServerGroupDomain::any_claimed(&mut conn).await?;

	Ok(Json(GrantAvailabilityView {
		state: if in_use {
			"no_group_domain"
		} else {
			"unconfigured"
		}
		.into(),
		group_domains: Vec::new(),
	}))
}

/// Fields needed to claim a domain for a group.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DomainClaimArgs {
	/// The group to give control of the domain.
	pub server_group_id: Uuid,
	/// The domain to claim. Case and a trailing dot are not significant. An
	/// internationalised domain is claimed in its ASCII-compatible (punycode)
	/// spelling.
	pub domain: String,
}

/// Claim a domain for a group.
///
/// The group then controls the domain and every name beneath it, and no other
/// group can claim a name overlapping it. Requires the caller to be on the
/// admin allow-list. Responds 400 if the name is not a valid domain of at least
/// two labels or does not sit within one of Canopy's managed zones, and 409 if
/// it overlaps a domain already claimed — by this group or another.
#[utoipa::path(
	post,
	path = "/claim",
	operation_id = "domains_claim",
	tag = "domains",
	security(("tailscale-admin" = [])),
	request_body = DomainClaimArgs,
	responses(
		(status = 200, body = GroupDomainView),
		(status = 400, body = ProblemDetailsSchema),
		(status = 409, description = "The domain overlaps one already claimed.", body = ProblemDetailsSchema),
	),
)]
pub async fn claim(
	State(state): State<AppState>,
	TailscaleAdmin(admin): TailscaleAdmin,
	Json(args): Json<DomainClaimArgs>,
) -> Result<Json<GroupDomainView>> {
	let mut conn = state.db.get().await?;
	let row = ServerGroupDomain::claim(
		&mut conn,
		args.server_group_id,
		&args.domain,
		Some(admin.login),
		&state.dns_zones,
	)
	.await?;
	Ok(Json(to_view(row, &state.dns_zones)))
}

/// Identifies a claim.
#[derive(Debug, Deserialize, ToSchema)]
pub struct DomainIdArgs {
	/// The claim to release.
	pub id: Uuid,
}

/// Release a claimed domain.
///
/// The group loses control of the domain and every name beneath it, and the
/// name becomes claimable again. Requires the caller to be on the admin
/// allow-list. Responds 404 if there is no such claim.
#[utoipa::path(
	post,
	path = "/release",
	operation_id = "domains_release",
	tag = "domains",
	security(("tailscale-admin" = [])),
	request_body = DomainIdArgs,
	responses((status = 200), (status = 404, body = ProblemDetailsSchema)),
)]
pub async fn release(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DomainIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	ServerGroupDomain::release(&mut conn, args.id).await?;
	Ok(Json(()))
}
