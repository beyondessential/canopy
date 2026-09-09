use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::{backup_jobs::BillingLabels, tailscale_auth::TailscaleAdmin};
use commons_types::{
	Uuid,
	server::TagMap,
	status::{HealthState, ShortStatus},
};
use database::server_groups::{NewServerGroup, PartialServerGroup, ServerGroup};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use commons_servers::tailscale_auth::TailscaleUser;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(list))
		.routes(routes!(get))
		.routes(routes!(create))
		.routes(routes!(update))
		.routes(routes!(delete))
		.routes(routes!(restore))
		.routes(routes!(list_archived))
		.routes(routes!(server_counts))
		.routes(routes!(search))
}

/// List all live server groups.
///
/// Returns every non-archived server group, including its name, notes, tags,
/// Slack notification delay, and effective version information. The request
/// body is ignored; send an empty JSON object.
#[utoipa::path(
	post,
	path = "/list",
	operation_id = "server_groups_list",
	tag = "server_groups",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Vec<ServerGroup>),
	),
)]
pub async fn list(
	State(state): State<AppState>,
	_user: TailscaleUser,
	_body: Json<serde_json::Value>,
) -> Result<Json<Vec<ServerGroup>>> {
	let mut conn = state.db.get().await?;
	let groups = ServerGroup::list_all(&mut conn).await?;
	Ok(Json(groups))
}

/// The number of live applications in one server group.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct GroupServerCount {
	/// Identifier of the server group.
	pub server_group_id: Uuid,
	/// Number of live (non-archived) applications currently in the group.
	pub server_count: i64,
}

/// Count live applications per group.
///
/// Returns one entry per server group that has at least one live
/// (non-archived) member server. Groups with no live members are omitted, so
/// treat a missing entry as a count of zero. The request body is ignored;
/// send an empty JSON object.
#[utoipa::path(
	post,
	path = "/server_counts",
	operation_id = "server_groups_server_counts",
	tag = "server_groups",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Vec<GroupServerCount>),
	),
)]
pub async fn server_counts(
	State(state): State<AppState>,
	_user: TailscaleUser,
	_body: Json<serde_json::Value>,
) -> Result<Json<Vec<GroupServerCount>>> {
	let mut conn = state.db.get().await?;
	let counts = ServerGroup::live_server_counts(&mut conn).await?;
	Ok(Json(
		counts
			.into_iter()
			.map(|(server_group_id, server_count)| GroupServerCount {
				server_group_id,
				server_count,
			})
			.collect(),
	))
}

/// Identifies the server group to operate on.
#[derive(Deserialize, ToSchema)]
pub struct GroupIdArgs {
	/// Identifier of the server group.
	pub server_group_id: Uuid,
}

/// One effective billing label attributed to a group's cloud resources.
///
/// Labels are computed from the group's configuration: explicit `billing.*`
/// tags on the group are honoured verbatim; otherwise the product comes from
/// the one its live members agree on, the deployment label from the group name in
/// lower-kebab-case, and the stage from the group's highest-ranked live member
/// (for example `prod`). A label with nothing to attribute to is omitted
/// entirely: the stage when the group has no ranked members, and the product
/// when its members span products.
// spec: APP#billing-attribution
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct BillingTag {
	/// Label key, for example `billing.product`.
	pub key: String,
	/// Label value.
	pub value: String,
}

/// A group's effective billing labels, for display on the group + server views.
pub(crate) async fn group_billing_labels(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group: &ServerGroup,
) -> Result<Vec<BillingTag>> {
	let highest_rank = ServerGroup::highest_member_ranks(conn, &[group.id])
		.await?
		.get(&group.id)
		.copied();
	// A group names a product only when its members agree on one; naming one
	// of several would attribute shared cost to whichever happened to be
	// picked. A central and a facility are both Tamanu, so the pair agrees.
	// spec: APP#billing-attribution
	let software = ServerGroup::sole_member_software(conn, &[group.id])
		.await?
		.remove(&group.id);
	Ok(
		BillingLabels::from_group(&group.tags, &group.name, software, highest_rank)
			.into_tags()
			.into_iter()
			.map(|(key, value)| BillingTag { key, value })
			.collect(),
	)
}

