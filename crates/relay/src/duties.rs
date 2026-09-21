//! What a relay can actually do in its cluster.
//!
//! Everything canopy may ask of a relay passes through here, which makes this
//! the list of canopy's authority over a cluster in one place. Widening it is
//! a deliberate act with a visible diff, rather than something that happens by
//! a handler reaching for a Kubernetes client.
//!
//! None of these is a check. Checks are `alertd`'s, both families, and reach
//! canopy as filings; this is only what canopy asks for by name. The
//! implementations are cluster work: the roster comes from listing a
//! namespace, sleep and wake from scaling workloads and hibernating a
//! database, the version from patching this relay's own Deployment.
//! [`Unattached`] stands in until they land — it answers every request as a
//! failure, which is honest about a relay that is connected but cannot yet
//! read its cluster.

use relay_protocol::{Hello, RosterEntry};

/// The cluster work behind canopy's requests.
///
/// Note what is absent: nothing here returns a Kubernetes object, and there is
/// no method that reads arbitrary cluster state. That is the boundary the
/// relay design exists to hold — a compromised canopy can obtain check results
/// and take the two deployment actions, and cannot obtain the cluster's
/// objects or any instance's data, because no method offers them.
pub trait Duties: Send + Sync + 'static {
	/// The instances running in a namespace, for the identity picker.
	fn roster(
		&self,
		namespace: &str,
	) -> impl Future<Output = Result<Vec<RosterEntry>, DutyError>> + Send;

	/// What this relay is running.
	fn build(&self) -> Hello;

	/// Put a namespace's deployment to sleep.
	///
	/// The relay is what refuses a deployment carrying no scheduled expiry
	/// ([`DutyError::NoScheduledExpiry`]), because the expiry is a fact of the
	/// namespace: enforcing it here means the restriction holds where the fact
	/// is known rather than resting on canopy asking correctly.
	fn sleep(&self, namespace: &str) -> impl Future<Output = Result<(), DutyError>> + Send;

	/// Wake a sleeping deployment.
	fn wake(&self, namespace: &str) -> impl Future<Output = Result<(), DutyError>> + Send;

	/// Move this relay onto the named version by patching its own Deployment's
	/// image tag, leaving the rollout to Kubernetes.
	///
	/// The floor is checked before this is called, so an implementation is
	/// being told a version it has already been cleared to run.
	fn run_version(&self, version: &str) -> impl Future<Output = Result<(), DutyError>> + Send;
}

#[derive(Debug, thiserror::Error)]
pub enum DutyError {
	/// The deployment has no scheduled expiry, so it cannot be put to sleep.
	#[error("{namespace} has no scheduled expiry, so it cannot be put to sleep")]
	NoScheduledExpiry { namespace: String },

	/// The namespace is not one this relay serves, or is not there.
	#[error("{namespace} is not a namespace this relay serves")]
	UnknownNamespace { namespace: String },

	/// The relay tried and the cluster refused, or something else went wrong.
	#[error("{0}")]
	Failed(String),
}

/// A relay that holds no cluster access yet.
///
/// Stands in until the check families and the cluster actions land. It
/// connects, authenticates, and answers — with a failure, naming what is
/// missing — so the transport is exercisable and a misconfigured deployment
/// reads as "this relay cannot do anything yet" rather than as a silence that
/// looks like a network fault.
pub struct Unattached {
	build: Hello,
}

impl Unattached {
	pub fn new(build: Hello) -> Self {
		Self { build }
	}
}

impl Duties for Unattached {
	async fn roster(&self, _namespace: &str) -> Result<Vec<RosterEntry>, DutyError> {
		Err(Self::no_cluster_access())
	}

	fn build(&self) -> Hello {
		self.build.clone()
	}

	async fn sleep(&self, _namespace: &str) -> Result<(), DutyError> {
		Err(Self::no_cluster_access())
	}

	async fn wake(&self, _namespace: &str) -> Result<(), DutyError> {
		Err(Self::no_cluster_access())
	}

	async fn run_version(&self, _version: &str) -> Result<(), DutyError> {
		Err(Self::no_cluster_access())
	}
}

impl Unattached {
	fn no_cluster_access() -> DutyError {
		DutyError::Failed("this relay build holds no cluster access".into())
	}
}
