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
use commons_types::{Uuid, source::SUBSTRATE_SOURCE, status::CheckResult};
use database::{
	KubernetesCluster,
	diesel_async::AsyncPgConnection,
	issues::{CheckInstance, CheckOutcome, InstancedCheckFiling, Scope, file_check_instances},
};
use relay_protocol::{
	Filing, FilingTarget, HarvestFiling, SubstrateFiling, SubstrateInstance, SubstrateOutcome,
};
use tracing::warn;

/// Where a filing lands, once the coordinates the relay named have been
/// resolved against what canopy knows.
///
/// This is the bridge from the relay's vocabulary (namespaces and instances)
/// to canopy's (applications, groups, clusters). Resolution happens here
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
///
/// What a check with instances says is canopy's to write, from the instances as
/// it graded them, as for any check with instances: the relay's own account of
/// them would count an instance a silence has since taken out. A check that
/// holds once, or that the relay could not read, is described by the relay, as
/// canopy's own plain determinations are described by whatever files them.
// spec: CHK#checks-with-instances
async fn ingest_substrate(
	conn: &mut AsyncPgConnection,
	relay_identity_id: Uuid,
	substrate: SubstrateFiling,
	placement: Placement,
) -> Result<()> {
	let (detail, outcome) = match substrate.outcome {
		// Brokenness is the whole check's, so its fields are the ones every
		// instance the check holds shares.
		SubstrateOutcome::Broken { detail } => (
			detail.and_then(|d| match d {
				serde_json::Value::Object(fields) => Some(fields),
				_ => None,
			}),
			CheckOutcome::Broken,
		),
		SubstrateOutcome::Instances(instances) => {
			refuse_malformed(&instances)?;
			(
				None,
				CheckOutcome::Instances(
					instances
						.into_iter()
						.map(|i| CheckInstance {
							key: i.key,
							label: i.label,
							observed: i.observed,
							detail: i.detail,
						})
						.collect(),
				),
			)
		}
	};

	let scope = placement.scope();

	// Provenance is the relay, and only where the scope is an application: it
	// is a separate concern from scope, and a group- or cluster-wide filing
	// carries none (see `CheckFiling::device_id`).
	let device_id = matches!(scope, Scope::Application(_)).then_some(relay_identity_id);

	let message = substrate.message;
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
			detail,
			outcome,
		},
		&|graded| match &message {
			Some(message) if graded.broken || graded.is_plain() => message.clone(),
			_ => graded.message(&substrate.check),
		},
	)
	.await?;

	Ok(())
}

/// Refuse a set of instances canopy cannot hold as the relay sent it.
///
/// The instances are the check's complete set, so a filing naming none would
/// say the condition has no instances at all rather than anything about them,
/// and the relay never sends one. An instance is told apart by its key, which
/// is empty only for the one instance of a check that holds once, so an empty
/// key beside others or a key named twice says nothing canopy could hold. And
/// an instance is never broken: a check the relay cannot read is broken as a
/// whole.
fn refuse_malformed(instances: &[SubstrateInstance]) -> Result<()> {
	if instances.is_empty() {
		return Err(AppError::custom(
			"a substrate filing names no instances, so there is nothing to file",
		));
	}
	if instances.iter().any(|i| i.observed == CheckResult::Broken) {
		return Err(AppError::custom(
			"a substrate instance cannot be broken: a check that cannot be read is broken as a whole",
		));
	}
	if instances.len() > 1 && instances.iter().any(|i| i.key.is_empty()) {
		return Err(AppError::custom(
			"a substrate instance beside others has an empty key",
		));
	}
	let mut keys: Vec<&str> = instances.iter().map(|i| i.key.as_str()).collect();
	keys.sort_unstable();
	if keys.windows(2).any(|w| w[0] == w[1]) {
		return Err(AppError::custom(
			"a substrate filing names one instance key twice",
		));
	}
	Ok(())
}

/// A filing canopy could not place, logged rather than dropped silently.
///
/// Worth a line each time: a coordinate that resolves to nothing means canopy
/// holds nothing for it (no registered cluster for the relay, or no
/// application or group for the namespace it named), and the check results for
/// it are going nowhere until it does.
pub fn unplaceable(relay_identity_id: Uuid, target: &FilingTarget) {
	warn!(
		relay = %relay_identity_id,
		?target,
		"relay filed against coordinates canopy cannot place",
	);
}