/// A server group together with its member applications and billing labels.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GroupDetail {
	/// The group itself.
	pub group: ServerGroup,
	/// The group's member applications, sorted by name, with current status and
	/// display host included.
	pub applications: Vec<super::applications::ServerInfo>,
	/// The group's member machines, sorted by name. A restore replica and a
	/// maintenance window are both declared over a machine, so an operator
	/// surface offering either needs the boxes rather than the workloads.
	// spec: RST#declared-replicas
	pub machines: Vec<GroupMachine>,
	/// The group's effective `billing.*` labels (product/deployment/stage).
	pub billing_labels: Vec<BillingTag>,
	/// Whether a maintenance window (or its settle period) suspends the group.
	pub maintained: bool,
	/// Whether the suspension is only the settle period: the window has
	/// ended and watching resumes when it elapses.
	pub maintenance_settling: bool,
}

/// One of a group's machines, as an operator picks it out of a list.
///
/// Carries the box's own state as well as its name, because the group tree
/// draws each machine as an enclosure around the applications on it and an
/// enclosure with nothing to say is a decoration. A box whose own checks are
/// failing while its workloads are fine is a state only this can show.
// spec: FLT#navigating-the-two-grains
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct GroupMachine {
	/// Unique identifier of the machine.
	pub id: Uuid,
	/// The operator-assigned name, where it has one.
	pub name: Option<String>,
	/// Whether the box is reachable, judged against its own threshold.
	pub up: ShortStatus,
	/// The box's own health, from the checks filed against it. What the
	/// applications on it make of their own checks is each application's.
	pub health: HealthState,
	/// Whether a maintenance window suspends this box, its own or its group's.
	pub maintained: bool,
	/// Whether every window over the box has ended and it is serving out the
	/// settle period.
	pub maintenance_settling: bool,
	/// Whether the window covering this box was declared over the box itself,
	/// as against one reaching it through its environment or its group.
	// spec: MNT#presentation
	pub own_window: bool,
	/// The platform the box reports, where it reports one. The one machine
	/// figure the tree shows: it is what distinguishes two otherwise
	/// identical rows.
	// spec: FIG#machine-figures
	pub platform: Option<String>,
}

/// Get a server group with its members.
///
/// Returns the group, its member applications (sorted by name, with current status
/// and display host), and the group's effective billing labels. Responds 404
/// if no group exists with the given identifier.
#[utoipa::path(
	post,
	path = "/get",
	operation_id = "server_groups_get",
	tag = "server_groups",
	security(("tailscale-user" = [])),
	request_body = GroupIdArgs,
	responses(
		(status = 200, body = GroupDetail),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<GroupIdArgs>,
) -> Result<Json<GroupDetail>> {
	let mut conn = state.db.get().await?;
	let group = ServerGroup::get_by_id(&mut conn, args.server_group_id).await?;
	let (applications, machines) = tree_members(&mut conn, &group).await?;
	let billing_labels = group_billing_labels(&mut conn, &group).await?;
	let maintained = database::maintenance_windows::MaintenanceWindow::suspends(
		&mut conn,
		None,
		None,
		Some(args.server_group_id),
	)
	.await?;
	let maintenance_settling = maintained
		&& database::maintenance_windows::MaintenanceWindow::open_for(
			&mut conn,
			database::issues::Scope::Group(args.server_group_id),
			None,
		)
		.await?
		.is_none();

	Ok(Json(GroupDetail {
		group,
		applications,
		machines,
		maintained,
		maintenance_settling,
		billing_labels,
	}))
}

