//! Operator policy attached to a reporting source (as opposed to a single
//! check). Currently the reachability mode; the ingest mode follows.

use serde::{Deserialize, Serialize};

/// The source a relay's substrate checks are filed under: what the cluster
/// does with an application's workloads, at grains other than one application
/// (spec `K8S`, "Checks determined about the substrate").
///
/// Filed only by a relay, so it is reserved from the device API — no ordinary
/// device can determine these conditions, and one claiming to would be
/// reporting something it cannot know.
pub const SUBSTRATE_SOURCE: &str = "kubernetes";

/// How a source's silence bears on its servers' reachability.
///
/// Stored as text in Postgres, validated as this enum at the edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum ReachabilityMode {
	/// A stale source warns, and all of a server's sources stale is
	/// unreachable. The default.
	#[default]
	On,
	/// A stale source raises no warning, but still counts toward
	/// unreachable — so a server whose only (or last) source is quiet still
	/// reads unreachable when it goes silent.
	Quiet,
	/// The source is excluded from reachability entirely.
	Off,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("invalid reachability mode; expected one of: on, quiet, off")]
pub struct ReachabilityModeFromStringError;

impl std::str::FromStr for ReachabilityMode {
	type Err = ReachabilityModeFromStringError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_ascii_lowercase().as_str() {
			"on" => Ok(Self::On),
			"quiet" => Ok(Self::Quiet),
			"off" => Ok(Self::Off),
			_ => Err(ReachabilityModeFromStringError),
		}
	}
}

impl TryFrom<String> for ReachabilityMode {
	type Error = ReachabilityModeFromStringError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl std::fmt::Display for ReachabilityMode {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let s = match self {
			Self::On => "on",
			Self::Quiet => "quiet",
			Self::Off => "off",
		};
		write!(f, "{s}")
	}
}

impl From<ReachabilityMode> for String {
	fn from(m: ReachabilityMode) -> Self {
		m.to_string()
	}
}

/// Whether the device API ingests a source's reports.
///
/// Stored as text in Postgres, validated as this enum at the edges.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "lowercase")]
pub enum IngestMode {
	/// Reports are ingested normally. The default.
	#[default]
	Allow,
	/// Reports are accepted but the source's data is discarded before
	/// ingestion — nothing is recorded.
	Ignore,
	/// The device API rejects the push outright.
	Deny,
}

#[derive(Debug, Clone, Copy, thiserror::Error)]
#[error("invalid ingest mode; expected one of: allow, ignore, deny")]
pub struct IngestModeFromStringError;

impl std::str::FromStr for IngestMode {
	type Err = IngestModeFromStringError;

	fn from_str(s: &str) -> Result<Self, Self::Err> {
		match s.to_ascii_lowercase().as_str() {
			"allow" => Ok(Self::Allow),
			"ignore" => Ok(Self::Ignore),
			"deny" => Ok(Self::Deny),
			_ => Err(IngestModeFromStringError),
		}
	}
}

impl TryFrom<String> for IngestMode {
	type Error = IngestModeFromStringError;

	fn try_from(value: String) -> Result<Self, Self::Error> {
		value.parse()
	}
}

impl std::fmt::Display for IngestMode {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		let s = match self {
			Self::Allow => "allow",
			Self::Ignore => "ignore",
			Self::Deny => "deny",
		};
		write!(f, "{s}")
	}
}

impl From<IngestMode> for String {
	fn from(m: IngestMode) -> Self {
		m.to_string()
	}
}
