//! The relay's end of the connection: dial canopy, answer what it asks, file
//! upward, and reconnect when the connection goes.
//!
//! The connection is continuous by design, so canopy observes the loss of a
//! relay directly rather than inferring it from a request failing. That puts
//! the obligation here: a relay that gives up on reconnecting is a cluster
//! canopy cannot read, and it must keep trying without hammering.

use std::{sync::Arc, time::Duration};

use relay_protocol::{
	Filing, Refusal, RefusalKind, Request, Response,
	frame::{read_required_frame, write_frame},
	transport::client_config,
};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

use crate::{
	config::Config,
	duties::{Duties, DutyError},
};

/// How long to wait before redialling, and the ceiling that backoff climbs to.
///
/// Bounded because the tail matters more than the head: a relay that backs off
/// to minutes leaves its cluster unreadable for minutes after canopy comes
/// back, and canopy coming back is the common case for a connection dropping.
const REDIAL_MIN: Duration = Duration::from_secs(1);
const REDIAL_MAX: Duration = Duration::from_secs(30);

/// Filings the relay wants to send. A channel rather than direct sends so the
/// check families do not each have to hold a connection, or care that one is
/// currently down.
pub type Filings = mpsc::Sender<Filing>;

/// Run the relay until the process is stopped.
///
/// Returns only if the filing channel closes, which means nothing is producing
/// filings any more.
pub async fn run<D: Duties>(config: Config, duties: Arc<D>, mut filings: mpsc::Receiver<Filing>) {
	let mut backoff = REDIAL_MIN;

	loop {
		match connect(&config).await {
			Ok(connection) => {
				info!(
					canopy = %config.canopy_addr,
					"connected to canopy",
				);
				backoff = REDIAL_MIN;

				// Serve and file on the same connection until it ends. A
				// connection loss is normal rather than exceptional, so it is
				// logged at debug and redialled.
				let reason = serve(&connection, &config, duties.clone(), &mut filings).await;
				match reason {
					Ended::ChannelClosed => {
						info!("nothing is filing any more; the relay is done");
						connection.close(0u32.into(), b"relay shutting down");
						return;
					}
					Ended::ConnectionLost(err) => {
						debug!("connection to canopy ended: {err}");
					}
				}
			}
			Err(err) => {
				// A relay that cannot reach canopy is what the cluster
				// connectivity check is for, so this is worth a warning: it is
				// the relay's side of a condition an operator will be paged
				// about.
				warn!(
					canopy = %config.canopy_addr,
					"cannot reach canopy, retrying in {}s: {err}",
					backoff.as_secs(),
				);
			}
		}

		tokio::time::sleep(backoff).await;
		backoff = (backoff * 2).min(REDIAL_MAX);
	}
}

/// Why serving stopped.
enum Ended {
	/// The connection went. Redial.
	ConnectionLost(String),
	/// Nothing is producing filings. Stop.
	ChannelClosed,
}

async fn connect(config: &Config) -> Result<quinn::Connection, String> {
	let mut endpoint = quinn::Endpoint::client("[::]:0".parse().expect("a valid bind address"))
		.map_err(|e| format!("cannot open a socket: {e}"))?;
	endpoint.set_default_client_config(
		client_config(&config.identity, config.canopy_spki.clone())
			.map_err(|e| format!("cannot configure TLS: {e}"))?,
	);

	let connecting = endpoint
		.connect(config.canopy_addr, &config.server_name)
		.map_err(|e| format!("cannot dial: {e}"))?;

	let connection = connecting.await.map_err(|e| e.to_string())?;

	// The endpoint owns the socket, so it has to outlive the connection.
	// Handing it to the connection's lifetime here keeps that true without
	// threading it through everything below.
	tokio::spawn({
		let connection = connection.clone();
		async move {
			connection.closed().await;
			drop(endpoint);
		}
	});

	Ok(connection)
}