/// A group's whole membership, in the shape the group tree renders it: every
/// live application in the group, decorated with its status and display host,
/// and every machine in it.
///
/// Both detail pages end with the same tree the group page shows, so all three
/// read the membership from here rather than each assembling its own.
// spec: FLT
pub async fn tree_members(
	conn: &mut database::diesel_async::AsyncPgConnection,
	group: &ServerGroup,
) -> Result<(Vec<super::applications::ServerInfo>, Vec<GroupMachine>)> {
	let members = group.list_servers(conn).await?;
	let group_name = group.name.clone();
	let mut applications: Vec<super::applications::ServerInfo> = members
		.into_iter()
		.map(|s| {
			let mut info = super::applications::server_to_info(s);
			info.group_name = Some(group_name.clone());
			info
		})
		.collect();
	applications.sort_by(|a, b| {
		a.name
			.as_deref()
			.unwrap_or("")
			.cmp(b.name.as_deref().unwrap_or(""))
	});
	super::applications::decorate_with_status(conn, &mut applications).await?;
	super::applications::fill_display_hosts(conn, &mut applications).await?;

	// The boxes, with the state the tree draws on each enclosure. Read in
	// batch: a group has as many machines as workloads, and asking per box
	// would put four round trips on every one of them.
	// spec: FLT#navigating-the-two-grains
	let boxes = database::machines::Machine::list_for_group(conn, group.id).await?;
	let machine_ids: Vec<Uuid> = boxes.iter().map(|m| m.id).collect();
	let machine_health = database::issues::machine_health_from_check_state(
		conn,
		&boxes.iter().map(|m| (m.id, m.group_id)).collect::<Vec<_>>(),
	)
	.await?;
	let machine_reports =
		database::reported_detail::MachineReportedDetail::latest_for_machines(conn, &machine_ids)
			.await?;
	let machine_detail = database::reported_detail::MachineReportedDetail::merge_by_machine(
		database::reported_detail::MachineReportedDetail::for_machines(conn, &machine_ids).await?,
	);
	// A window is declared over a machine or a group, and a box is suspended
	// by either.
	// spec: MNT#presentation
	let suspended =
		database::maintenance_windows::MaintenanceWindow::suspended_targets(conn).await?;
	let machines: Vec<GroupMachine> = boxes
		.into_iter()
		.map(|m| GroupMachine {
			up: m.reachability(machine_reports.get(&m.id).copied()),
			health: machine_health.get(&m.id).copied().unwrap_or_default(),
			maintained: suspended.suspends(m.id, m.group_id),
			maintenance_settling: suspended.settling(m.id, m.group_id),
			own_window: suspended.machine_window(m.id),
			// A box's platform is its own reports' or nothing: the fallback
			// through an application's Postgres banner belongs to the
			// application grain, not here.
			platform: machine_detail.get(&m.id).and_then(|d| d.os_platform()),
			id: m.id,
			name: m.name,
		})
		.collect();

	Ok((applications, machines))
}

/// Request to create a new server group.
#[derive(Deserialize, ToSchema)]
pub struct ServerGroupsCreateArgs {
	/// Display name for the new group.
	pub name: String,
	/// Free-form notes about the group. Defaults to empty.
	#[serde(default)]
	pub notes: String,
	/// Tags to set on the group, as an object of string keys to string
	/// values. Keys with the reserved `canopy:` prefix cannot be set.
	/// Defaults to empty.
	#[serde(default)]
	pub tags: TagMap,
	/// Optional initial delay, in whole seconds, before an "incident opened"
	/// Slack notification for this group is delivered; an incident that
	/// resolves within the window never notifies. Omit to accept the default.
	#[serde(default)]
	#[schema(value_type = Option<i64>, format = "int64")]
	pub slack_open_delay: Option<database::pg_duration::PgDuration>,
	/// Optional linger window, in whole seconds: how long an incident stays
	/// open after its last failure recovers, so a failure returning within
	/// the window continues the same incident instead of opening (and
	/// notifying about) a new one. Omit to accept the default.
	#[serde(default)]
	#[schema(value_type = Option<i64>, format = "int64")]
	pub slack_close_delay: Option<database::pg_duration::PgDuration>,
}

