use base64::Engine;
use commons_errors::{AppError, Result};
use commons_types::device::DeviceRole;
use diesel::QueryableByName;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use uuid::Uuid;

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::devices)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Device {
	/// The ID of the device.
	pub id: Uuid,

	/// The created timestamp.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,

	/// The updated timestamp.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,

	/// The role of the device.
	///
	/// This is used for permission checks.
	#[diesel(deserialize_as = String, serialize_as = String)]
	pub role: DeviceRole,

	/// Stable Tailscale node ID (e.g. `nodekey:abc...`). Populated for
	/// devices that authenticate over the tailnet; null for mTLS-only.
	pub tailscale_node_id: Option<String>,

	/// Tailscale-side hostname (e.g. `device-01.tailnet.ts.net`). Mirrors
	/// the control plane for display; not load-bearing for auth.
	pub tailscale_node_name: Option<String>,

	/// Tailnet the node belongs to. Same caveat as above.
	pub tailscale_tailnet: Option<String>,
}

/// Captures the stable Tailscale identity associated with an incoming
/// request, used to attach a device row to a tailnet node either on
/// auto-discovery or via an admin pre-attach.
#[derive(Clone, Debug)]
pub struct TailscaleIdentity {
	pub node_id: String,
	pub node_name: Option<String>,
	pub tailnet: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Insertable)]
#[diesel(table_name = crate::schema::device_keys)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct DeviceKey {
	/// The ID of the device key.
	pub id: Uuid,

	/// The created timestamp.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,

	/// The updated timestamp.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,

	/// The device this key belongs to.
	pub device_id: Uuid,

	/// The public key data in PublicKeyInfo form.
	///
	/// This is the RFC 5280, Section 4.1.2.7 form of the public key as contained by X.509
	/// certificates or by RFC 7250 Raw Public Keys.
	///
	/// This contains both the public key and its algorithm, and is extensible to support all types
	/// of keys that TLS or X.509 in general can support.
	pub key_data: Vec<u8>,

	/// Optional name/description for the key.
	pub name: Option<String>,

	/// Whether this key is active and can be used for authentication.
	pub is_active: bool,

	/// When this key last authenticated over mutual TLS, to within
	/// [`AUTHENTICATED_STAMP_EVERY`]. Authentication over the tailnet carries
	/// no key, and leaves it.
	// spec: TSI#encryption
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub last_authenticated_at: Option<Timestamp>,
}

/// How stale a key's [`DeviceKey::last_authenticated_at`] may get before an
/// authentication with it stamps it again, so the device API does not write on
/// every request.
pub const AUTHENTICATED_STAMP_EVERY: std::time::Duration = std::time::Duration::from_secs(60);

/// Device with its keys and latest connection info for management purposes.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DeviceWithInfo {
	pub device: Device,
	pub keys: Vec<DeviceKey>,
	pub latest_connection: Option<DeviceConnection>,
}

impl Device {
	pub async fn from_key(db: &mut AsyncPgConnection, key: &[u8]) -> Result<Option<Self>> {
		use crate::schema::{device_keys, devices};

		devices::table
			.inner_join(device_keys::table.on(device_keys::device_id.eq(devices::id)))
			.select(Self::as_select())
			.filter(device_keys::key_data.eq(key))
			.filter(device_keys::is_active.eq(true))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Create a device with an initial key. The device is a `Application`: this is
	/// the enrollment path, which only ever mints server devices (other roles
	/// are provisioned through [`create_at_role`]).
	pub async fn create(db: &mut AsyncPgConnection, key: Vec<u8>) -> Result<Self> {
		use crate::schema::devices;

		// Create the device first
		let device: Self = diesel::insert_into(devices::table)
			.values(devices::role.eq(DeviceRole::Machine))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;

		// Create the initial key for the device
		DeviceKey::create(db, device.id, key, Some("Initial Key".to_string())).await?;

		Ok(device)
	}

	/// Create a device already at `role`, with one active key named `key_name`.
	/// Used by operator-provisioned credentials (spec DPK), where Canopy mints
	/// the keypair server-side and names its key rather than using "Initial Key".
	pub async fn create_at_role(
		db: &mut AsyncPgConnection,
		key: Vec<u8>,
		role: DeviceRole,
		key_name: Option<String>,
	) -> Result<Self> {
		use crate::schema::devices;

		let device: Self = diesel::insert_into(devices::table)
			.values(devices::role.eq(role))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)?;

		DeviceKey::create(db, device.id, key, key_name).await?;

		Ok(device)
	}

