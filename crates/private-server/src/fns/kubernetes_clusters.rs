//! Operator management of the Kubernetes cluster registry (spec `K8S`,
//! "Cluster registry").
//!
//! Registering a cluster is one flow that both names it and enrols its relay:
//! `register` mints the relay's credential and leaves a **draft** accounting for
//! the minted identity, `confirm` turns that draft into a registered cluster
//! once the relay has connected and answered, and `reissue` re-mints the
//! credential for one lost before it reached the cluster. A draft persists until
//! an operator removes it; nothing ages it out.
//!
//! Canopy holds no connection credential for a cluster, so there is nothing here
//! to encrypt, rotate, or persist beyond what identifies the relay: the private
//! key the relay installs is returned once, at minting, and never stored.

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::{Uuid, device::DeviceRole};
use database::KubernetesCluster;
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use super::devices::{ProvisionedCredential, mint_provisioned_credential};
use crate::state::AppState;

/// How recently the relay must have answered for registration to read the
/// cluster as connected. It must exceed the hub's probe cadence
/// (`jobs::relay::PROBE_CADENCE`), or a relay answering normally would read as
/// stale between probes; the `Build` exchange stamps on connect too, so a fresh
/// relay confirms without waiting a full cadence.
const REGISTRATION_FRESHNESS: SignedDuration = SignedDuration::from_secs(90);

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(list))
		.routes(routes!(register))
		.routes(routes!(confirm))
		.routes(routes!(reissue))
		.routes(routes!(remove))
}

/// A cluster in the registry, as an operator sees it.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ClusterView {
	/// Unique identifier for this cluster.
	pub id: Uuid,
	/// What an operator sees in the host picker.
	pub name: String,
	/// The relay's identity. Minting a new credential for it keeps this stable,
	/// so an application's cluster reference never moves.
	pub relay_identity_id: Uuid,
	/// Whether the registration has been confirmed. A draft is `false`.
	pub registered: bool,
	/// When the registration was confirmed, or `null` for a draft.
	pub registered_at: Option<Timestamp>,
	/// When canopy last had this cluster's relay answer, or `null` if it never
	/// has. Never cleared once set, so it reads as "when did we last hear from
	/// this" for a cluster that has gone quiet.
	pub last_answered_at: Option<Timestamp>,
	/// Whether the relay has answered recently enough to read as connected right
	/// now. This is what registration turns on.
	pub answering: bool,
}

impl ClusterView {
	fn of(cluster: KubernetesCluster, now: Timestamp) -> Self {
		let answering = cluster
			.last_answered_at
			.is_some_and(|at| now.duration_since(at).abs() < REGISTRATION_FRESHNESS);
		Self {
			id: cluster.id,
			name: cluster.name,
			relay_identity_id: cluster.relay_identity_id,
			registered: cluster.registered_at.is_some(),
			registered_at: cluster.registered_at,
			last_answered_at: cluster.last_answered_at,
			answering,
		}
	}
}

/// The registry as two lists: the registered clusters, and the drafts an
/// operator has begun but not finished.
#[derive(Debug, Clone, Serialize, ToSchema)]
pub struct ClusterList {
	/// The registered clusters, ordered by name.
	pub registered: Vec<ClusterView>,
	/// The in-progress drafts, newest first.
	pub drafts: Vec<ClusterView>,
}

/// List the registry: registered clusters and in-progress drafts.
///
/// Registered clusters and drafts are returned as separate lists, each carrying
/// whether its relay is answering right now.
#[utoipa::path(
	post,
	path = "/list",
	tag = "kubernetes_clusters",
	security(("tailscale-admin" = [])),
	responses((status = 200, body = ClusterList)),
)]
pub async fn list(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
) -> Result<Json<ClusterList>> {
	let mut conn = state.db.get().await?;
	let now = Timestamp::now();
	let registered = KubernetesCluster::list_registered(&mut conn)
		.await?
		.into_iter()
		.map(|c| ClusterView::of(c, now))
		.collect();
	let drafts = KubernetesCluster::list_drafts(&mut conn)
		.await?
		.into_iter()
		.map(|c| ClusterView::of(c, now))
		.collect();
	Ok(Json(ClusterList { registered, drafts }))
}

/// Begin registering a cluster: name it and mint its relay's credential.
#[derive(Debug, Deserialize, ToSchema)]
pub struct RegisterArgs {
	/// What to call the cluster in the host picker.
	pub name: String,
}

