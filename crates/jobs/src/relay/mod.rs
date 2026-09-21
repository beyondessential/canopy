//! Canopy's end of the relay connections (spec `K8S`).
//!
//! Canopy listens; it never dials a cluster. A relay opens its connection
//! outward from inside the cluster, which is what lets canopy hold no
//! credential to that cluster and the cluster accept no connection from
//! canopy. Canopy's authority over a cluster is therefore exactly the set of
//! requests its relay answers.
//!
//! ## What authenticates a connection
//!
//! The device key presented in the QUIC handshake, and nothing else. Canopy
//! terminates TLS itself here, so the handshake proves possession of the key;
//! canopy then looks the presented `SubjectPublicKeyInfo` up in `device_keys`
//! and requires the device to carry the relay role.
//!
//! Nothing on this path may assume "reached us, therefore trusted". A relay
//! usually arrives over an overlay network, and that network's access control
//! is worth having, but it is not the gate and this code does not consult it.
//! An unauthenticated connection is closed before a single stream is accepted.
//!
//! ## The singleton this accepts
//!
//! One worker holds every relay connection, so losing it makes every cluster
//! unreadable at once. That surfaces correctly — the cluster-connectivity
//! check fails for every registered cluster — but it is a single point of
//! failure the design accepts rather than one it has overlooked.

use std::net::SocketAddr;

use commons_errors::Result;
use commons_types::{Uuid, device::DeviceRole};
use database::{Db, devices::Device};
use jiff::Timestamp;
use relay_protocol::{
	Filing, FilingTarget, Hello, Request, Response,
	frame::read_required_frame,
	transport::{self, Identity},
};
use tracing::{debug, info, warn};

pub mod ingest;
pub mod registry;

pub use registry::{Connected, Registry};

/// Why a connection was refused. Each is a flat close from the relay's point
/// of view; the distinction is for canopy's logs, where "a key canopy has
/// never seen" and "a key whose device is not a relay" want different
/// responses from an operator.
#[derive(Debug, thiserror::Error)]
enum Rejected {
	#[error("the peer presented no usable certificate: {0}")]
	NoIdentity(String),

	#[error("no active device key matches the presented key {0}")]
	UnknownKey(String),

	#[error("device {device_id} carries the {role} role, not relay")]
	NotARelay { device_id: Uuid, role: DeviceRole },

	#[error("the connection settled on no protocol canopy speaks: {0}")]
	NoProtocol(String),

	#[error("the relay did not answer what it is running: {0}")]
	SilentOnBuild(String),
}

/// Listen for relays, and hold each connection for as long as it lasts.
///
/// Runs until the endpoint is closed. Each connection is handled in its own
/// task: one relay's slow or broken connection must not hold up another
/// cluster's.
pub async fn listen(db: Db, registry: Registry, endpoint: quinn::Endpoint) {
	let addr = endpoint
		.local_addr()
		.map(|a| a.to_string())
		.unwrap_or_else(|_| "?".into());
	info!("listening for relays on {addr}");

	while let Some(incoming) = endpoint.accept().await {
		let db = db.clone();
		let registry = registry.clone();
		tokio::spawn(async move {
			let remote = incoming.remote_address();
			let connection = match incoming.await {
				Ok(connection) => connection,
				Err(err) => {
					debug!("relay handshake from {remote} failed: {err}");
					return;
				}
			};

			match authenticate(&db, &connection).await {
				Ok(device_id) => {
					if let Err(err) = hold(db, registry, connection, device_id).await {
						debug!(relay = %device_id, "relay connection ended: {err}");
					}
				}
				Err(rejected) => {
					// Worth a warning rather than a debug line: a peer that got
					// through the handshake and then failed the device-key gate
					// is either a misconfigured relay or something that should
					// not be dialling canopy at all.
					warn!("refusing a relay connection from {remote}: {rejected}");
					connection.close(0u32.into(), b"not an enrolled relay");
				}
			}
		});
	}

	info!("stopped listening for relays");
}

/// The device a connection belongs to, or why it is refused.
///
/// The same SPKI lookup the HTTP mTLS path performs, against the same column,
/// so one store answers for both paths and a revoked key stops working
/// everywhere at once (`Device::from_key` matches only active keys).
async fn authenticate(
	db: &Db,
	connection: &quinn::Connection,
) -> std::result::Result<Uuid, Rejected> {
	let spki = transport::peer_spki(connection).map_err(|e| Rejected::NoIdentity(e.to_string()))?;

	// Refuse before the lookup if the two ends did not settle on a version
	// canopy speaks. It should not be reachable — ALPN would have failed the
	// handshake — but a connection whose version canopy cannot name is not one
	// to start reading messages from.
	transport::negotiated_version(connection).map_err(|e| Rejected::NoProtocol(e.to_string()))?;

	let mut conn = db
		.get()
		.await
		.map_err(|e| Rejected::NoIdentity(format!("no database connection: {e}")))?;
	let device = Device::from_key(&mut conn, &spki)
		.await
		.map_err(|e| Rejected::NoIdentity(format!("looking up the presented key: {e}")))?
		.ok_or_else(|| Rejected::UnknownKey(transport::hex(&spki)))?;

	if device.role != DeviceRole::Relay {
		return Err(Rejected::NotARelay {
			device_id: device.id,
			role: device.role,
		});
	}

	Ok(device.id)
}

