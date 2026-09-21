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
	/// The group a namespace names — a deployment at a rank.
	Group(Uuid),
	/// Canopy-wide, with the relay's cluster as the check's instance.
	Cluster { label: String },
}

impl Placement {
	/// The check-state scope this placement files at.
	fn scope(&self) -> Scope {
		match self {
			Self::Application(id) => Scope::Application(*id),
			Self::Group(id) => Scope::Group(*id),
			Self::Cluster { .. } => Scope::Global,
		}
	}
}

/// Resolve the coordinates a relay named to somewhere in canopy.
///
/// **Not yet implemented, and deliberately so.** What an application scheduled
/// across a cluster rather than run on a box belongs to is the open question
/// the Kubernetes project carries (see `FLT`, "Cardinality"): an application
/// runs on exactly one machine today, and a relay's applications have none.
/// Until a cluster is something an application can be hosted by, and the
/// registry that names one exists, every filing is unplaceable — which the
/// caller logs.
///
/// This is the one function the cluster registry and the identity work fill
/// in; nothing else in the relay path needs to change for a filing to start
/// landing.
pub async fn resolve(
	_conn: &mut AsyncPgConnection,
	_relay_identity_id: Uuid,
	_target: &FilingTarget,
) -> Result<Option<Placement>> {
	Ok(None)
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
	// is a separate concern from scope, and a group- or canopy-wide filing
	// carries none (see `CheckFiling::device_id`).
	let device_id = matches!(scope, Scope::Application(_)).then_some(relay_identity_id);

	// A cluster-wide check is Canopy-wide with each cluster an instance of it,
	// so the cluster is the instance label rather than part of the check name.
	// Everything else is a single unlabelled instance, which is what
	// `file_check` files.
	let label = match &placement {
		Placement::Cluster { label } => label.clone(),
		_ => String::new(),
	};

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
