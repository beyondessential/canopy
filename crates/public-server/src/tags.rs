use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::{backup_jobs::BillingLabels, device_auth::ServerDevice};
use commons_types::server::{RESERVED_TAG_PREFIX, TagMap};
use database::{
	Db, applications::Application, diesel_async::AsyncPgConnection, server_groups::ServerGroup,
};

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(public: get_self))
}

/// Get the tags for the calling device's own server.
///
/// Returns the effective set of tags for the server the calling device is
/// registered as: any tags set on the server itself, overlaid onto any tags
/// inherited from its server group (a tag set on the server takes precedence
/// over a group tag with the same key). If the server isn't in a group,
/// this returns just its own tags.
///
/// The result also includes a few read-only, synthetic tags describing the
/// server, under the reserved `canopy:` key prefix: `canopy:kind`,
/// `canopy:rank` (if the server has one set), and `canopy:group-id` /
/// `canopy:group-name` (if the server belongs to a group). Operators cannot
/// set tags under that prefix, so these never collide with tags you set
/// yourself.
///
/// When the server belongs to a group, the effective `billing.*` labels are
/// also included, matching the labels canopy attributes to cloud resources:
/// `billing.product`, `billing.deployment`, and `billing.stage` (the last
/// derived from *this* server's own rank, and omitted when the server has no
/// rank). The stage is per-server, not the group's highest rank, so a `clone`
/// server reports `billing.stage=clone` rather than the group's `prod`.
///
/// These are only defaults: a stored `billing.*` tag is honoured over the
/// computed value — the server's own tag first, then the group's. So an
/// operator can pin any billing label on a specific server or the whole group.
///
/// - **401**: the request has no client certificate, or the certificate
///   doesn't match a known device.
/// - **409**: the calling device is attached to more than one server, which
///   should not normally happen; contact support if you see this.
/// - **412**: the device is registered but has not yet been attached to a
///   server.
#[utoipa::path(
	get,
	path = "/",
	tag = "tags",
	security(("server-device" = [])),
	responses(
		(status = 200, body = TagMap),
		(status = 401, body = ProblemDetailsSchema),
		(status = 409, body = ProblemDetailsSchema),
		(status = 412, body = ProblemDetailsSchema),
	),
)]
pub async fn get_self(device: ServerDevice, State(db): State<Db>) -> Result<Json<TagMap>> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;
	let mut applications = Application::get_by_device_id(&mut conn, device_id).await?;
	if applications.len() > 1 {
		return Err(AppError::Conflict(format!(
			"device {device_id} is attached to {} applications; expected at most one",
			applications.len(),
		)));
	}
	let server = applications.pop().ok_or(AppError::DeviceHasNoServer)?;
	Ok(Json(effective_tags_for_server(&mut conn, &server).await?))
}

/// The device-facing effective tag set for a box Canopy holds no application
/// for: its own tags overlaid on its group's, plus the group's synthetic
/// `canopy:` tags.
///
/// The type, product, kind and rank tags are an application's, and so are the
/// billing labels, which attribute a workload to a group. A box with no
/// workload has none of them rather than a made-up default.
// spec: FLT#what-each-carries
pub async fn effective_tags_for_machine(
	conn: &mut AsyncPgConnection,
	machine: &database::machines::Machine,
) -> Result<TagMap> {
	let mut merged = machine.tags_merged_with_group(conn).await?;
	if let Some(group_id) = machine.group_id {
		let group = ServerGroup::get_by_id(conn, group_id).await?;
		merged.0.insert(
			format!("{RESERVED_TAG_PREFIX}group-id"),
			group.id.to_string(),
		);
		merged
			.0
			.insert(format!("{RESERVED_TAG_PREFIX}group-name"), group.name);
	}
	Ok(merged)
}

/// The device-facing effective tag set for a server: its own tags overlaid
/// on its group's, plus the synthetic read-only `canopy:` tags and the
/// effective `billing.*` labels. Shared between the standalone `GET /tags`
/// endpoint and the status-push response, so the two always agree.
pub async fn effective_tags_for_server(
	conn: &mut AsyncPgConnection,
	server: &Application,
) -> Result<TagMap> {
	let mut merged = server.tags_for_device(conn).await?;

	// Fill in the effective billing labels where the server doesn't already
	// carry one, matching what canopy attributes to the group's cloud
	// resources. `merged` already holds server tags overlaid on group tags, so a
	// stored `billing.*` tag (server's own first, then the group's) is honoured
	// and only the missing labels fall back to computed values.
	//
	// Every computed label describes *this* server, not its group: the stage
	// comes from the server's own rank, so a rank=clone server reports
	// `billing.stage=clone` and never the group's `prod`; and the product comes
	// from the server's own product, so a SENAITE server in a Tamanu group
	// reports `billing.product=senaite`. Attribution needs a group to
	// attribute to, so an ungrouped server carries none.
	// spec: APP#billing-attribution
	if let Some(group_id) = server.group_id {
		let group = ServerGroup::get_by_id(conn, group_id).await?;
		for (key, value) in
			BillingLabels::for_server(&group.tags, &group.name, &server.r#type, server.rank)
				.into_tags()
		{
			merged.0.entry(key).or_insert(value);
		}
	}

	Ok(merged)
}
