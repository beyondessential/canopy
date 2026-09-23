//! The relay binary: one per Kubernetes cluster.
//!
//! Deployed once per cluster from the infrastructure repository — namespace,
//! ServiceAccount and RBAC, the device-key Secret, and an initial image tag —
//! after which canopy keeps it current by naming the version it should run.
//! Standing up a new cluster is that install plus creating the relay identity
//! in canopy; no CI change, and the cluster inventory lives nowhere in this
//! repository.

use std::{error::Error, net::SocketAddr, path::PathBuf, sync::Arc};

use clap::Parser;
use lloggs::{LoggingArgs, PreArgs};
use relay::{Config, duties::Unattached};
use relay_protocol::Hello;
use tracing::{error, info};

/// The relay reports a startup failure as plain text and exits.
///
/// Deliberately not a diagnostic-rendering error type: nothing reads this but
/// a container log, and the crate carries no error framework for the sake of
/// one `main`.
type BoxError = Box<dyn Error + Send + Sync>;

/// lloggs' error type follows whichever feature the workspace build settles
/// on, so this never names it and takes its `Display` instead.
fn startup(context: &str, e: impl std::fmt::Display) -> BoxError {
	format!("{context}: {e}").into()
}

#[derive(Debug, Parser)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// PKCS#8 PEM file holding this relay's device key: the credential canopy
	/// minted when its device was created, mounted as a Secret.
	#[arg(long, env = "CANOPY_RELAY_KEY_FILE")]
	key_file: PathBuf,

	/// Canopy's public key, in hex, which this relay verifies on every
	/// connection. Not optional and not skippable: a relay that accepted an
	/// unverified peer would take instructions from it.
	#[arg(long, env = "CANOPY_RELAY_CANOPY_KEY")]
	canopy_key: String,

	/// Where canopy listens for relays.
	#[arg(long, env = "CANOPY_RELAY_CANOPY_ADDR")]
	canopy_addr: SocketAddr,

	/// The name presented in the TLS handshake. The pin is what identifies
	/// canopy, so this only has to be a name.
	#[arg(long, env = "CANOPY_RELAY_SERVER_NAME", default_value = "canopy")]
	server_name: String,
}

#[tokio::main]
async fn main() -> Result<(), BoxError> {
	let mut _guard = PreArgs::parse()
		.setup()
		.map_err(|e| startup("logging setup failed", e))?;
	let args = Args::parse();
	if _guard.is_none() {
		_guard = Some(
			args.logging
				.setup(|v| match v {
					0 => "info",
					1 => "debug",
					_ => "trace",
				})
				.map_err(|e| startup("logging setup failed", e))?,
		);
	}

	let config = Config::load(
		&args.key_file,
		&args.canopy_key,
		args.canopy_addr,
		args.server_name,
	)
	.map_err(|e| startup("relay configuration is unusable", e))?;

	info!(
		key = %relay_protocol::transport::hex(config.identity.spki()),
		floor = %config.floor,
		"relay starting; this is the key canopy must have enrolled",
	);

	let build = Hello {
		suite_version: "unattached".into(),
		relay_version: env!("CARGO_PKG_VERSION").into(),
		version_floor: config.floor.to_string(),
	};
	let duties = Arc::new(Unattached::new(build));

	let (filings, filings_rx) = tokio::sync::mpsc::channel(256);

	// The cluster checks read the cluster through the relay's own
	// ServiceAccount. A relay that cannot build a client still connects and
	// answers, so canopy sees it and can say what is wrong, but it files
	// nothing about the cluster, and the cluster reads unreachable.
	match kube::Client::try_default().await {
		Ok(client) => {
			tokio::spawn(relay::cluster::watch::run(client, filings.clone()));
		}
		Err(err) => error!("cannot reach this cluster's API, so no cluster checks run: {err}"),
	}
	// Held so the channel stays open whatever the checks do.
	let _filings = filings;

	relay::run(config, duties, filings_rx).await;
	Ok(())
}
