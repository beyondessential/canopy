use axum::{Json, extract::State};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::ServerDevice;

use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_types::server::{app_type::ApplicationType, rank::ServerRank};
use database::{Db, applications::Application, url_field::UrlField};
use serde::Serialize;
use utoipa::ToSchema;
use uuid::Uuid;

use crate::state::AppState;

/// The application routes as fielded agents call them, under `/servers`.
///
/// Kept because a device API client cannot follow a redirect: a fielded
/// bestool asks for the path it was built against and takes the answer, so the
/// old prefix has to keep answering rather than pointing elsewhere.
// spec: PAC
pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(public: list))
		.routes(routes!(public: self_identity))
}

/// The same two routes under `/applications`, which is what they are about.
///
/// Aliases, not a move. Both prefixes answer identically and are documented
/// separately, because withdrawing either from the schema would break the
/// generated client — a consumer cannot tell a withdrawn path from an
/// unreachable one.
// spec: PAC
pub fn alias_routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(public: list_applications))
		.routes(routes!(public: application_self))
}

/// List publicly-listed central applications.
///
/// The `/applications` name for [`list`], answering identically.
#[utoipa::path(
	get,
	path = "/",
	operation_id = "list_applications",
	tag = "applications",
	responses(
		(status = 200, description = "Publicly-listed central applications, ordered by rank then name.", body = Vec<PublicServer>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn list_applications(state: State<Db>) -> Result<Json<Vec<PublicServer>>> {
	list(state).await
}

/// Get the calling identity and the box it is enrolled as.
///
/// The `/applications` name for [`self_identity`], answering identically.
#[utoipa::path(
	get,
	path = "/self",
	operation_id = "application_self",
	tag = "applications",
	security(("server-device" = [])),
	responses(
		(status = 200, body = SelfResponse),
		(status = 401, body = ProblemDetailsSchema),
		(status = 409, body = ProblemDetailsSchema),
		(status = 412, body = ProblemDetailsSchema),
	),
)]
pub async fn application_self(
	device: ServerDevice,
	state: State<Db>,
) -> Result<Json<SelfResponse>> {
	self_identity(device, state).await
}

/// A publicly-listed central server that a client can connect to.
#[derive(Debug, Serialize, ToSchema)]
pub struct PublicServer {
	/// Public-facing display name of the server.
	pub name: String,
	/// The server's reachable base URL.
	pub host: UrlField,
	/// The server's environment tier (production, clone, demo, test, or
	/// dev), if set. Used to order the listing and to let clients label
	/// non-production entries.
	pub rank: Option<ServerRank>,
}

fn rank_order(rank: &Option<ServerRank>) -> u32 {
	match rank {
		Some(ServerRank::Production) => 0,
		Some(ServerRank::Clone) => 1,
		Some(ServerRank::Demo) => 2,
		Some(ServerRank::Test) => 3,
		Some(ServerRank::Dev) => 4,
		_ => 5,
	}
}

/// List publicly-listed central applications.
///
/// Returns every central server that has both a public display name and a
/// reachable host configured, ordered by environment tier (production
/// first, then clone, demo, test, dev) and then by name. Used by clients
/// to let a user pick which server to connect to.
#[utoipa::path(
	get,
	path = "/",
	operation_id = "list_servers",
	tag = "applications",
	responses(
		(status = 200, description = "Publicly-listed central applications, ordered by rank then name.", body = Vec<PublicServer>),
		(status = 500, body = ProblemDetailsSchema),
	),
)]
pub async fn list(State(db): State<Db>) -> Result<Json<Vec<PublicServer>>> {
	let mut db = db.get().await?;
	let mut applications =
		Application::list_by_type(&mut db, ApplicationType::TamanuCentral, 0, None)
			.await?
			.into_iter()
			.filter_map(|s| {
				// Only list applications that have both a public name and a URL — the
				// mobile app needs a reachable host.
				match (s.public_name, s.host) {
					(Some(name), Some(host)) => Some(PublicServer {
						name,
						host,
						rank: s.rank,
					}),
					_ => None,
				}
			})
			.collect::<Vec<_>>();

	applications.sort_by(|a, b| {
		rank_order(&a.rank)
			.cmp(&rank_order(&b.rank))
			.then_with(|| a.name.cmp(&b.name))
	});

	Ok(Json(applications))
}

/// The calling device's own identity, as assigned at enrollment.
#[derive(Debug, Serialize, ToSchema)]
pub struct SelfResponse {
	/// The box the calling device is enrolled as. This is the id a device
	/// pushes status against, and the one `GET /machines/self` calls
	/// `machine_id`.
	pub server_id: Uuid,
	/// The calling device's own identity.
	pub device_id: Uuid,
}

/// Report the calling device's own identity.
///
/// Deprecated in favour of `GET /machines/self`, which says what runs on the box
/// as well as which box it is.
///
/// Resolves the caller from its device certificate and returns the box it is
/// enrolled as together with its own device ID — the same pair returned when the
/// device completed enrollment. A device authenticates entirely from its
/// certificate, so it never needs these IDs to make calls; this endpoint lets
/// one that has lost track of them recover them.
///
/// The id answered is the box's, not any workload's: an identity belongs to a
/// box, so the answer stays the same however many applications run on it.
///
/// - **401**: the request has no client certificate, or the certificate
///   doesn't match a known device.
/// - **409**: retained for callers that handle it; no longer raised, since an
///   identity is enrolled as at most one box.
/// - **412**: the device is registered but has not yet been attached to a
///   box.
// spec: DID
#[utoipa::path(
	get,
	path = "/self",
	operation_id = "server_self",
	tag = "applications",
	security(("server-device" = [])),
	responses(
		(status = 200, body = SelfResponse),
		(status = 401, body = ProblemDetailsSchema),
		(status = 409, body = ProblemDetailsSchema),
		(status = 412, body = ProblemDetailsSchema),
	),
)]
pub async fn self_identity(
	device: ServerDevice,
	State(db): State<Db>,
) -> Result<Json<SelfResponse>> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;
	let machine = database::machines::Machine::get_by_device_id(&mut conn, device_id)
		.await?
		.ok_or(AppError::DeviceHasNoServer)?;
	Ok(Json(SelfResponse {
		server_id: machine.id,
		device_id,
	}))
}
