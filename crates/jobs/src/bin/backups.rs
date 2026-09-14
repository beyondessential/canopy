//! Backups scheduler pod. One long-lived Deployment that runs all four backup
//! control-plane loops against the same database, each as an independent spawned
//! task:
//!
//! - upstream preflight ([`jobs::backup::preflight`]),
//! - kopia maintenance scheduler ([`jobs::backup::maintenance`]),
//! - read-only inspection scheduler ([`jobs::backup::inspection`]),
//! - passphrase rotation loop ([`jobs::backup::rotation`]),
//! - S3 CloudWatch metrics task ([`jobs::backup::s3_metrics`]),
//! - progress-series pruning ([`jobs::backup::progress_prune`]) — fleet-wide, so
//!   unlike the loops above it never contends with a group's own backup work,
//! - recovery vault writer ([`jobs::backup::recovery_snapshot`]) — encrypts a state snapshot
//!   to `age` recipients and writes it to object-locked S3. Its recipients are
//!   mandatory, so the pod refuses to start if they're unset.
//!
//! The maintenance + inspection loops run kopia **in-process** (subprocesses)
//! rather than spawning Kubernetes Jobs; they share a [`jobs::backup::worker::Worker`]
//! (DB pool, kube client for Secret reads, concurrency semaphore, in-flight
//! group set, and the container-creds endpoint), built once here. Preflight and
//! s3-metrics keep building their own pool + AWS clients.
//!
//! Each kopia subprocess gets its group's maintenance-role creds by polling the
//! loopback container-credentials endpoint ([`jobs::backup::creds_server`]),
//! which mints + refreshes them from the pod's shared `canopy-jobs` IRSA
//! identity.

use clap::Parser;
use lloggs::{LoggingArgs, PreArgs};
use miette::{IntoDiagnostic, miette};
use tracing::info;

use commons_servers::backup_secrets::BackupSecrets;
use jobs::backup::creds_server::CredsServer;
use jobs::backup::worker::{Cfg, Worker};

#[derive(Debug, Parser)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,
}

#[tokio::main]
async fn main() -> miette::Result<()> {
	let mut _guard = PreArgs::parse().setup()?;
	let args = Args::parse();
	if _guard.is_none() {
		_guard = Some(args.logging.setup(|v| match v {
			0 => "info",
			1 => "debug",
			_ => "trace",
		})?);
	}
	info!("backups scheduler starting");

	// Shared worker state for the in-process kopia loops, built once.
	let pool = database::init();
	let secrets = BackupSecrets::try_default()
		.await
		.ok_or_else(|| miette!("secret store unavailable; cannot read/rotate repo passphrases"))?;
	let creds = CredsServer::start()
		.await
		.map_err(|e| miette!("container-creds endpoint init failed: {e}"))?;
	let worker = Worker::new(pool, secrets, Cfg::from_env(), creds);

	// recovery vault recipients are MANDATORY — refuse to start without them so there's
	// never a silent recovery gap (Canopy owns every passphrase).
	let recovery_config = jobs::backup::recovery_snapshot::RecoveryVaultConfig::from_env()
		.map_err(|e| miette!("recovery vault misconfigured: {e}"))?;

	let preflight = jobs::backup::preflight::spawn();
	let maintenance = jobs::backup::maintenance::spawn(worker.clone());
	let inspection = jobs::backup::inspection::spawn(worker.clone());
	let rotation = jobs::backup::rotation::spawn(worker.clone());
	let s3_metrics = jobs::backup::s3_metrics::spawn();
	let tag_reconcile = jobs::backup::tag_reconcile::spawn();
	let progress_prune = jobs::backup::progress_prune::spawn();
	let recovery_snapshot = jobs::backup::recovery_snapshot::spawn(worker, recovery_config);

	let artifact_sweep = jobs::artifact_sweep::spawn().await;

	tokio::try_join!(
		preflight,
		maintenance,
		inspection,
		rotation,
		s3_metrics,
		tag_reconcile,
		progress_prune,
		recovery_snapshot,
		artifact_sweep
	)
	.into_diagnostic()?;
	Ok(())
}