/// Create a server group.
///
/// Creates a new, empty server group and returns it. Requires the caller to
/// be on the admin allow-list. Responds 400 if the request is invalid.
#[utoipa::path(
	post,
	path = "/create",
	operation_id = "server_groups_create",
	tag = "server_groups",
	security(("tailscale-admin" = [])),
	request_body = ServerGroupsCreateArgs,
	responses(
		(status = 200, body = ServerGroup),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn create(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ServerGroupsCreateArgs>,
) -> Result<Json<ServerGroup>> {
	let mut conn = state.db.get().await?;
	let group = ServerGroup::create(
		&mut conn,
		NewServerGroup {
			name: args.name,
			notes: args.notes,
			tags: args.tags,
			slack_open_delay: args.slack_open_delay,
			slack_close_delay: args.slack_close_delay,
		},
	)
	.await?;
	Ok(Json(group))
}

/// Request to update a server group.
#[derive(Deserialize, ToSchema)]
pub struct ServerGroupsUpdateArgs {
	/// Identifier of the group to update.
	pub server_group_id: Uuid,
	/// The fields to change. Any field omitted is left unchanged.
	pub data: PartialServerGroup,
}

/// Update a server group.
///
/// Applies a partial update: only the fields present in `data` (name, notes,
/// tags, Slack notification delay) are changed. Returns the updated group.
/// Requires the caller to be on the admin allow-list. Responds 404 if the
/// group does not exist and 400 if the request is invalid.
#[utoipa::path(
	post,
	path = "/update",
	operation_id = "server_groups_update",
	tag = "server_groups",
	security(("tailscale-admin" = [])),
	request_body = ServerGroupsUpdateArgs,
	responses(
		(status = 200, body = ServerGroup),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn update(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ServerGroupsUpdateArgs>,
) -> Result<Json<ServerGroup>> {
	let mut conn = state.db.get().await?;
	let group = ServerGroup::update(&mut conn, args.server_group_id, args.data).await?;
	Ok(Json(group))
}

/// Archive a server group.
///
/// Soft-deletes the group: it disappears from live listings but is kept and
/// can be restored later. Requires the caller to be on the admin allow-list.
/// Responds 409 if the group still has live member applications; move or archive
/// those first.
#[utoipa::path(
	post,
	path = "/delete",
	operation_id = "server_groups_delete",
	tag = "server_groups",
	security(("tailscale-admin" = [])),
	request_body = GroupIdArgs,
	responses(
		(status = 200),
		(status = 409, body = ProblemDetailsSchema),
	),
)]
pub async fn delete(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<GroupIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	ServerGroup::soft_delete(&mut conn, args.server_group_id).await?;
	Ok(Json(()))
}

/// Restore an archived server group.
///
/// Un-archives a previously deleted group so it reappears in live listings.
/// Requires the caller to be on the admin allow-list. Responds 404 if the
/// group does not exist.
#[utoipa::path(
	post,
	path = "/restore",
	operation_id = "server_groups_restore",
	tag = "server_groups",
	security(("tailscale-admin" = [])),
	request_body = GroupIdArgs,
	responses(
		(status = 200),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn restore(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<GroupIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	ServerGroup::restore(&mut conn, args.server_group_id).await?;
	Ok(Json(()))
}

/// List archived server groups.
///
/// Returns every group that has been archived (soft-deleted) and can be
/// restored. The request body is ignored; send an empty JSON object.
#[utoipa::path(
	post,
	path = "/list_archived",
	operation_id = "server_groups_list_archived",
	tag = "server_groups",
	security(("tailscale-user" = [])),
	responses(
		(status = 200, body = Vec<ServerGroup>),
	),
)]
pub async fn list_archived(
	State(state): State<AppState>,
	_user: TailscaleUser,
	_body: Json<serde_json::Value>,
) -> Result<Json<Vec<ServerGroup>>> {
	let mut conn = state.db.get().await?;
	let groups = ServerGroup::list_archived(&mut conn).await?;
	Ok(Json(groups))
}

/// Search terms for finding server groups.
#[derive(Deserialize, ToSchema)]
pub struct ServerGroupsSearchArgs {
	/// Free-text search query.
	pub query: String,
}

/// Search live server groups.
///
/// Returns non-archived groups matching the free-text query.
#[utoipa::path(
	post,
	path = "/search",
	operation_id = "server_groups_search",
	tag = "server_groups",
	security(("tailscale-user" = [])),
	request_body = ServerGroupsSearchArgs,
	responses(
		(status = 200, body = Vec<ServerGroup>),
	),
)]
pub async fn search(
	State(state): State<AppState>,
	_user: TailscaleUser,
	Json(args): Json<ServerGroupsSearchArgs>,
) -> Result<Json<Vec<ServerGroup>>> {
	let mut conn = state.db.get().await?;
	let groups = ServerGroup::search(&mut conn, &args.query).await?;
	Ok(Json(groups))
}
