//! A registered cluster as a host in the fleet (spec `K8S`, "A cluster's
//! page").
//!
//! The registry (`kubernetes_clusters`) is administration: registering a
//! cluster and re-issuing its relay's credential. This is monitoring: the
//! cluster's health, its reachability, the checks its relay files about it,
//! and the applications it hosts, addressed beneath the fleet beside its
//! machines.

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::{
	Uuid,
	server::{app_type::ApplicationType, rank::ServerRank},
	status::{ConsolidatedChecks, HealthState, ShortStatus},
};
use database::{KubernetesCluster, server_groups::ServerGroup};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::state::AppState;

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: get_detail))
		.routes(routes!(write: update))
}

/// Request body identifying a cluster.
#[derive(Deserialize, ToSchema)]
pub struct ClusterIdArgs {
	/// The cluster to read.
	pub cluster_id: Uuid,
}

/// A cluster, as its page presents it.
#[derive(Serialize, ToSchema)]
pub struct Cluster {
	/// Unique identifier for this cluster.
	pub id: Uuid,
	/// What an operator calls the cluster.
	pub name: String,
	/// How long, in seconds, the cluster may go unheard before it reads as
	/// unreachable.
	pub alert_when_down_for: i64,
}

/// An application a cluster hosts, as a row in the cluster's list.
#[derive(Serialize, ToSchema)]
pub struct ClusterApplication {
	/// Unique identifier for this application.
	pub id: Uuid,
	/// What the application is called within its group.
	pub name: Option<String>,
	/// What the application is.
	#[schema(value_type = String)]
	pub r#type: ApplicationType,
	/// The application's environment tier.
	pub rank: Option<ServerRank>,
	/// Where the application answers, or empty when it has no address.
	pub display_host: String,
	/// The group the application belongs to, taken from its namespace.
	pub group_id: Option<Uuid>,
	/// That group's name.
	pub group_name: Option<String>,
	/// Whether the application is currently reporting, on its own threshold.
	pub up: ShortStatus,
	/// The application's own health.
	pub health: HealthState,
}

/// Everything a cluster's page presents.
#[derive(Serialize, ToSchema)]
pub struct ClusterDetail {
	/// The cluster's own record.
	pub cluster: Cluster,
	/// When the cluster was last reported on by its relay.
	pub last_reported_at: Option<Timestamp>,
	/// Whether the cluster is currently reporting, on its own threshold.
	pub up: ShortStatus,
	/// The cluster's own health, from the checks filed against it.
	pub health: HealthState,
	/// The checks filed against the cluster, graded and classified.
	pub checks: ConsolidatedChecks,
	/// The applications the cluster hosts.
	pub applications: Vec<ClusterApplication>,
}

/// Get everything a registered cluster's page presents.
///
/// Returns 404 for a cluster that is still a draft: only a registered cluster
/// hosts applications or carries checks.
// spec: K8S
#[utoipa::path(
	post,
	path = "/get_detail",
	operation_id = "clusters_get_detail",
	tag = "clusters",
	security(("tailscale-user" = [])),
	request_body = ClusterIdArgs,
	responses(
		(status = 200, body = ClusterDetail),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get_detail(
	State(state): State<AppState>,
	Json(args): Json<ClusterIdArgs>,
) -> Result<Json<ClusterDetail>> {
	let mut conn = state.db.get().await?;
	let cluster = registered(&mut conn, args.cluster_id).await?;

	// One consolidated read drives both the headline health and the checks
	// table, so they cannot disagree.
	let checks =
		database::issues::consolidated_checks_latest_for_cluster(&mut conn, cluster.id).await?;
	let health = checks.health_state;
	let last_reported_at = cluster.last_reported_at(&mut conn).await?;
	let up = cluster.reachability(last_reported_at);

	let hosted = cluster.applications(&mut conn).await?;
	let group_ids: Vec<Uuid> = hosted.iter().filter_map(|a| a.group_id).collect();
	let group_names = ServerGroup::names_by_ids(&mut conn, &group_ids).await?;
	let application_ids: Vec<Uuid> = hosted.iter().map(|a| a.id).collect();
	let last_reported =
		database::reported_detail::ReportedDetail::last_reported_ats(&mut conn, &application_ids)
			.await?;
	let pairs: Vec<(Uuid, Option<Uuid>)> = hosted.iter().map(|a| (a.id, a.group_id)).collect();
	let healths = database::issues::health_from_check_state(&mut conn, &pairs).await?;
	let applications = hosted
		.into_iter()
		.map(|a| ClusterApplication {
			up: ShortStatus::grade(last_reported.get(&a.id).copied(), a.alert_when_down_for.0),
			health: healths.get(&a.id).copied().unwrap_or_default(),
			group_name: a.group_id.and_then(|g| group_names.get(&g).cloned()),
			display_host: a.host.as_ref().map(|h| h.0.to_string()).unwrap_or_default(),
			id: a.id,
			name: a.name,
			r#type: a.r#type,
			rank: a.rank,
			group_id: a.group_id,
		})
		.collect();

	Ok(Json(ClusterDetail {
		cluster: Cluster {
			id: cluster.id,
			name: cluster.name,
			alert_when_down_for: cluster.alert_when_down_for.0.as_secs(),
		},
		last_reported_at,
		up,
		health,
		checks,
		applications,
	}))
}

/// What an admin changes about a cluster from its page.
#[derive(Deserialize, ToSchema)]
pub struct ClusterUpdateArgs {
	/// The cluster to change.
	pub cluster_id: Uuid,
	/// A new name. Omit to leave it.
	pub name: Option<String>,
	/// How long, in seconds, the cluster may go unheard before it reads as
	/// unreachable. Must be positive. Omit to leave it.
	pub alert_when_down_for: Option<i64>,
}

/// Change a registered cluster's name or unreachable threshold.
// spec: K8S
#[utoipa::path(
	post,
	path = "/update",
	operation_id = "clusters_update",
	tag = "clusters",
	security(("tailscale-admin" = [])),
	request_body = ClusterUpdateArgs,
	responses(
		(status = 200, body = Cluster),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn update(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ClusterUpdateArgs>,
) -> Result<Json<Cluster>> {
	let mut conn = state.db.get().await?;
	let mut cluster = registered(&mut conn, args.cluster_id).await?;
	if let Some(name) = args.name {
		let name = name.trim();
		if name.is_empty() {
			return Err(AppError::BadRequest("a cluster needs a name".into()));
		}
		cluster = KubernetesCluster::rename(&mut conn, cluster.id, name).await?;
	}
	if let Some(secs) = args.alert_when_down_for {
		cluster = KubernetesCluster::set_alert_when_down_for(
			&mut conn,
			cluster.id,
			SignedDuration::from_secs(secs),
		)
		.await?;
	}
	Ok(Json(Cluster {
		id: cluster.id,
		name: cluster.name,
		alert_when_down_for: cluster.alert_when_down_for.0.as_secs(),
	}))
}

/// The registered cluster by this id, a draft reading as absent.
async fn registered(
	conn: &mut database::diesel_async::AsyncPgConnection,
	id: Uuid,
) -> Result<KubernetesCluster> {
	let cluster = KubernetesCluster::get_by_id(conn, id).await?;
	if !cluster.is_registered() {
		return Err(AppError::NotFound(format!("no registered cluster {id}")));
	}
	Ok(cluster)
}
