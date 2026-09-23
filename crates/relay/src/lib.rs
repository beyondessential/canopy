//! The canopy relay: the process that runs inside a Kubernetes cluster and
//! monitors the Tamanu applications there on canopy's behalf (spec `K8S`).
//!
//! One relay per cluster. It holds the cluster's permissions on its own
//! ServiceAccount and connects to each instance's local Postgres; it opens its
//! connection outward to canopy and accepts none inward. So canopy holds no
//! credential to the cluster, and what canopy can learn of a cluster is
//! bounded by the set of requests this answers.
//!
//! ## What is here and what is not
//!
//! This crate is the relay's frame: its identity, its connection, the
//! reconnect loop, and the dispatch that answers canopy's requests and files
//! upward. It also carries the checks about the cluster itself ([`cluster`]),
//! which exist only in a cluster and have no counterpart elsewhere.
//!
//! The checks about each application do not live here. They are `alertd`'s,
//! for boxes and clusters alike, so a check's two behaviours stay in one crate
//! and cannot drift on separate release cycles; the relay embeds that suite and
//! sends what it produces up the filings channel.
//!
//! So the seam for a check is [`client::Filings`], not [`Duties`]. `Duties` is
//! the separate, smaller thing: the cluster actions canopy *asks* for, none of
//! which is a check.
//!
//! Keeping the frame separable is not tidiness. It means the transport, the
//! authentication, and the protocol can be exercised without a cluster or a
//! database anywhere near them, which is what the tests here do.

pub mod client;
pub mod cluster;
pub mod config;
pub mod duties;
pub mod version;

pub use client::run;
pub use config::Config;
pub use duties::Duties;
pub use version::VersionFloor;
