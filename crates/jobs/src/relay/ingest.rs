//! Taking a relay's filing into canopy.
//!
//! The two families take different paths, because they are different things:
//!
//! - A **harvest** filing is a status-push body, so it is destined for the very
//!   ingestion an HTTP push goes through. A Kubernetes application and an
//!   application that pushes its own reports therefore share one catalog entry
//!   and one policy per check, and cannot drift into subtly different checks —
//!   because there is one implementation, not two kept in agreement.
//! - A **substrate** filing has no push analogue (the `kubernetes` source is
//!   reserved from the device API), so it goes through `file_check_instances`,
//!   the same path canopy's own determinations take.
//!
//! Both carry the relay's identity as provenance, and both land at a scope
//! expressed in the single [`database::issues::Scope`] vocabulary.

use commons_errors::{AppError, Result};
use commons_types::{Uuid, source::SUBSTRATE_SOURCE};
use database::{
	KubernetesCluster,
	diesel_async::AsyncPgConnection,
	issues::{CheckInstance, InstancedCheckFiling, Scope, file_check_instances},
};
use relay_protocol::{Filing, FilingTarget, HarvestFiling, SubstrateFiling};
use tracing::warn;

/// Where a filing lands, once the coordinates the relay named have been
/// resolved against what canopy knows.
///
/// This is the bridge from the relay's vocabulary (namespaces and instances)
/// to canopy's (applications, groups, canopy-wide). Resolution happens here
/// and nowhere else, so a relay never holds a canopy identifier.
#[derive(Debug, Clone)]
pub enum Placement {
	/// The application an instance coordinate names.
	Application(Uuid),
	/// The group a namespace names — its applications at one rank.
	Group(Uuid),
	/// The cluster the relay serves. A cluster is its own check target: its
	/// substrate checks are read on the cluster, not on the applications
	/// scheduled across it.
	Cluster(Uuid),
}

impl Placement {
	/// The check-state scope this placement files at.
	fn scope(&self) -> Scope {
		match self {
			Self::Application(id) => Scope::Application(*id),
			Self::Group(id) => Scope::Group(*id),
			Self::Cluster(id) => Scope::Cluster(*id),
		}
	}
}

/// Resolve the coordinates a relay named to somewhere in canopy.
///
/// A relay's identity names its cluster: the registered `kubernetes_clusters`
/// row whose `relay_identity_id` is this connection's authenticated identity. A
/// relay whose identity resolves to no registered cluster — a draft the operator
/// has not finished, or one they removed — can place nothing, and the caller
/// logs the filing rather than dropping it silently.
///
/// From the cluster, each target resolves to a scope:
///
/// - [`FilingTarget::Cluster`] is the cluster itself, filed at
///   [`Scope::Cluster`]. This is what makes a cluster's substrate checks land
///   and, through filings arriving, what keeps the cluster reachable (spec
///   `CHK`, "Reachability").
/// - [`FilingTarget::Namespace`] names a group at a rank, and
///   [`FilingTarget::Instance`] names one application scheduled in the cluster.
///   Resolving either requires correlating what the relay reports against the
///   cluster's applications, which come from the harvest path. That path is not
///   wired yet (see [`ingest_harvest`] and `HarvestFiling`), and no cluster
///   application exists to correlate against until it is, so these are logged as
///   unplaceable in the meantime. The cluster grain above does not depend on
///   them.
pub async fn resolve(
	conn: &mut AsyncPgConnection,
	relay_identity_id: Uuid,
	target: &FilingTarget,
) -> Result<Option<Placement>> {
	// The connection is authenticated as the relay's identity, and a cluster is
	// derived from that identity rather than from anything the relay says. Only
	// a registered cluster hosts anything or carries checks: a draft is a
	// registration in progress, not a cluster in the registry.
	let Some(cluster) =
		KubernetesCluster::get_registered_by_relay_identity(conn, relay_identity_id).await?
	else {
		return Ok(None);
	};

	match target {
		FilingTarget::Cluster => Ok(Some(Placement::Cluster(cluster.id))),
		// The namespace-to-group and instance-to-application correlations arrive
		// with the harvest path; until a cluster application exists there is
		// nothing to resolve these against.
		FilingTarget::Namespace { .. } | FilingTarget::Instance { .. } => Ok(None),
	}
}