	pub async fn from_tailscale_node_id(
		db: &mut AsyncPgConnection,
		node_id: &str,
	) -> Result<Option<Self>> {
		use crate::schema::devices;

		devices::table
			.select(Self::as_select())
			.filter(devices::tailscale_node_id.eq(node_id))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Insert a tailnet device at `role`, identified by its Tailscale identity
	/// with no mTLS key yet (`device_keys` left empty). Used by the operator
	/// attach/create flows, which trust the tailnet identity as a `Application`.
	pub async fn create_with_tailscale(
		db: &mut AsyncPgConnection,
		identity: TailscaleIdentity,
		role: DeviceRole,
	) -> Result<Self> {
		use crate::schema::devices;

		diesel::insert_into(devices::table)
			.values((
				devices::role.eq(role),
				devices::tailscale_node_id.eq(identity.node_id),
				devices::tailscale_node_name.eq(identity.node_name),
				devices::tailscale_tailnet.eq(identity.tailnet),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Pre-attach a tailnet identity to an existing device. Used by the
	/// admin "attach Tailscale identity" workflow when the operator
	/// knows a device is about to come online over the tailnet (or is
	/// being moved off mTLS).
	///
	/// If another device already holds this `node_id`, this returns
	/// `DeviceTailscaleNodeAlreadyClaimed` and the operator must reach for the
	/// merge flow.
	pub async fn attach_tailscale(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
		identity: TailscaleIdentity,
	) -> Result<()> {
		use crate::schema::devices::dsl;

		db.transaction::<_, AppError, _>(async |conn| {
			let conflict: Option<Self> = dsl::devices
				.select(Self::as_select())
				.filter(dsl::tailscale_node_id.eq(&identity.node_id))
				.filter(dsl::id.ne(device_id))
				.first(conn)
				.await
				.optional()
				.map_err(AppError::from)?;
			if conflict.is_some() {
				return Err(AppError::DeviceTailscaleNodeAlreadyClaimed);
			}

			diesel::update(dsl::devices.filter(dsl::id.eq(device_id)))
				.set((
					dsl::tailscale_node_id.eq(identity.node_id),
					dsl::tailscale_node_name.eq(identity.node_name),
					dsl::tailscale_tailnet.eq(identity.tailnet),
				))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			Ok(())
		})
		.await
	}

	/// Clear the tailnet identity from a device. The opposite of
	/// `attach_tailscale`; leaves the device row otherwise untouched
	/// (keys, role, server attachment all stay).
	pub async fn detach_tailscale(db: &mut AsyncPgConnection, device_id: Uuid) -> Result<()> {
		use crate::schema::devices::dsl;

		diesel::update(dsl::devices.filter(dsl::id.eq(device_id)))
			.set((
				dsl::tailscale_node_id.eq(None::<String>),
				dsl::tailscale_node_name.eq(None::<String>),
				dsl::tailscale_tailnet.eq(None::<String>),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Merge `source_id` into `target_id`: every foreign-key reference
	/// to `source_id` is re-parented to `target_id`, then `source_id`
	/// is deleted. Runs in a single transaction.
	///
	/// Conflict cases (returns [`AppError::DeviceMergeConflict`]):
	///
	/// - Both source and target hold a tailscale identity. The
	///   operator must `detach_tailscale` on one side first.
	/// - Both source and target are bound to a machine
	///   (`machines.device_id`). The operator must clear one
	///   `machines.device_id` first; otherwise the unique constraint
	///   would fail during the rewrite.
	///
	/// Target wins for `role` and for `tailscale_*` (if it already
	/// has them). If only the source has a tailscale identity, the
	/// merge adopts it onto the target.
	pub async fn merge_into(
		db: &mut AsyncPgConnection,
		source_id: Uuid,
		target_id: Uuid,
	) -> Result<()> {
		use crate::schema::{
			artifacts, device_connections, device_keys, devices, issues, machines, statuses,
			versions,
		};

		if source_id == target_id {
			return Err(AppError::custom(
				"merge_into: source and target are the same",
			));
		}

		db.transaction::<_, AppError, _>(async |conn| {
			let source: Self = devices::table
				.select(Self::as_select())
				.filter(devices::id.eq(source_id))
				.first(conn)
				.await
				.map_err(AppError::from)?;
			let target: Self = devices::table
				.select(Self::as_select())
				.filter(devices::id.eq(target_id))
				.first(conn)
				.await
				.map_err(AppError::from)?;

			// Conflict: both have tailscale identity.
			if source.tailscale_node_id.is_some() && target.tailscale_node_id.is_some() {
				return Err(AppError::DeviceMergeConflict);
			}

			// Conflict: both bound to a (possibly different) machine.
			let source_machine_count: i64 = machines::table
				.filter(machines::device_id.eq(source_id))
				.count()
				.get_result(conn)
				.await
				.map_err(AppError::from)?;
			let target_machine_count: i64 = machines::table
				.filter(machines::device_id.eq(target_id))
				.count()
				.get_result(conn)
				.await
				.map_err(AppError::from)?;
			if source_machine_count > 0 && target_machine_count > 0 {
				return Err(AppError::DeviceMergeConflict);
			}

			// Adopt source's tailscale identity onto target if target
			// has none. (We don't need to clear source first; deleting
			// source at the end is sufficient. But the partial unique
			// index on tailscale_node_id would fire if both rows held
			// the same value briefly — we already ruled that out via
			// the conflict check above.)
			if target.tailscale_node_id.is_none() && source.tailscale_node_id.is_some() {
				// Clear source's identity *first* so we don't transiently
				// hold the same `tailscale_node_id` on two rows (the
				// `devices_tailscale_node_id_key` unique constraint would
				// fire on the target update otherwise).
				diesel::update(devices::table.filter(devices::id.eq(source_id)))
					.set((
						devices::tailscale_node_id.eq(None::<String>),
						devices::tailscale_node_name.eq(None::<String>),
						devices::tailscale_tailnet.eq(None::<String>),
					))
					.execute(conn)
					.await
					.map_err(AppError::from)?;
				diesel::update(devices::table.filter(devices::id.eq(target_id)))
					.set((
						devices::tailscale_node_id.eq(source.tailscale_node_id.as_deref()),
						devices::tailscale_node_name.eq(source.tailscale_node_name.as_deref()),
						devices::tailscale_tailnet.eq(source.tailscale_tailnet.as_deref()),
					))
					.execute(conn)
					.await
					.map_err(AppError::from)?;
			}

			// Re-parent simple foreign keys.
			diesel::update(device_keys::table.filter(device_keys::device_id.eq(source_id)))
				.set(device_keys::device_id.eq(target_id))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			diesel::update(
				device_connections::table.filter(device_connections::device_id.eq(source_id)),
			)
			.set(device_connections::device_id.eq(target_id))
			.execute(conn)
			.await
			.map_err(AppError::from)?;
			diesel::update(artifacts::table.filter(artifacts::device_id.eq(source_id)))
				.set(artifacts::device_id.eq(target_id))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			diesel::update(issues::table.filter(issues::device_id.eq(source_id)))
				.set(issues::device_id.eq(target_id))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			diesel::update(statuses::table.filter(statuses::device_id.eq(source_id)))
				.set(statuses::device_id.eq(target_id))
				.execute(conn)
				.await
				.map_err(AppError::from)?;
			diesel::update(versions::table.filter(versions::device_id.eq(source_id)))
				.set(versions::device_id.eq(target_id))
				.execute(conn)
				.await
				.map_err(AppError::from)?;

			// machines.device_id is UNIQUE. We already ruled out the
			// both-bound case above; here the only writers are source-bound
			// (rewrite to target) or neither (no-op).
			diesel::update(machines::table.filter(machines::device_id.eq(source_id)))
				.set(machines::device_id.eq(target_id))
				.execute(conn)
				.await
				.map_err(AppError::from)?;

			// Finally, delete the source device row.
			diesel::delete(devices::table.filter(devices::id.eq(source_id)))
				.execute(conn)
				.await
				.map_err(AppError::from)?;

			Ok(())
		})
		.await
	}

	/// Get a single device by ID with its keys and latest connection info.
	pub async fn get_with_info(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
	) -> Result<DeviceWithInfo> {
		use crate::schema::{device_keys, devices};

		let device: Self = devices::table
			.select(Self::as_select())
			.filter(devices::id.eq(device_id))
			.first(db)
			.await
			.map_err(AppError::from)?;

		// All keys, active and inactive — the detail view shows disabled keys
		// (greyed, re-enableable), active first then oldest-first.
		let keys: Vec<DeviceKey> = device_keys::table
			.select(DeviceKey::as_select())
			.filter(device_keys::device_id.eq(device_id))
			.order((device_keys::is_active.desc(), device_keys::created_at.asc()))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let latest_connections =
			DeviceConnection::get_latest_from_device_ids(db, std::iter::once(device_id)).await?;

		let latest_connection = latest_connections.into_iter().next();

		Ok(DeviceWithInfo {
			device,
			keys,
			latest_connection,
		})
	}

	/// List all devices with their keys and latest connection info.
	pub async fn list_trusted_with_info(db: &mut AsyncPgConnection) -> Result<Vec<DeviceWithInfo>> {
		Self::list_trusted_with_info_paginated(db, i64::MAX, 0).await
	}

	/// All devices holding a given role, newest first. Used to list restore
	/// consumers (`backup-restore` devices) for the operator's replica forms.
	pub async fn list_by_role(db: &mut AsyncPgConnection, role: DeviceRole) -> Result<Vec<Self>> {
		use crate::schema::devices;
		devices::table
			.select(Self::as_select())
			.filter(devices::role.eq(role))
			.order(devices::created_at.desc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// List devices with pagination.
	pub async fn list_trusted_with_info_paginated(
		db: &mut AsyncPgConnection,
		limit: i64,
		offset: i64,
	) -> Result<Vec<DeviceWithInfo>> {
		use crate::schema::{device_keys, devices};

		let trusted_devices: Vec<Self> = devices::table
			.select(Self::as_select())
			.order(devices::created_at.desc())
			.limit(limit)
			.offset(offset)
			.load(db)
			.await
			.map_err(AppError::from)?;

		let device_ids: Vec<Uuid> = trusted_devices.iter().map(|d| d.id).collect();

		let device_keys: Vec<DeviceKey> = device_keys::table
			.select(DeviceKey::as_select())
			.filter(device_keys::device_id.eq_any(&device_ids))
			.filter(device_keys::is_active.eq(true))
			.order(device_keys::created_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)?;

		let latest_connections =
			DeviceConnection::get_latest_from_device_ids(db, device_ids.iter().copied()).await?;

		let mut keys_by_device: HashMap<Uuid, Vec<DeviceKey>> = HashMap::new();
		for key in device_keys {
			keys_by_device.entry(key.device_id).or_default().push(key);
		}

		let mut connections_by_device: HashMap<Uuid, DeviceConnection> = HashMap::new();
		for connection in latest_connections {
			connections_by_device.insert(connection.device_id, connection);
		}

		let result = trusted_devices
			.into_iter()
			.map(|device| DeviceWithInfo {
				keys: keys_by_device.remove(&device.id).unwrap_or_default(),
				latest_connection: connections_by_device.remove(&device.id),
				device,
			})
			.collect();

		Ok(result)
	}

	/// Count all devices.
	pub async fn count_trusted(db: &mut AsyncPgConnection) -> Result<i64> {
		use crate::schema::devices;
		use diesel::dsl::count_star;

		devices::table
			.select(count_star())
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// Trust a device by setting its role.
	pub async fn trust(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
		new_role: DeviceRole,
	) -> Result<()> {
		use crate::schema::devices::dsl;

		diesel::update(dsl::devices.filter(dsl::id.eq(device_id)))
			.set(dsl::role.eq(new_role))
			.execute(db)
			.await
			.map_err(AppError::from)?;

		Ok(())
	}

	/// Revoke a device's access: deactivate its keys and detach any tailnet
	/// identity, so it can no longer authenticate by either path. The row is
	/// kept (with its role) for history; re-granting access means provisioning
	/// a fresh credential or re-enrolling.
	pub async fn revoke(db: &mut AsyncPgConnection, device_id: Uuid) -> Result<()> {
		db.transaction::<_, AppError, _>(async |conn| {
			Self::deactivate_keys(conn, device_id).await?;
			Self::detach_tailscale(conn, device_id).await?;
			Ok(())
		})
		.await
	}

	/// Like [`from_key`] but matches inactive keys too. Used by enrollment to
	/// rebind a returning box to its existing device row (whose key was
	/// deactivated on the prior server's archival) rather than spawning a
	/// duplicate identity.
	pub async fn from_key_any_state(
		db: &mut AsyncPgConnection,
		key: &[u8],
	) -> Result<Option<Self>> {
		use crate::schema::{device_keys, devices};

		devices::table
			.inner_join(device_keys::table.on(device_keys::device_id.eq(devices::id)))
			.select(Self::as_select())
			.filter(device_keys::key_data.eq(key))
			.first(db)
			.await
			.optional()
			.map_err(AppError::from)
	}

	/// Add (or re-activate) an mTLS key on a device that has no *other* active
	/// key. Idempotent if the key is already active on this device. Refuses if
	/// the device already carries a different active key — adding a second
	/// credential to an already-keyed (e.g. already-enrolled) device is an
	/// operator action, not something an enrolling box may do.
	pub async fn add_key(db: &mut AsyncPgConnection, device_id: Uuid, key: Vec<u8>) -> Result<()> {
		use crate::schema::device_keys::dsl;

		let active: Vec<DeviceKey> = DeviceKey::find_by_device(db, device_id).await?;
		if active.iter().any(|k| k.key_data == key) {
			return Ok(());
		}
		if !active.is_empty() {
			return Err(AppError::Conflict(format!(
				"device {device_id} already has an active key; refusing to add another during enrollment",
			)));
		}

		// Re-activate a previously-deactivated copy of this exact key if present
		// (returning box), else insert a fresh row.
		let reactivated = diesel::update(
			dsl::device_keys
				.filter(dsl::device_id.eq(device_id))
				.filter(dsl::key_data.eq(&key)),
		)
		.set(dsl::is_active.eq(true))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		if reactivated == 0 {
			DeviceKey::create(db, device_id, key, Some("Enrollment key".to_string())).await?;
		}
		Ok(())
	}

	/// Bulk-fetch the Tailscale hostname (`tailscale_node_name`) for each given
	/// device that has one. Used to derive a display URL for applications that have
	/// no stored URL but are bound to a tailnet node.
	pub async fn tailscale_names_by_ids(
		db: &mut AsyncPgConnection,
		ids: &[Uuid],
	) -> Result<HashMap<Uuid, String>> {
		use crate::schema::devices::dsl;
		if ids.is_empty() {
			return Ok(HashMap::new());
		}
		let rows: Vec<(Uuid, Option<String>)> = dsl::devices
			.select((dsl::id, dsl::tailscale_node_name))
			.filter(dsl::id.eq_any(ids))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows
			.into_iter()
			.filter_map(|(id, name)| name.map(|n| (id, n)))
			.collect())
	}

	/// Deactivate all of a device's keys. Used on server archival so a
	/// decommissioned box's key cannot silently re-authenticate; re-enrollment
	/// must go through the gated token + proof-of-possession flow.
	pub async fn deactivate_keys(db: &mut AsyncPgConnection, device_id: Uuid) -> Result<()> {
		use crate::schema::device_keys::dsl;
		diesel::update(dsl::device_keys.filter(dsl::device_id.eq(device_id)))
			.set(dsl::is_active.eq(false))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Search devices by key data (supports partial matches).
	pub async fn search_by_key(
		db: &mut AsyncPgConnection,
		query: &str,
	) -> Result<Vec<DeviceWithInfo>> {
		use crate::schema::{device_keys, devices};
		use diesel::sql_query;
		use diesel::sql_types::{Binary, Bool, Uuid as SqlUuid};

		// Try different search strategies
		let mut matching_device_ids: Vec<Uuid> = Vec::new();

		#[derive(QueryableByName)]
		struct MatchingDevice {
			#[diesel(sql_type = SqlUuid)]
			device_id: Uuid,
		}

		// Strategy 1: Try hex decode and binary search
		if let Ok(hex_bytes) = hex::decode(query.replace([' ', ':'], "")) {
			let hex_matches: Vec<Uuid> = sql_query(
				"SELECT DISTINCT device_id FROM device_keys
				 WHERE is_active = $1 AND position($2 in key_data) > 0",
			)
			.bind::<Bool, _>(true)
			.bind::<Binary, _>(&hex_bytes)
			.load::<MatchingDevice>(db)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.map(|m| m.device_id)
			.collect();
			matching_device_ids.extend(hex_matches);
		}

		// Strategy 2: For PEM format, extract base64 and decode
		// Handle both newline-separated and space-separated PEM (from text input fields)
		if query.contains("-----BEGIN") && query.contains("-----END") {
			// Extract everything between BEGIN and END markers
			let begin_marker = "-----BEGIN";
			let end_marker = "-----END";

			if let Some(begin_pos) = query.find(begin_marker)
				&& let Some(end_pos) = query.find(end_marker)
			{
				// Get the content between the markers
				// Find the end of the BEGIN header line
				let begin_header_end = if let Some(newline_pos) = query[begin_pos..].find('\n') {
					begin_pos + newline_pos + 1
				} else {
					// For space-separated PEM, find the end of the header
					// Look for "-----" after the key type (e.g., "PUBLIC KEY-----")
					let after_begin = begin_pos + begin_marker.len(); // Skip "-----BEGIN"
					if let Some(end_marker_pos) = query[after_begin..].find("-----") {
						let header_end = after_begin + end_marker_pos + 5; // +5 for "-----"
						// Find the first space after the complete header
						if let Some(space_pos) = query[header_end..].find(' ') {
							header_end + space_pos + 1
						} else {
							header_end
						}
					} else {
						begin_pos
					}
				};
				let content_start = begin_header_end;

				let pem_content = &query[content_start..end_pos];

				// Check if this is malformed PEM with indented base64 lines
				// Only reject multi-line PEM with indented content, not space-separated single-line PEM
				let lines: Vec<&str> = pem_content.lines().collect();
				let has_indented_lines = lines.len() > 1
					&& lines.iter().any(|line| {
						let trimmed = line.trim();
						!trimmed.is_empty()
							&& !trimmed.starts_with("-----")
							&& line.starts_with([' ', '\t'])
					});

				if !has_indented_lines {
					// Remove any remaining header/footer fragments and whitespace
					let base64_part = pem_content
						.split_whitespace()
						.filter(|part| !part.starts_with("-----") && !part.is_empty())
						.collect::<Vec<_>>()
						.join("");

					if let Ok(decoded) = base64::prelude::BASE64_STANDARD.decode(base64_part) {
						let pem_matches: Vec<Uuid> = sql_query(
							"SELECT DISTINCT device_id FROM device_keys
								 WHERE is_active = $1 AND position($2 in key_data) > 0",
						)
						.bind::<Bool, _>(true)
						.bind::<Binary, _>(&decoded)
						.load::<MatchingDevice>(db)
						.await
						.map_err(AppError::from)?
						.into_iter()
						.map(|m| m.device_id)
						.collect();
						matching_device_ids.extend(pem_matches);
					}
				}
			}
		}

		// Strategy 3: Base64 string search by encoding PostgreSQL binary data
		// PostgreSQL's encode() adds line breaks every 76 chars, so we need to remove them
		if query.len() > 3
			&& query
				.chars()
				.all(|c| c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=')
		{
			let base64_matches: Vec<Uuid> = sql_query(
				"SELECT DISTINCT device_id FROM device_keys
				 WHERE is_active = $1 AND replace(encode(key_data, 'base64'), E'\\n', '') LIKE '%' || $2 || '%'",
			)
			.bind::<Bool, _>(true)
			.bind::<diesel::sql_types::Text, _>(query)
			.load::<MatchingDevice>(db)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.map(|m| m.device_id)
			.collect();
			matching_device_ids.extend(base64_matches);
		}

		// Strategy 4: Raw byte search as fallback
		if matching_device_ids.is_empty() {
			let raw_matches: Vec<Uuid> = sql_query(
				"SELECT DISTINCT device_id FROM device_keys
				 WHERE is_active = $1 AND position($2 in key_data) > 0",
			)
			.bind::<Bool, _>(true)
			.bind::<Binary, _>(query.as_bytes())
			.load::<MatchingDevice>(db)
			.await
			.map_err(AppError::from)?
			.into_iter()
			.map(|m| m.device_id)
			.collect();
			matching_device_ids.extend(raw_matches);
		}

		// Remove duplicates
		matching_device_ids.sort();
		matching_device_ids.dedup();

		if matching_device_ids.is_empty() {
			return Ok(vec![]);
		}

		// Get the matching devices
		let matching_devices: Vec<Self> = devices::table
			.select(Self::as_select())
			.filter(devices::id.eq_any(&matching_device_ids))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let device_ids: Vec<Uuid> = matching_devices.iter().map(|d| d.id).collect();

		// Get all keys for matching devices
		let matching_keys: Vec<DeviceKey> = device_keys::table
			.select(DeviceKey::as_select())
			.filter(device_keys::device_id.eq_any(&device_ids))
			.filter(device_keys::is_active.eq(true))
			.load(db)
			.await
			.map_err(AppError::from)?;

		// Get latest connections
		let latest_connections =
			DeviceConnection::get_latest_from_device_ids(db, device_ids.iter().copied()).await?;

		// Group data
		let mut keys_by_device: HashMap<Uuid, Vec<DeviceKey>> = HashMap::new();
		for key in matching_keys {
			keys_by_device.entry(key.device_id).or_default().push(key);
		}

		let mut connections_by_device: HashMap<Uuid, DeviceConnection> = HashMap::new();
		for connection in latest_connections {
			connections_by_device.insert(connection.device_id, connection);
		}

		let result = matching_devices
			.into_iter()
			.map(|device| DeviceWithInfo {
				keys: keys_by_device.remove(&device.id).unwrap_or_default(),
				latest_connection: connections_by_device.remove(&device.id),
				device,
			})
			.collect();

		Ok(result)
	}

	/// Search devices by key name.
	pub async fn search_by_key_name(
		db: &mut AsyncPgConnection,
		query: &str,
	) -> Result<Vec<DeviceWithInfo>> {
		use crate::schema::{device_keys, devices};

		let device_ids: Vec<Uuid> = device_keys::table
			.select(device_keys::device_id)
			.filter(
				device_keys::name
					.is_not_null()
					.and(device_keys::name.ilike(format!("%{}%", query))),
			)
			.distinct()
			.load::<Uuid>(db)
			.await?;

		if device_ids.is_empty() {
			return Ok(Vec::new());
		}

		// Get devices, keys, and connections
		let devices_result: Vec<Device> = devices::table
			.select(Device::as_select())
			.filter(devices::id.eq_any(&device_ids))
			.load(db)
			.await?;

		let all_keys: Vec<DeviceKey> = device_keys::table
			.select(DeviceKey::as_select())
			.filter(device_keys::device_id.eq_any(&device_ids))
			.filter(device_keys::is_active.eq(true))
			.load(db)
			.await?;

		let all_connections =
			DeviceConnection::get_latest_from_device_ids(db, device_ids.iter().copied()).await?;

		use std::collections::HashMap;

		let mut keys_by_device: HashMap<Uuid, Vec<DeviceKey>> = HashMap::new();
		for key in all_keys {
			keys_by_device.entry(key.device_id).or_default().push(key);
		}

		let mut connections_by_device: HashMap<Uuid, DeviceConnection> = HashMap::new();
		for conn in all_connections {
			connections_by_device.insert(conn.device_id, conn);
		}

		let result = devices_result
			.into_iter()
			.map(|device| DeviceWithInfo {
				keys: keys_by_device.remove(&device.id).unwrap_or_default(),
				latest_connection: connections_by_device.remove(&device.id),
				device,
			})
			.collect();

		Ok(result)
	}

	/// Search devices by connection IP.
	pub async fn search_by_connection_ip(
		db: &mut AsyncPgConnection,
		query: &str,
	) -> Result<Vec<DeviceWithInfo>> {
		use crate::schema::{device_keys, devices};
		use diesel::sql_query;
		use diesel::sql_types::Uuid as SqlUuid;

		#[derive(QueryableByName)]
		struct DeviceIdResult {
			#[diesel(sql_type = SqlUuid)]
			device_id: Uuid,
		}

		// Bounded by created_at so partition pruning engages; without this the
		// LIKE is applied to every weekly partition's full contents. Even with
		// the bound this remains a seq scan within the recent partitions —
		// LIKE on ip::text can't use any index — but the search space shrinks
		// from "all history" to "last 90 days".
		let device_ids: Vec<Uuid> = sql_query(
			"SELECT DISTINCT device_id FROM device_connections \
			 WHERE created_at >= NOW() - INTERVAL '90 days' AND ip::text LIKE $1",
		)
		.bind::<diesel::sql_types::Text, _>(format!("%{}%", query))
		.load::<DeviceIdResult>(db)
		.await?
		.into_iter()
		.map(|r| r.device_id)
		.collect();

		if device_ids.is_empty() {
			return Ok(Vec::new());
		}

		// Get devices, keys, and connections
		let devices_result: Vec<Device> = devices::table
			.select(Device::as_select())
			.filter(devices::id.eq_any(&device_ids))
			.load(db)
			.await?;

		let all_keys: Vec<DeviceKey> = device_keys::table
			.select(DeviceKey::as_select())
			.filter(device_keys::device_id.eq_any(&device_ids))
			.filter(device_keys::is_active.eq(true))
			.load(db)
			.await?;

		let all_connections =
			DeviceConnection::get_latest_from_device_ids(db, device_ids.iter().copied()).await?;

		use std::collections::HashMap;

		let mut keys_by_device: HashMap<Uuid, Vec<DeviceKey>> = HashMap::new();
		for key in all_keys {
			keys_by_device.entry(key.device_id).or_default().push(key);
		}

		let mut connections_by_device: HashMap<Uuid, DeviceConnection> = HashMap::new();
		for conn in all_connections {
			connections_by_device.insert(conn.device_id, conn);
		}

		let result = devices_result
			.into_iter()
			.map(|device| DeviceWithInfo {
				keys: keys_by_device.remove(&device.id).unwrap_or_default(),
				latest_connection: connections_by_device.remove(&device.id),
				device,
			})
			.collect();

		Ok(result)
	}

	/// Search devices by stored Tailscale identifiers: `tailscale_node_id`
	/// (substring match) or `tailscale_node_name` (case-insensitive
	/// substring match). The directory's IP-and-name resolution is
	/// handled at the endpoint layer — this method only consults the
	/// DB columns.
	pub async fn search_by_tailscale_fields(
		db: &mut AsyncPgConnection,
		query: &str,
	) -> Result<Vec<DeviceWithInfo>> {
		use crate::schema::{device_keys, devices};

		let needle = format!("%{query}%");
		let device_ids: Vec<Uuid> = devices::table
			.select(devices::id)
			.filter(
				devices::tailscale_node_id
					.ilike(needle.clone())
					.or(devices::tailscale_node_name.ilike(needle.clone()))
					.or(devices::tailscale_tailnet.ilike(needle)),
			)
			.load::<Uuid>(db)
			.await?;

		if device_ids.is_empty() {
			return Ok(Vec::new());
		}

		let devices_result: Vec<Self> = devices::table
			.select(Self::as_select())
			.filter(devices::id.eq_any(&device_ids))
			.load(db)
			.await?;

		let all_keys: Vec<DeviceKey> = device_keys::table
			.select(DeviceKey::as_select())
			.filter(device_keys::device_id.eq_any(&device_ids))
			.filter(device_keys::is_active.eq(true))
			.load(db)
			.await?;

		let all_connections =
			DeviceConnection::get_latest_from_device_ids(db, device_ids.iter().copied()).await?;

		let mut keys_by_device: HashMap<Uuid, Vec<DeviceKey>> = HashMap::new();
		for key in all_keys {
			keys_by_device.entry(key.device_id).or_default().push(key);
		}
		let mut connections_by_device: HashMap<Uuid, DeviceConnection> = HashMap::new();
		for conn in all_connections {
			connections_by_device.insert(conn.device_id, conn);
		}

		Ok(devices_result
			.into_iter()
			.map(|device| DeviceWithInfo {
				keys: keys_by_device.remove(&device.id).unwrap_or_default(),
				latest_connection: connections_by_device.remove(&device.id),
				device,
			})
			.collect())
	}

	/// Look up a single device by its exact `tailscale_node_id`,
	/// returning the full `DeviceWithInfo`. Used by the search
	/// endpoint when the user pastes a Tailscale IP/name that the
	/// directory resolves to a known node id.
	pub async fn get_with_info_by_node_id(
		db: &mut AsyncPgConnection,
		node_id: &str,
	) -> Result<Option<DeviceWithInfo>> {
		let Some(device) = Self::from_tailscale_node_id(db, node_id).await? else {
			return Ok(None);
		};
		Self::get_with_info(db, device.id).await.map(Some)
	}

	/// Every `(device, machine_id, node_id)` triple for devices that have a
	/// Tailscale node id and are bound to a machine. Drives the key-expiry
	/// sweep: the tailnet carries plenty of nodes that aren't canopy-managed
	/// boxes (operator laptops, other infra, …), and the sweep deliberately
	/// ignores those — it's scoped to the headless devices canopy actually
	/// runs.
	///
	/// The machine is the subject: a node's key expiring is a fact about the
	/// box, not about any one workload on it.
	// spec: DTR
	pub async fn list_tailnet_bound_machines(
		db: &mut AsyncPgConnection,
	) -> Result<Vec<(Self, Uuid, String)>> {
		use crate::schema::{devices, machines};

		let rows: Vec<(Self, Uuid, String)> = devices::table
			.inner_join(machines::table.on(machines::device_id.eq(devices::id.nullable())))
			.filter(devices::tailscale_node_id.is_not_null())
			.filter(machines::deleted_at.is_null())
			.select((
				Self::as_select(),
				machines::id,
				devices::tailscale_node_id.assume_not_null(),
			))
			.load(db)
			.await?;
		Ok(rows)
	}
}

impl DeviceKey {
	pub async fn create(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
		key: Vec<u8>,
		name: Option<String>,
	) -> Result<Self> {
		use crate::schema::device_keys::dsl;

		diesel::insert_into(dsl::device_keys)
			.values(&(
				dsl::device_id.eq(device_id),
				dsl::key_data.eq(key),
				dsl::name.eq(name),
				dsl::is_active.eq(true),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Note that the active key `key` authenticated over mutual TLS just now,
	/// unless it was noted within [`AUTHENTICATED_STAMP_EVERY`].
	// spec: TSI#encryption
	pub async fn mark_authenticated(db: &mut AsyncPgConnection, key: &[u8]) -> Result<()> {
		use crate::schema::device_keys::dsl;

		diesel::update(
			dsl::device_keys
				.filter(dsl::key_data.eq(key))
				.filter(dsl::is_active.eq(true))
				.filter(diesel::dsl::sql::<diesel::sql_types::Bool>(&format!(
					"(last_authenticated_at IS NULL \
					   OR last_authenticated_at < now() - interval '{} seconds')",
					AUTHENTICATED_STAMP_EVERY.as_secs()
				))),
		)
		.set(dsl::last_authenticated_at.eq(diesel::dsl::now))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(())
	}

	/// The active key of `device` that most recently authenticated over mutual
	/// TLS, which is the one its own agent holds. None when no active key of
	/// it has.
	// spec: TSI#encryption
	pub async fn last_authenticated(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
	) -> Result<Option<Self>> {
		Ok(Self::last_authenticated_of(db, &[device_id])
			.await?
			.remove(&device_id))
	}

	/// [`Self::last_authenticated`] for several devices at once.
	pub async fn last_authenticated_of(
		db: &mut AsyncPgConnection,
		device_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Self>> {
		use crate::schema::device_keys::dsl;

		if device_ids.is_empty() {
			return Ok(Default::default());
		}
		let keys: Vec<Self> = dsl::device_keys
			.select(Self::as_select())
			.filter(dsl::device_id.eq_any(device_ids))
			.filter(dsl::is_active.eq(true))
			.filter(dsl::last_authenticated_at.is_not_null())
			.distinct_on(dsl::device_id)
			.order((
				dsl::device_id,
				dsl::last_authenticated_at.desc(),
				dsl::id,
			))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(keys.into_iter().map(|key| (key.device_id, key)).collect())
	}

	pub async fn find_by_device(db: &mut AsyncPgConnection, device_id: Uuid) -> Result<Vec<Self>> {
		use crate::schema::device_keys::dsl;

		dsl::device_keys
			.select(Self::as_select())
			.filter(dsl::device_id.eq(device_id))
			.filter(dsl::is_active.eq(true))
			.order(dsl::created_at.asc())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn deactivate(db: &mut AsyncPgConnection, key_id: Uuid) -> Result<()> {
		use crate::schema::device_keys::dsl;

		diesel::update(dsl::device_keys.filter(dsl::id.eq(key_id)))
			.set(dsl::is_active.eq(false))
			.execute(db)
			.await
			.map_err(AppError::from)?;

		Ok(())
	}

	/// Re-enable a previously deactivated key. The inverse of [`deactivate`];
	/// used by the operator's per-key controls to undo a disable (e.g. during a
	/// botched rotation).
	pub async fn reactivate(db: &mut AsyncPgConnection, key_id: Uuid) -> Result<()> {
		use crate::schema::device_keys::dsl;

		diesel::update(dsl::device_keys.filter(dsl::id.eq(key_id)))
			.set(dsl::is_active.eq(true))
			.execute(db)
			.await
			.map_err(AppError::from)?;

		Ok(())
	}

	pub async fn update_name(
		db: &mut AsyncPgConnection,
		key_id: Uuid,
		name: Option<String>,
	) -> Result<()> {
		use crate::schema::device_keys::dsl;

		diesel::update(dsl::device_keys.filter(dsl::id.eq(key_id)))
			.set(dsl::name.eq(name))
			.execute(db)
			.await
			.map_err(AppError::from)?;

		Ok(())
	}
}

#[derive(Clone, Debug, Insertable)]
#[diesel(table_name = crate::schema::device_connections)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct NewDeviceConnection {
	pub device_id: Uuid,
	pub ip: ipnet::IpNet,
	pub user_agent: Option<String>,
}

impl NewDeviceConnection {
	pub async fn create(&self, db: &mut AsyncPgConnection) -> Result<DeviceConnection> {
		use crate::schema::device_connections::dsl as dc;

		diesel::insert_into(dc::device_connections)
			.values(self)
			.returning(DeviceConnection::as_select())
			.get_result::<DeviceConnection>(db)
			.await
			.map_err(AppError::from)
	}
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, QueryableByName, Selectable)]
#[diesel(table_name = crate::schema::device_connections)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct DeviceConnection {
	pub id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	pub device_id: Uuid,
	pub ip: ipnet::IpNet,
	pub user_agent: Option<String>,
}

impl DeviceConnection {
	pub async fn get_latest_from_device_ids(
		db: &mut AsyncPgConnection,
		device_ids: impl Iterator<Item = Uuid>,
	) -> Result<Vec<Self>> {
		let ids: Vec<Uuid> = device_ids.collect();
		if ids.is_empty() {
			return Ok(Vec::new());
		}

		// One LATERAL probe per device rather than DISTINCT ON over the
		// window: DISTINCT ON can't terminate early, so it reads every
		// connection row in the window for each device — and a row is
		// written per authenticated request, so that's ~all of a healthy
		// device's traffic for 90 days. The LIMIT 1 under the lateral join
		// reads one row per partition via the (device_id, created_at DESC)
		// composite index. Still bounded by created_at so partition pruning
		// engages.
		diesel::sql_query(
			"SELECT dc.* FROM unnest($1) AS d(id) \
			 CROSS JOIN LATERAL ( \
				SELECT * FROM device_connections \
				WHERE device_id = d.id AND created_at >= NOW() - INTERVAL '90 days' \
				ORDER BY created_at DESC LIMIT 1 \
			 ) dc",
		)
		.bind::<diesel::sql_types::Array<diesel::sql_types::Uuid>, _>(ids)
		.load(db)
		.await
		.map_err(AppError::from)
	}

	/// Get connection history for a device, newest first, optionally starting
	/// strictly before a `(created_at, id)` cursor (the last row of the
	/// previous page). The cursor pair makes pagination correct under ties on
	/// `created_at`. Backed by the `(device_id, created_at DESC)` composite
	/// index, so the `LIMIT` stops the scan early instead of fetching every
	/// matching row across all partitions.
	pub async fn get_history_for_device(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
		before: Option<(Timestamp, Uuid)>,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::device_connections::dsl as dc;
		use diesel::BoolExpressionMethods;

		let mut q = dc::device_connections
			.select(Self::as_select())
			.filter(dc::device_id.eq(device_id))
			.into_boxed();

		if let Some((before_ts, before_id)) = before {
			q = q.filter(
				dc::created_at
					.lt(jiff_diesel::Timestamp::from(before_ts))
					.or(dc::created_at
						.eq(jiff_diesel::Timestamp::from(before_ts))
						.and(dc::id.lt(before_id))),
			);
		}

		q.order((dc::created_at.desc(), dc::id.desc()))
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Get total connection count for a specific device.
	pub async fn get_connection_count_for_device(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
	) -> Result<i64> {
		use crate::schema::device_connections::dsl as dc;

		dc::device_connections
			.filter(dc::device_id.eq(device_id))
			.count()
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	pub fn nodejs_version(&self) -> Option<String> {
		self.user_agent.as_ref().and_then(|ua| {
			ua.split_ascii_whitespace()
				.find_map(|p| p.strip_prefix("Node.js/"))
				.map(ToOwned::to_owned)
		})
	}
}
