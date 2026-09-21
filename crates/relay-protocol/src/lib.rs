//! The wire contract between canopy and the relay it runs in each
//! Kubernetes cluster (spec `K8S`).
//!
//! The relay is a second deployable on its own release cycle, so a relay at
//! version N talks to a canopy at version M and the contract has to be
//! explicit from the first release. It lives in one crate depended on by both
//! ends so the two cannot drift on message shape.
//!
//! Deliberately absent: `kube`, `k8s-openapi`, and the harvest. No message
//! carries a Kubernetes object — everything crossing the connection is a filed
//! check, an answer to one named question, or an action on a deployment. This
//! crate is where that stays true.
//!
//! ## The shape of an exchange
//!
//! QUIC streams are cheap and independently delivered, so **the stream is the
//! correlation**: no request ids, no multiplexing layer, and a cancelled
//! request is a reset stream. A slow roster query cannot stall a queue of
//! filings behind it.
//!
//! - The relay opens a **unidirectional** stream per [`Filing`] and writes one
//!   frame. There is no response, and none is wanted (see [`filing`]).
//! - Canopy opens a **bidirectional** stream per [`Request`], writes one frame,
//!   and reads one [`Response`] frame back.
//!
//! So the direction of a stream says what is on it, with no marker to read and
//! no ambiguity to resolve. What canopy records of a relay's build ([`Hello`])
//! is the answer to [`Request::Build`], which canopy asks as soon as it has
//! authenticated the connection — one exchange rather than a connect-time
//! announcement *and* a query that reads the same thing.
//!
//! ## Direction of authority
//!
//! Which cluster a connection belongs to is derived from the authenticated
//! relay device, never claimed in a message, so nothing here carries a cluster
//! identity. Filings likewise address a target in the coordinates the relay
//! actually holds — a namespace and an instance within it — and canopy maps
//! those to its own server records (see [`FilingTarget`]).

pub mod alpn;
pub mod filing;
pub mod frame;
pub mod request;
pub mod transport;

pub use alpn::{ALPN_V1, ProtocolVersion};
pub use filing::{Filing, FilingTarget, HarvestFiling, Instance, SubstrateFiling};
pub use frame::{MAX_FRAME_BYTES, ProtocolError, read_frame, read_required_frame, write_frame};
pub use request::{Hello, Refusal, RefusalKind, Request, Response, RosterEntry};
pub use transport::{Identity, TransportError};

/// The reserved source a relay's substrate checks are filed under. Defined in
/// `commons-types` with the rest of the source vocabulary, re-exported here
/// because it is what a relay files under.
pub use commons_types::source::SUBSTRATE_SOURCE;