/// File what a relay reported.
pub async fn ingest(
	conn: &mut AsyncPgConnection,
	relay_identity_id: Uuid,
	filing: Filing,
	placement: Placement,
) -> Result<()> {
	match filing {
		Filing::Harvest(harvest) => {
			ingest_harvest(conn, relay_identity_id, harvest, placement).await
		}
		Filing::Substrate(substrate) => {
			ingest_substrate(conn, relay_identity_id, substrate, placement).await
		}
	}
}

/// A harvested filing, through the push ingestion.
///
/// Only an application can be the subject: the harvest's subject is the thing
/// that has a database, a version, an API, and duties that ought to be
/// running, and that is one application. A harvest filing placed anywhere else
/// is a protocol error rather than something to file at a coarser grain.
///
/// **Not yet wired.** Parity requires this to go through the same ingestion an
/// HTTP push takes, and that ingestion is a private function of the public
/// server's status handler, reachable only through its axum route. Lifting it
/// somewhere both callers can reach is the harvest card's, and doing it now
/// would mean lifting it against a payload shape that the cluster-host
/// question is about to settle. Unreachable in the meantime, `resolve`
/// placing nothing.
async fn ingest_harvest(
	_conn: &mut AsyncPgConnection,
	_relay_identity_id: Uuid,
	_harvest: HarvestFiling,
	placement: Placement,
) -> Result<()> {
	let Placement::Application(_) = placement else {
		return Err(AppError::custom(
			"a harvest filing describes one application, so it cannot be filed at a coarser grain",
		));
	};

	Err(AppError::custom(
		"harvest ingestion is not wired yet: it awaits the shared push-ingestion seam",
	))
}

/// A substrate filing, through canopy's own filing path.
async fn ingest_substrate(
	conn: &mut AsyncPgConnection,
	relay_identity_id: Uuid,
	substrate: SubstrateFiling,
	placement: Placement,
) -> Result<()> {
	let scope = placement.scope();

	// Provenance is the relay, and only where the scope is an application: it
	// is a separate concern from scope, and a group- or cluster-wide filing
	// carries none (see `CheckFiling::device_id`).
	let device_id = matches!(scope, Scope::Application(_)).then_some(relay_identity_id);

	// Each substrate filing is a single unlabelled instance: the cluster is now
	// its own scope rather than an instance of a canopy-wide check, so its
	// identity is the scope, not a label on the check.
	let label = String::new();

	let message = substrate.message.clone();
	file_check_instances(
		conn,
		InstancedCheckFiling {
			source: SUBSTRATE_SOURCE,
			scope,
			device_id,
			check: &substrate.check,
			title: substrate.title.as_deref(),
			default_ceiling: substrate.default_ceiling,
			default_escalates: substrate.default_escalates,
			documentation: substrate.documentation.as_deref(),
			instances: vec![CheckInstance {
				label,
				observed: substrate.observed,
				detail: substrate.detail.clone(),
			}],
		},
		&|_| message.clone(),
	)
	.await?;

	Ok(())
}

/// A filing canopy could not place, logged rather than dropped silently.
///
/// Worth a line each time: a coordinate that resolves to nothing means canopy
/// holds no application for that instance, and the check results for it are
/// going nowhere until it does.
pub fn unplaceable(relay_identity_id: Uuid, target: &FilingTarget) {
	warn!(
		relay = %relay_identity_id,
		?target,
		"relay filed against coordinates canopy cannot place; no application record carries them",
	);
}
