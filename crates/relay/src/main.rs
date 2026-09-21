//! The relay binary: one per Kubernetes cluster.
//!
//! Deployed once per cluster from the infrastructure repository — namespace,
//! ServiceAccount and RBAC, the device-key Secret, and an initial image tag —
//! after which canopy keeps it current by naming the version it should run.
//! Standing up a new cluster is that deployment plus creating the relay device
//! in canopy; no CI change, and the cluster inventory lives nowhere in this
//! repository.

use std::{net::SocketAddr, path::PathBuf, sync::Arc};

use clap::Parser;
use lloggs::{LoggingArgs, PreArgs};
use miette::{Result, miette};
use relay::{Config, duties::Unattached};
use relay_protocol::Hello;
use tracing::info;

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
async fn main() -> Result<()> {
	let mut _guard = PreArgs::parse().setup()?;
	let args = Args::parse();
	if _guard.is_none() {
		_guard = Some(args.logging.setup(|v| match v {
			0 => "info",
			1 => "debug",
			_ => "trace",
		})?);
	}

	let config = Config::load(
		&args.key_file,
		&args.canopy_key,
		args.canopy_addr,
		args.server_name,
	)
	.map_err(|e| miette!("relay configuration is unusable: {e}"))?;

	info!(
		key = %relay_protocol::transport::hex(config.identity.spki()),
		floor = %config.floor,
		"relay starting; this is the key canopy must have enrolled",
	);

	// The check families are what produce filings and what reads the cluster.
	// Until they land this relay connects, authenticates, and answers — which
	// is what makes the transport and the enrollment testable against a real
	// canopy before there is a check to run.
	let build = Hello {
		suite_version: "unattached".into(),
		relay_version: env!("CARGO_PKG_VERSION").into(),
		version_floor: config.floor.to_string(),
	};
	let duties = Arc::new(Unattached::new(build));

	// Nothing files yet, so the sender is held here to keep the channel open;
	// the check families take it when they arrive.
	let (filings_tx, filings_rx) = tokio::sync::mpsc::channel(256);
	let _filings = filings_tx;

	relay::run(config, duties, filings_rx).await;
	Ok(())
}