/// The result of beginning a registration: the draft, and the relay credential
/// returned once for the operator to install into the cluster.
#[derive(Serialize, ToSchema)]
pub struct RegistrationStarted {
	/// The draft that was created, accounting for the minted relay identity.
	pub cluster: ClusterView,
	/// The relay's credential, returned once for the operator to install.
	pub credential: ProvisionedCredential,
}

/// Name a cluster and mint its relay's credential, leaving a draft.
///
/// This is what enrols the relay: minting its credential and recording the draft
/// that accounts for the minted identity are one step, so an operator reaches
/// both from the one page. The draft becomes a registered cluster through
/// `confirm`, once the relay has connected and answered.
#[utoipa::path(
	post,
	path = "/register",
	tag = "kubernetes_clusters",
	security(("tailscale-admin" = [])),
	request_body = RegisterArgs,
	responses(
		(status = 200, body = RegistrationStarted),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn register(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<RegisterArgs>,
) -> Result<Json<RegistrationStarted>> {
	let name = args.name.trim();
	if name.is_empty() {
		return Err(commons_errors::AppError::BadRequest(
			"a cluster needs a name".into(),
		));
	}
	let mut conn = state.db.get().await?;

	// Mint the relay's identity first, then record the draft that accounts for
	// it: the draft carries the relay's identity, so the minted relay is never
	// loose even if the operator abandons the wizard here.
	let credential = mint_provisioned_credential(
		&mut conn,
		DeviceRole::Relay,
		None,
		&format!("{name} relay"),
		false,
	)
	.await?;
	let cluster = KubernetesCluster::create_draft(&mut conn, name, credential.device_id).await?;

	Ok(Json(RegistrationStarted {
		cluster: ClusterView::of(cluster, Timestamp::now()),
		credential,
	}))
}

/// Identify one cluster.
#[derive(Debug, Deserialize, ToSchema)]
pub struct ClusterIdArgs {
	/// The cluster to act on.
	pub id: Uuid,
}

/// Confirm a registration: register the cluster if its relay is answering.
///
/// Canopy confirms the relay is connected and answering before the cluster is
/// registered, so a cluster it cannot read is caught as the operator adds it.
/// What that confirms is that canopy can reach the relay: a relay that answers
/// while its access to the cluster is still incomplete registers, and reports
/// what it cannot do as checks. Idempotent — an already-registered cluster is
/// returned unchanged.
#[utoipa::path(
	post,
	path = "/confirm",
	tag = "kubernetes_clusters",
	security(("tailscale-admin" = [])),
	request_body = ClusterIdArgs,
	responses(
		(status = 200, body = ClusterView),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn confirm(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ClusterIdArgs>,
) -> Result<Json<ClusterView>> {
	let mut conn = state.db.get().await?;
	let cluster = KubernetesCluster::get_by_id(&mut conn, args.id).await?;
	let now = Timestamp::now();
	let view = ClusterView::of(cluster, now);

	if view.registered {
		return Ok(Json(view));
	}
	if !view.answering {
		return Ok(Json(view));
	}
	let registered = KubernetesCluster::register(&mut conn, args.id).await?;
	Ok(Json(ClusterView::of(registered, now)))
}

/// Re-issue a cluster's relay credential, retiring the one before it.
///
/// For a credential lost before it reached the cluster. Re-issuing keeps the
/// relay's identity, so an application's cluster reference never moves, and
/// retires the superseded key, which was never deployed anywhere.
#[utoipa::path(
	post,
	path = "/reissue",
	tag = "kubernetes_clusters",
	security(("tailscale-admin" = [])),
	request_body = ClusterIdArgs,
	responses(
		(status = 200, body = ProvisionedCredential),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn reissue(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ClusterIdArgs>,
) -> Result<Json<ProvisionedCredential>> {
	let mut conn = state.db.get().await?;
	let cluster = KubernetesCluster::get_by_id(&mut conn, args.id).await?;
	let credential = mint_provisioned_credential(
		&mut conn,
		DeviceRole::Relay,
		Some(cluster.relay_identity_id),
		&format!("{} relay", cluster.name),
		true,
	)
	.await?;
	Ok(Json(credential))
}

/// Remove a cluster from the registry, draft or registered.
///
/// The relay's identity is removed with the cluster; a draft removed this way is
/// the operator abandoning a registration.
#[utoipa::path(
	post,
	path = "/remove",
	tag = "kubernetes_clusters",
	security(("tailscale-admin" = [])),
	request_body = ClusterIdArgs,
	responses(
		(status = 200, body = ()),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn remove(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ClusterIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	// 404 if it does not exist, rather than a silent success.
	KubernetesCluster::get_by_id(&mut conn, args.id).await?;
	KubernetesCluster::remove(&mut conn, args.id).await?;
	Ok(Json(()))
}