/// Answer canopy's requests and send filings, until one side stops.
async fn serve<D: Duties>(
	connection: &quinn::Connection,
	config: &Config,
	duties: Arc<D>,
	filings: &mut mpsc::Receiver<Filing>,
) -> Ended {
	loop {
		tokio::select! {
			// Canopy asking something.
			accepted = connection.accept_bi() => {
				match accepted {
					Ok((send, recv)) => {
						let duties = duties.clone();
						let floor = config.floor.clone();
						tokio::spawn(async move {
							if let Err(err) = answer(send, recv, duties, floor).await {
								warn!("failed to answer canopy: {err}");
							}
						});
					}
					Err(err) => return Ended::ConnectionLost(err.to_string()),
				}
			}

			// Something to file.
			filing = filings.recv() => {
				match filing {
					Some(filing) => {
						let connection = connection.clone();
						// Each filing on its own stream, so a slow one holds up
						// nothing behind it.
						tokio::spawn(async move {
							if let Err(err) = file(&connection, &filing).await {
								// Not retried here: the next refile carries the
								// same state, which is the reconciliation this
								// design leans on instead of acknowledgements.
								debug!("a filing did not get through: {err}");
							}
						});
					}
					None => return Ended::ChannelClosed,
				}
			}
		}
	}
}

/// Send one filing on its own unidirectional stream.
async fn file(connection: &quinn::Connection, filing: &Filing) -> Result<(), String> {
	let mut stream = connection
		.open_uni()
		.await
		.map_err(|e| format!("opening a filing stream: {e}"))?;
	write_frame(&mut stream, filing)
		.await
		.map_err(|e| format!("writing the filing: {e}"))?;
	stream
		.finish()
		.map_err(|e| format!("finishing the filing stream: {e}"))?;
	Ok(())
}

/// Read one request and answer it on the same stream.
async fn answer<D: Duties>(
	mut send: quinn::SendStream,
	mut recv: quinn::RecvStream,
	duties: Arc<D>,
	floor: crate::version::VersionFloor,
) -> Result<(), String> {
	let request: Request = read_required_frame(&mut recv)
		.await
		.map_err(|e| format!("reading the request: {e}"))?;

	let response = dispatch(&request, duties, &floor).await;

	write_frame(&mut send, &response)
		.await
		.map_err(|e| format!("writing the answer: {e}"))?;
	send.finish()
		.map_err(|e| format!("finishing the answer: {e}"))?;
	Ok(())
}

/// Turn a request into an answer.
///
/// Separate from the stream handling so the whole of what a relay will do can
/// be exercised without a connection.
pub async fn dispatch<D: Duties>(
	request: &Request,
	duties: Arc<D>,
	floor: &crate::version::VersionFloor,
) -> Response {
	match request {
		Request::Ping => Response::Pong,

		Request::Build => Response::Build(duties.build()),

		Request::NamespaceRoster { namespace } => match duties.roster(namespace).await {
			Ok(instances) => Response::NamespaceRoster { instances },
			Err(err) => err.into(),
		},

		Request::Sleep { namespace } => match duties.sleep(namespace).await {
			Ok(()) => Response::Asleep,
			Err(err) => err.into(),
		},

		Request::Wake { namespace } => match duties.wake(namespace).await {
			Ok(()) => Response::Awake,
			Err(err) => err.into(),
		},

		Request::RunVersion { version } => {
			// The floor is checked here, before the duty is asked to do
			// anything, so a version this relay will not run is refused
			// whatever the implementation would have done with it.
			if let Err(err) = floor.admits(version) {
				error!("canopy named a version this relay will not run: {err}");
				return Response::refuse(RefusalKind::BelowVersionFloor, err.to_string());
			}

			match duties.run_version(version).await {
				Ok(()) => Response::VersionAccepted,
				Err(err) => err.into(),
			}
		}
	}
}

impl From<DutyError> for Response {
	/// A refusal and a failure are different answers, and the difference
	/// matters to canopy: a refusal is the relay enforcing a rule canopy could
	/// not have checked, and a failure is the relay having tried.
	fn from(err: DutyError) -> Self {
		let message = err.to_string();
		match err {
			DutyError::NoScheduledExpiry { .. } => Response::Refused(Refusal {
				kind: RefusalKind::NoScheduledExpiry,
				message,
			}),
			DutyError::UnknownNamespace { .. } => Response::Refused(Refusal {
				kind: RefusalKind::UnknownNamespace,
				message,
			}),
			DutyError::Failed(_) => Response::Failed { message },
		}
	}
}
