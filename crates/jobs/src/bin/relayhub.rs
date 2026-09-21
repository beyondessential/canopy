//! The relay hub pod: canopy's end of every relay connection (spec `K8S`).
//!
//! One long-lived Deployment that listens for relays and holds their
//! connections. It never dials a cluster — a relay dials in — so this pod
//! holds no credential to any cluster, and a cluster canopy can read is a
//! relay that has connected.
//!
//! Its identity is a keypair mounted into the pod, whose public half every
//! relay pins. Refusing to start without it is deliberate: a hub that
//! generated its own key on boot would present a key no relay recognises, so
//! every cluster would go unreadable in a way that looks like a fleet-wide
//! outage rather than a missing secret.

use std::net::SocketAddr;

use clap::Parser;
use jobs::relay::{self, Registry};
use lloggs::{LoggingArgs, PreArgs};
use miette::{IntoDiagnostic, miette};
use relay_protocol::transport::Identity;
use tracing::info;

#[derive(Debug, Parser)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,

	/// Where to listen for relays. UDP, because the transport is QUIC.
	#[arg(long, env = "CANOPY_RELAY_LISTEN", default_value = "[::]:8443")]
	listen: SocketAddr,

	/// PKCS#8 PEM file holding canopy's own key for the relay transport. Its
	/// public half is what each relay is configured to pin, so replacing it
	/// means reconfiguring every relay.
	#[arg(long, env = "CANOPY_RELAY_KEY_FILE")]
	key_file: std::path::PathBuf,
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

	let key_pem = std::fs::read_to_string(&args.key_file)
		.into_diagnostic()
		.map_err(|e| miette!("reading {}: {e}", args.key_file.display()))?;
	let identity = Identity::from_pkcs8_pem(&key_pem)
		.map_err(|e| miette!("canopy's relay key is not usable: {e}"))?;
	info!(
		key = %relay_protocol::transport::hex(identity.spki()),
		"relay hub identity loaded; this is the key relays pin",
	);

	let endpoint = relay::endpoint(&identity, args.listen)
		.map_err(|e| miette!("cannot listen for relays: {e}"))?;

	relay::listen(database::init(), Registry::new(), endpoint).await;
	Ok(())
}
