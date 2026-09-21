//! The connections canopy is holding, one per relay.
//!
//! Canopy is the listener here, not a client: there is one relay per
//! registered cluster and each dials in, so what canopy has is N inbound
//! connections it did not initiate. This is the record of them, and it is what
//! answers "is that cluster connected and answering" for cluster registration
//! and for the cluster-connectivity check.
//!
//! Entries are keyed by the **authenticated relay device**. A connection's
//! cluster is derived from that device row, never from anything the relay says
//! in a message, so which cluster canopy is talking to is a lookup rather than
//! an assertion to trust.

use std::{collections::HashMap, sync::Arc};

use commons_errors::{AppError, Result};
use commons_types::Uuid;
use jiff::Timestamp;
use relay_protocol::{
	Hello, Request, Response,
	frame::{read_required_frame, write_frame},
};
use tokio::sync::RwLock;
use tracing::debug;

/// One live relay connection.
#[derive(Clone)]
pub struct Connected {
	/// The relay's device, which is what identifies its cluster.
	pub device_id: Uuid,
	pub connection: quinn::Connection,
	/// What the relay reported it is running, read once the connection was
	/// authenticated.
	pub build: Hello,
	pub since: Timestamp,
}

impl std::fmt::Debug for Connected {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("Connected")
			.field("device_id", &self.device_id)
			.field("remote", &self.connection.remote_address())
			.field("build", &self.build)
			.field("since", &self.since)
			.finish()
	}
}

/// The relays canopy currently holds a connection to.
#[derive(Clone, Default)]
pub struct Registry(Arc<RwLock<HashMap<Uuid, Connected>>>);

impl Registry {
	pub fn new() -> Self {
		Self::default()
	}

	/// Record a newly authenticated connection.
	///
	/// A relay that reconnects while canopy still holds its previous
	/// connection replaces it: the new one is the one that works, and the
	/// stale entry would otherwise answer for a socket that is gone. The
	/// displaced connection is closed rather than left to linger.
	pub async fn insert(&self, connected: Connected) {
		let mut held = self.0.write().await;
		if let Some(previous) = held.insert(connected.device_id, connected) {
			debug!(
				device_id = %previous.device_id,
				"relay reconnected; closing the connection it replaced",
			);
			previous
				.connection
				.close(0u32.into(), b"replaced by a newer connection");
		}
	}

	/// Drop a connection, if the entry is still the one that was registered.
	///
	/// The generation check matters: a relay that reconnects fast enough can
	/// have its new entry already in place when the old connection's task
	/// notices it is gone, and an unconditional remove would delete the live
	/// entry on the way out.
	pub async fn remove(&self, device_id: Uuid, connection: &quinn::Connection) {
		let mut held = self.0.write().await;
		if held
			.get(&device_id)
			.is_some_and(|c| c.connection.stable_id() == connection.stable_id())
		{
			held.remove(&device_id);
		}
	}

	pub async fn get(&self, device_id: Uuid) -> Option<Connected> {
		self.0.read().await.get(&device_id).cloned()
	}

	/// Every relay currently connected.
	pub async fn connected(&self) -> Vec<Connected> {
		self.0.read().await.values().cloned().collect()
	}

	/// Ask a relay something, on a stream opened for this exchange.
	///
	/// The stream is the correlation: it carries one request and one response
	/// and is then done, so a slow answer holds nothing else up and a caller
	/// that gives up resets only its own stream.
	pub async fn request(&self, device_id: Uuid, request: Request) -> Result<Response> {
		let Some(connected) = self.get(device_id).await else {
			return Err(AppError::custom(format!(
				"relay {device_id} is not connected",
			)));
		};

		let (mut send, mut recv) =
			connected.connection.open_bi().await.map_err(|e| {
				AppError::custom(format!("opening a stream to relay {device_id}: {e}"))
			})?;

		write_frame(&mut send, &request)
			.await
			.map_err(|e| AppError::custom(format!("asking relay {device_id}: {e}")))?;
		send.finish()
			.map_err(|e| AppError::custom(format!("finishing the request to {device_id}: {e}")))?;

		let response: Response = read_required_frame(&mut recv)
			.await
			.map_err(|e| AppError::custom(format!("reading relay {device_id}'s answer: {e}")))?;

		// A response of the wrong shape is a protocol failure, not an answer:
		// a caller must not read "asleep" as the roster it asked for.
		if !response.answers(&request) {
			return Err(AppError::custom(format!(
				"relay {device_id} answered {response:?} to {request:?}",
			)));
		}

		Ok(response)
	}
}