/// Hold an authenticated connection: record what the relay is running, then
/// take its filings until the connection ends.
async fn hold(
	db: Db,
	registry: Registry,
	connection: quinn::Connection,
	device_id: Uuid,
) -> Result<()> {
	let build = ask_build(&connection)
		.await
		.map_err(|e| commons_errors::AppError::custom(e.to_string()))?;
	info!(
		relay = %device_id,
		relay_version = %build.relay_version,
		suite_version = %build.suite_version,
		"relay connected",
	);

	registry
		.insert(Connected {
			device_id,
			connection: connection.clone(),
			build,
			since: Timestamp::now(),
		})
		.await;

	let outcome = take_filings(&db, &connection, device_id).await;

	registry.remove(device_id, &connection).await;
	info!(relay = %device_id, "relay disconnected");
	outcome
}

/// Ask a freshly authenticated relay what it is running.
///
/// Done here rather than left to a caller because the registry entry carries
/// it: the skew alert and an operator looking at a cluster both read what
/// canopy already holds, instead of each costing a round trip.
async fn ask_build(connection: &quinn::Connection) -> std::result::Result<Hello, Rejected> {
	// Ask on this connection directly: the registry entry does not exist yet,
	// and it is this connection's answer that belongs in it.
	let (mut send, mut recv) = connection
		.open_bi()
		.await
		.map_err(|e| Rejected::SilentOnBuild(e.to_string()))?;
	relay_protocol::frame::write_frame(&mut send, &Request::Build)
		.await
		.map_err(|e| Rejected::SilentOnBuild(e.to_string()))?;
	send.finish()
		.map_err(|e| Rejected::SilentOnBuild(e.to_string()))?;

	let response: Response = read_required_frame(&mut recv)
		.await
		.map_err(|e| Rejected::SilentOnBuild(e.to_string()))?;

	match response {
		Response::Build(hello) => Ok(hello),
		other => Err(Rejected::SilentOnBuild(format!(
			"answered {other:?} when asked what it is running",
		))),
	}
}

/// Read filings until the relay goes away.
///
/// Each filing arrives on its own unidirectional stream, so one that is slow
/// to arrive or malformed costs only itself. A stream that fails is logged and
/// abandoned; the connection carries on, because the next refile will bring
/// the same state again.
async fn take_filings(db: &Db, connection: &quinn::Connection, device_id: Uuid) -> Result<()> {
	loop {
		let stream = match connection.accept_uni().await {
			Ok(stream) => stream,
			// The relay closing or going away is the normal end of this loop.
			Err(err) => return Err(commons_errors::AppError::custom(err.to_string())),
		};

		let db = db.clone();
		tokio::spawn(async move {
			if let Err(err) = take_one_filing(&db, stream, device_id).await {
				warn!(relay = %device_id, "discarding a filing: {err}");
			}
		});
	}
}

async fn take_one_filing(db: &Db, mut stream: quinn::RecvStream, device_id: Uuid) -> Result<()> {
	let filing: Filing = read_required_frame(&mut stream)
		.await
		.map_err(|e| commons_errors::AppError::custom(e.to_string()))?;

	let target = match &filing {
		// A harvest filing is always about one instance, so its coordinates are
		// an instance in a namespace by construction rather than by validation.
		Filing::Harvest(harvest) => FilingTarget::Instance {
			namespace: harvest.namespace.clone(),
			instance: harvest.instance.clone(),
		},
		Filing::Substrate(substrate) => substrate.target.clone(),
	};

	let mut conn = db.get().await?;
	let Some(placement) = ingest::resolve(&mut conn, device_id, &target).await? else {
		ingest::unplaceable(device_id, &target);
		return Ok(());
	};

	ingest::ingest(&mut conn, device_id, filing, placement).await
}

/// Build the listening endpoint canopy accepts relays on.
pub fn endpoint(identity: &Identity, addr: SocketAddr) -> Result<quinn::Endpoint> {
	let config = transport::server_config(identity)
		.map_err(|e| commons_errors::AppError::custom(e.to_string()))?;
	quinn::Endpoint::server(config, addr)
		.map_err(|e| commons_errors::AppError::custom(format!("binding {addr} for relays: {e}")))
}
