use serde::{Deserialize, Serialize};

/// A kopia snapshot, as returned by `kopia snapshot list --json`.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Snapshot {
	pub id: String,
	#[serde(default)]
	pub description: String,
	#[serde(default)]
	pub source: SnapshotSource,
	#[serde(default)]
	pub tags: std::collections::HashMap<String, String>,
	#[serde(default)]
	pub start_time: String,
	#[serde(default)]
	pub stats: Option<SnapshotStats>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotSource {
	#[serde(default)]
	pub host: String,
	#[serde(default)]
	pub user_name: String,
	#[serde(default)]
	pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotStats {
	#[serde(default)]
	pub total_size: u64,
}

impl Snapshot {
	/// Returns the hostname from the source field.
	pub fn hostname(&self) -> &str {
		&self.source.host
	}

	/// Returns the total size in bytes, or 0 if unknown.
	pub fn total_size_bytes(&self) -> u64 {
		self.stats.as_ref().map(|s| s.total_size).unwrap_or(0)
	}
}

/// Parse the output of a snapshot-list job, which may contain non-JSON lines
/// (e.g. "Connected to repository.") before the JSON array.
///
/// Finds the first `[` and last `]` and parses the substring between them.
pub fn parse_snapshot_list_output(raw: &str) -> serde_json::Result<Vec<Snapshot>> {
	let start = raw.find('[');
	let end = raw.rfind(']');
	match (start, end) {
		(Some(s), Some(e)) if s <= e => serde_json::from_str(&raw[s..=e]),
		_ => serde_json::from_str("[]"),
	}
}

/// What a kopia Job needs to reach the repository: the Secret the Canopy
/// syncer materialises beside the replica, and the group and backup type the
/// proxy sidecar asks Canopy for credentials against. Kopia itself sees dummy
/// keys pointing at a loopback endpoint; the sidecar holds the real
/// credentials and refreshes them. Derived from the parent
/// `PostgresPhysicalReplica`'s spec.
#[derive(Debug, Clone)]
pub struct KopiaSource {
	/// Name of the namespace-local Secret carrying kopia's env vars (dummy
	/// AWS keys, canopy-provided bucket/region/prefix and the repo password),
	/// materialised by the canopy syncer as `<replica-name>-canopy-creds`.
	pub secret_name: String,
	/// Canopy group id, passed to the sidecar as `PGRO_GROUP`.
	pub group: String,
	/// Canopy backup type, passed to the sidecar as `PGRO_TYPE`.
	pub backup_type: String,
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn parse_snapshot_list_output_clean_json() {
		let raw = r#"[{"id":"snap1","startTime":"2024-01-01T00:00:00Z"}]"#;
		let snaps = parse_snapshot_list_output(raw).unwrap();
		assert_eq!(snaps.len(), 1);
		assert_eq!(snaps[0].id, "snap1");
	}

	#[test]
	fn parse_snapshot_list_output_with_preamble() {
		let raw =
			"Connected to repository.\n[{\"id\":\"snap1\",\"startTime\":\"2024-01-01T00:00:00Z\"}]";
		let snaps = parse_snapshot_list_output(raw).unwrap();
		assert_eq!(snaps.len(), 1);
		assert_eq!(snaps[0].id, "snap1");
	}

	#[test]
	fn parse_snapshot_list_output_with_multiline_preamble() {
		let raw = "Connected to repository.\nSome other info\n[{\"id\":\"snap1\",\"startTime\":\"2024-01-01T00:00:00Z\"}]\n";
		let snaps = parse_snapshot_list_output(raw).unwrap();
		assert_eq!(snaps.len(), 1);
		assert_eq!(snaps[0].id, "snap1");
	}

	#[test]
	fn parse_snapshot_list_output_empty_array() {
		let raw = "Connected to repository.\n[]";
		let snaps = parse_snapshot_list_output(raw).unwrap();
		assert!(snaps.is_empty());
	}

	#[test]
	fn parse_snapshot_list_output_no_json() {
		let raw = "Connected to repository.\nNo snapshots found.";
		let snaps = parse_snapshot_list_output(raw).unwrap();
		assert!(snaps.is_empty());
	}

	#[test]
	fn snapshot_total_size_bytes_with_stats() {
		let snap = Snapshot {
			id: "s1".into(),
			stats: Some(SnapshotStats { total_size: 1024 }),
			..Default::default()
		};
		assert_eq!(snap.total_size_bytes(), 1024);
	}

	#[test]
	fn snapshot_total_size_bytes_without_stats() {
		let snap = Snapshot {
			id: "s1".into(),
			stats: None,
			..Default::default()
		};
		assert_eq!(snap.total_size_bytes(), 0);
	}
}
