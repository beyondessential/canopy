//! Sweeps the artifact store of bytes no artifact reaches.
//!
//! Canopy keeps none of what it has stopped serving, and a deregistration drops
//! an artifact's bytes as it drops its row. What this catches is the residue
//! nothing else can: a registration whose row write never landed after the bytes
//! did, and every held artifact's bytes after a revert of the migration that
//! gave artifacts a group.
//!
//! Deliberately not an age rule on the store itself. An artifact's bytes live as
//! long as its registration, and a group can sit on one version for a year
//! without a rebuild, so anything expiring by age alone would take live bytes
//! out from under a registered row and answer every device with a 404.
// spec: ART#where-an-artifact-rests

use std::time::Duration;

use commons_servers::artifact_store::ArtifactStore;
use database::artifacts::Artifact;
use jiff::Timestamp;
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, warn};

/// How old an object has to be before the sweep will consider it.
///
/// A registration stores the bytes before it writes the row that names them, so
/// an object younger than this may have a registration still in flight behind
/// it. Wide enough to cover any upload, since the cost of waiting is a day of
/// storage and the cost of being wrong is an artifact that was registered
/// successfully and cannot be served.
pub const GRACE: Duration = Duration::from_secs(24 * 3600);

/// How often to sweep. The residue accrues a failed registration at a time, so
/// there is nothing to gain from looking often.
const TICK: Duration = Duration::from_secs(6 * 3600);

/// One pass. Public so a test can drive it without waiting out [`TICK`].
pub async fn tick(
	db: &mut database::diesel_async::AsyncPgConnection,
	store: &ArtifactStore,
	now: Timestamp,
) {
	let stored = match store.stored().await {
		Ok(stored) => stored,
		Err(e) => {
			warn!("artifact-sweep: listing the store failed: {e}");
			return;
		}
	};

	// Read the rows after listing the store, never before: an artifact
	// registered while the listing ran is in here and so is not swept, where the
	// other order would have it missing from both and drop bytes that had just
	// arrived.
	let held = match Artifact::held_ids(db).await {
		Ok(held) => held,
		Err(e) => {
			warn!("artifact-sweep: reading the registered artifacts failed: {e}");
			return;
		}
	};

	let cutoff = now - GRACE;
	let mut swept = 0usize;
	for (artifact, stored_at) in stored {
		if held.contains(&artifact) || stored_at > cutoff {
			continue;
		}
		match store.delete(artifact).await {
			Ok(()) => swept += 1,
			Err(e) => warn!(%artifact, "artifact-sweep: delete failed: {e}"),
		}
	}

	match swept {
		0 => debug!("artifact-sweep: nothing to sweep"),
		n => warn!(
			"artifact-sweep: dropped {n} artifact(s) no registration reached; a registration \
			 failing after its bytes were stored is what leaves these"
		),
	}
}

/// Where no store is configured there is nothing holding artifacts to sweep, so
/// the pod carries on without one rather than refusing to start: the servers are
/// what report an artifact they cannot hold.
pub async fn spawn() -> JoinHandle<()> {
	let Some(store) = ArtifactStore::try_default().await else {
		warn!("artifact-sweep: no artifact store configured; not sweeping");
		return task::spawn(std::future::pending());
	};

	let pool = database::init();
	task::spawn(async move {
		loop {
			sleep(TICK).await;
			let Ok(mut db) = pool.get().await else {
				error!("Failed to get database connection");
				continue;
			};
			tick(&mut db, &store, Timestamp::now()).await;
		}
	})
}
