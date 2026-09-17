//! Sweeps the artifact store of bytes no artifact reaches: a registration whose
//! row write never landed after its bytes did, and what a revert of the group
//! scoping leaves behind.
//!
//! Not an age rule. A group can sit on one version for a year without a rebuild,
//! so expiring by age alone would take live bytes out from under a registered row.
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

/// How old an object has to be before the sweep will consider it. A registration
/// stores the bytes before the row that names them, so anything younger may have
/// one still in flight behind it.
pub const GRACE: Duration = Duration::from_secs(24 * 3600);

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

	// After the listing, never before: an artifact registered while it ran is
	// then in here, where the other order has it in neither and sweeps it.
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
		n => warn!("artifact-sweep: dropped {n} artifact(s) no registration reached"),
	}
}

/// With no store configured the pod carries on rather than refusing to start:
/// the servers are what report an artifact they cannot hold.
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
