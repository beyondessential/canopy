//! The machine-facing identity endpoint, mounted at `/machines`.
//!
//! An identity belongs to a box rather than to the software on it, so a box
//! asking what it is gets its machine and the applications Canopy holds for it.
//! `GET /servers/self` answers the older, application-shaped question and is
//! kept for callers that predate the split.
// spec: DID

use axum::{Json, extract::State};
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::ServerDevice;
use commons_types::Uuid;
use commons_types::server::app_type::ApplicationType;
use database::{Db, machines::Machine};
use serde::Serialize;
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new().routes(routes!(public: self_identity))
}

/// The calling identity, the box it is enrolled as, and what runs on that box.
#[derive(Debug, Serialize, ToSchema)]
pub struct MachineSelfResponse {
	/// The calling identity's own identifier.
	pub device_id: Uuid,
	/// The machine this identity is enrolled as.
	pub machine_id: Uuid,
	/// The types of application Canopy currently holds for that machine. Empty
	/// for a box that has enrolled but not yet reported what runs on it, which
	/// is awaiting a report rather than an error.
	///
	/// A workload is named by its type, which is what the reporter itself said
	/// it was. Canopy's own identifier for an application is internal and never
	/// on the wire.
	// spec: DID#query
	pub applications: Vec<ApplicationType>,
}

/// Report the calling machine's own identity.
///
/// Resolves the caller from its certificate and returns the box it is enrolled
/// as, together with the applications Canopy holds for that box. A machine
/// authenticates entirely from its certificate, so it never needs these ids to
/// make calls; this endpoint lets one that has lost track of them recover them.
///
/// An identity belongs to at most one machine, so the answer is never
/// ambiguous — unlike `GET /servers/self`, which asks which *application* the
/// caller is and cannot answer for a box running more than one.
///
/// - **401**: no client certificate, or one that matches no known identity.
/// - **412**: the identity is registered but is not enrolled as a machine.
// spec: DID#query
#[utoipa::path(
	get,
	path = "/self",
	operation_id = "machine_self",
	tag = "machines",
	security(("server-device" = [])),
	responses(
		(status = 200, body = MachineSelfResponse),
		(status = 401, body = ProblemDetailsSchema),
		(status = 412, body = ProblemDetailsSchema),
	),
)]
pub async fn self_identity(
	device: ServerDevice,
	State(db): State<Db>,
) -> Result<Json<MachineSelfResponse>> {
	let mut conn = db.get().await?;
	let device_id = device.0.0.id;
	let machine = Machine::get_by_device_id(&mut conn, device_id)
		.await?
		.ok_or(AppError::DeviceHasNoServer)?;
	let applications = machine
		.applications(&mut conn)
		.await?
		.into_iter()
		.map(|a| a.r#type)
		.collect();
	Ok(Json(MachineSelfResponse {
		device_id,
		machine_id: machine.id,
		applications,
	}))
}
