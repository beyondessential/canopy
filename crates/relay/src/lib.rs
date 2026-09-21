//! The canopy relay: the process that runs inside a Kubernetes cluster and
//! monitors the Tamanu deployments there on canopy's behalf (spec `K8S`).
//!
//! One relay per cluster. It holds the cluster's permissions on its own
//! ServiceAccount and connects to each instance's local Postgres; it opens its
//! connection outward to canopy and accepts none inward. So canopy holds no
//! credential to the cluster, and what canopy can learn of a cluster is
//! bounded by the set of requests this answers.
//!
//! ## What is here and what is not
//!
//! This crate is the relay's *frame*: its identity, its connection, the
//! reconnect loop, and the dispatch that answers canopy's requests and files
//! upward. The checks themselves — the harvest against each instance's
//! database, and the substrate checks read from the Kubernetes API — arrive as
//! their own work, and they do not live here: both families are `alertd`'s,
//! the substrate half behind a `kube` feature, so a check's two behaviours stay
//! in one crate and cannot drift on separate release cycles. The relay embeds
//! that suite and sends what it produces up the filings channel.
//!
//! So the seam for a check is [`client::Filings`], not [`Duties`]. `Duties` is
//! the separate, smaller thing: the cluster actions canopy *asks* for, none of
//! which is a check.
//!
//! Keeping the frame separable is not tidiness. It means the transport, the
//! authentication, and the protocol can be exercised without a cluster or a
//! database anywhere near them, which is what the tests here do.

pub mod client;
pub mod config;
pub mod duties;
pub mod version;

pub use client::run;
pub use config::Config;
pub use duties::Duties;
pub use version::VersionFloor;
