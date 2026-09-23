use std::collections::HashMap;

use axum::Json;
use axum::extract::State;
use canopy_utoipa_axum::{router::OpenApiRouter, routes};
use commons_errors::{AppError, ProblemDetailsSchema, Result};
use commons_servers::device_auth::keygen;
use commons_servers::tailnet_directory::DirectoryEntry;
use commons_servers::tailscale_auth::TailscaleAdmin;
use commons_types::{Uuid, device::DeviceRole};
use database::applications::Application;
use database::devices::{Device, DeviceConnection, DeviceKey, DeviceWithInfo, TailscaleIdentity};
use jiff::Timestamp;
use serde::{Deserialize, Serialize};
use utoipa::ToSchema;

use crate::fns::Page;
use crate::fns::applications::{
	ServerInfo, decorate_with_status, fill_display_hosts, server_to_info,
};
use crate::state::AppState;

/// Full record for a device: its identity and role, every authentication
/// key ever registered on it, its most recent connection, and a live
/// Tailscale snapshot when one is available.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeviceInfo {
	/// Core identity, role, and Tailscale attachment for the device.
	pub device: DeviceData,
	/// Every authentication key ever registered on this device, active or not.
	pub keys: Vec<DeviceKeyInfo>,
	/// The most recent connection seen from this device, if any.
	pub latest_connection: Option<DeviceConnectionData>,
	/// Live snapshot from the Tailscale network for a device that's
	/// currently attached to a tailnet node. Omitted if the device has
	/// no tailnet attachment, the tailnet directory integration isn't
	/// configured, or the node isn't found in the directory's cache.
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub tailnet_live: Option<TailnetLiveInfo>,
}

/// A live snapshot of a device's corresponding node on the Tailscale
/// network, fetched from the cached tailnet directory rather than stored
/// on the device record itself.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct TailnetLiveInfo {
	/// The Tailscale network's unique identifier for this node.
	pub node_id: String,
	/// The node's display name in the Tailscale admin console.
	pub display_name: String,
	/// The tailnet (Tailscale network) this node belongs to.
	pub tailnet: String,
	/// The node's current Tailscale IP addresses.
	pub addresses: Vec<String>,
	/// ACL tags applied to the node in Tailscale.
	pub tags: Vec<String>,
	/// When the Tailscale network last saw this node online. Omitted if
	/// that information wasn't available.
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub last_seen: Option<Timestamp>,
	/// Whether the node has been seen within the last 5 minutes — the
	/// same heuristic Tailscale's own admin console uses to mark a node
	/// online. False when `last_seen` is missing.
	pub online: bool,
}

/// `last_seen` newer than this counts the node as online. Matches the
/// Tailscale admin console's own heuristic.
const ONLINE_THRESHOLD: jiff::SignedDuration = jiff::SignedDuration::from_mins(5);

impl From<DirectoryEntry> for TailnetLiveInfo {
	fn from(e: DirectoryEntry) -> Self {
		let online = e
			.last_seen
			.is_some_and(|t| Timestamp::now().duration_since(t).abs() <= ONLINE_THRESHOLD);
		Self {
			node_id: e.node_id,
			display_name: e.node_name,
			tailnet: e.tailnet,
			addresses: e.addresses.into_iter().map(|a| a.to_string()).collect(),
			tags: e.tags,
			last_seen: e.last_seen,
			online,
		}
	}
}

impl DeviceInfo {
	pub fn name(&self) -> String {
		self.keys
			.iter()
			.filter(|key| key.is_active)
			.filter_map(|key| {
				key.name
					.as_ref()
					.filter(|name| *name != "Initial Key")
					.cloned()
			})
			.next_back()
			.or_else(|| {
				self.latest_connection
					.as_ref()
					.map(|conn| conn.ip.to_string())
			})
			.unwrap_or_else(|| self.device.id.to_string())
	}
}

/// Core identity and trust state of a device.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeviceData {
	/// Unique identifier for the device.
	pub id: Uuid,
	/// When the device was first registered.
	pub created_at: Timestamp,
	/// When the device record was last changed.
	pub updated_at: Timestamp,
	/// The device's current role, which determines what it's trusted to
	/// do (act as a monitored server, publish releases, administer the
	/// fleet, or perform backup restores).
	pub role: DeviceRole,
	/// The Tailscale node this device is attached to, if any. The live
	/// address and display name for this node, when available, are
	/// included separately in the response's tailnet snapshot.
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub tailscale_node_id: Option<String>,
	/// The Tailscale display name recorded for the attached node, if any.
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub tailscale_node_name: Option<String>,
	/// The tailnet (Tailscale network) the attached node belongs to, if any.
	#[serde(skip_serializing_if = "Option::is_none", default)]
	pub tailscale_tailnet: Option<String>,
}

/// An authentication key registered on a device.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeviceKeyInfo {
	/// Unique identifier for this key.
	pub id: Uuid,
	/// The device this key belongs to.
	pub device_id: Uuid,
	/// Operator-assigned label for the key, if any.
	pub name: Option<String>,
	/// The key's public half, PEM-encoded.
	pub pem_data: String,
	/// When the key was added.
	pub created_at: Timestamp,
	/// Whether this key can currently authenticate. Inactive keys are kept for
	/// history and can be re-enabled.
	pub is_active: bool,
}

/// A single recorded connection from a device to the API.
#[derive(Debug, Clone, Serialize, Deserialize, ToSchema)]
pub struct DeviceConnectionData {
	/// Unique identifier for this connection record.
	pub id: Uuid,
	/// When the connection was recorded.
	pub created_at: Timestamp,
	/// The device that connected.
	pub device_id: Uuid,
	/// IP address the device connected from.
	pub ip: String,
	/// User-Agent header sent with the request, if any.
	pub user_agent: Option<String>,
}

impl DeviceInfo {
	/// Build a wire-shape `DeviceInfo` from a `DeviceWithInfo` row plus
	/// the live tailnet snapshot from the directory (if any). This is
	/// the **only** way to produce a `DeviceInfo` — every handler that
	/// returns one should go through here so `tailnet_live` is filled
	/// consistently across views.
	pub(super) async fn from_db(d: DeviceWithInfo, state: &AppState) -> Self {
		let mut info = Self {
			device: DeviceData {
				id: d.device.id,
				created_at: d.device.created_at,
				updated_at: d.device.updated_at,
				role: d.device.role,
				tailscale_node_id: d.device.tailscale_node_id,
				tailscale_node_name: d.device.tailscale_node_name,
				tailscale_tailnet: d.device.tailscale_tailnet,
			},
			keys: d.keys.into_iter().map(DeviceKeyInfo::from).collect(),
			latest_connection: d.latest_connection.map(DeviceConnectionData::from),
			tailnet_live: None,
		};
		if let Some(node_id) = info.device.tailscale_node_id.clone()
			&& let Some(directory) = &state.tailnet_directory
			&& let Ok(Some(entry)) = directory.find_by_node_id(&node_id).await
		{
			info.tailnet_live = Some(entry.into());
		}
		info
	}
}

impl From<DeviceKey> for DeviceKeyInfo {
	fn from(key: DeviceKey) -> Self {
		Self {
			id: key.id,
			device_id: key.device_id,
			name: key.name,
			pem_data: format_key_as_pem(&key.key_data),
			created_at: key.created_at,
			is_active: key.is_active,
		}
	}
}

impl From<DeviceConnection> for DeviceConnectionData {
	fn from(conn: DeviceConnection) -> Self {
		Self {
			id: conn.id,
			created_at: conn.created_at,
			device_id: conn.device_id,
			ip: conn.ip.addr().to_string(),
			user_agent: conn.user_agent,
		}
	}
}

fn format_key_as_pem(key_data: &[u8]) -> String {
	use base64::prelude::*;
	let base64_data = BASE64_STANDARD.encode(key_data);
	let mut pem = String::with_capacity(base64_data.len() + 100);
	pem.push_str("-----BEGIN PUBLIC KEY-----\n");
	for chunk in base64_data.as_bytes().chunks(64) {
		pem.push_str(&String::from_utf8_lossy(chunk));
		pem.push('\n');
	}
	pem.push_str("-----END PUBLIC KEY-----");
	pem
}

pub fn routes() -> OpenApiRouter<AppState> {
	OpenApiRouter::new()
		.routes(routes!(read_only: get_device_by_id))
		.routes(routes!(read_only: get_servers_for_device))
		.routes(routes!(read_only: connection_history))
		.routes(routes!(read_only: connection_count))
		.routes(routes!(read_only: list_trusted))
		.routes(routes!(danger: disable_all_keys))
		.routes(routes!(danger: update_role))
		.routes(routes!(danger: provision_credential))
		.routes(routes!(danger: add_key))
		.routes(routes!(danger: deactivate_key))
		.routes(routes!(danger: reactivate_key))
		.routes(routes!(read_only: search))
		.routes(routes!(write: update_key_name))
		.routes(routes!(danger: attach_tailscale))
		.routes(routes!(danger: detach_tailscale))
		.routes(routes!(danger: merge_into))
		.routes(routes!(read_only: resolve_tailnet_identifier))
}

/// Identifies a single device by id.
#[derive(Deserialize, ToSchema)]
pub struct DeviceIdArgs {
	/// The device to operate on.
	pub device_id: Uuid,
}

/// Look up a single device by id.
///
/// Returns full device details: its identity and role, every key ever
/// registered on it, its most recent connection, and a live Tailscale
/// snapshot when available. Returns 404 if no device exists with that id.
#[utoipa::path(
	post,
	path = "/get_device_by_id",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = DeviceIdArgs,
	responses(
		(status = 200, body = DeviceInfo),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn get_device_by_id(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeviceIdArgs>,
) -> Result<Json<DeviceInfo>> {
	let mut conn = state.db.get().await?;
	let device_with_info = Device::get_with_info(&mut conn, args.device_id).await?;
	let info = DeviceInfo::from_db(device_with_info, &state).await;
	Ok(Json(info))
}

/// Pagination window for a paged listing endpoint.
#[derive(Deserialize, ToSchema)]
pub struct PaginationArgs {
	/// Number of items to skip from the start of the result set.
	pub offset: u64,
	/// Maximum number of items to return. Endpoints that use this type
	/// apply their own default when omitted.
	pub limit: Option<u64>,
}

/// List the applications a device is currently associated with.
///
/// Returns the applications this device is currently bound to (usually zero or
/// one), including reachability status and a best-effort display address
/// for each.
#[utoipa::path(
	post,
	path = "/get_servers_for_device",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = DeviceIdArgs,
	responses(
		(status = 200, body = Vec<ServerInfo>),
	),
)]
pub async fn get_servers_for_device(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeviceIdArgs>,
) -> Result<Json<Vec<ServerInfo>>> {
	let mut conn = state.db.get().await?;
	let applications = Application::get_by_device_id(&mut conn, args.device_id).await?;
	let mut infos: Vec<ServerInfo> = applications.into_iter().map(server_to_info).collect();
	decorate_with_status(&mut conn, &mut infos).await?;
	fill_display_hosts(&mut conn, &mut infos).await?;
	Ok(Json(infos))
}

/// Pagination cursor for connection history, pointing to a specific
/// connection record.
#[derive(Deserialize, ToSchema)]
pub struct HistoryCursor {
	/// Timestamp of the connection to page before.
	pub created_at: Timestamp,
	/// Id of the connection to page before, used to break ties when
	/// timestamps match.
	pub id: Uuid,
}

/// Request parameters for listing a device's connection history.
#[derive(Deserialize, ToSchema)]
pub struct ConnectionHistoryArgs {
	/// The device whose connection history to list.
	pub device_id: Uuid,
	/// Cursor to page backwards from. Omit to start from the most recent
	/// connection.
	pub before: Option<HistoryCursor>,
	/// Maximum number of connections to return. Defaults to 100.
	pub limit: Option<u64>,
}

/// List a device's connection history, most recent first.
///
/// Supports cursor-based pagination: pass the oldest entry from a previous
/// page as `before` to continue further back in time.
#[utoipa::path(
	post,
	path = "/connection_history",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = ConnectionHistoryArgs,
	responses(
		(status = 200, body = Vec<DeviceConnectionData>),
	),
)]
pub async fn connection_history(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ConnectionHistoryArgs>,
) -> Result<Json<Vec<DeviceConnectionData>>> {
	let mut conn = state.db.get().await?;
	let before = args.before.map(|c| (c.created_at, c.id));
	let connections = DeviceConnection::get_history_for_device(
		&mut conn,
		args.device_id,
		before,
		args.limit.unwrap_or(100).try_into().unwrap_or(100),
	)
	.await?;
	Ok(Json(
		connections
			.into_iter()
			.map(DeviceConnectionData::from)
			.collect(),
	))
}

/// Count how many times a device has connected.
///
/// Returns the total number of connections recorded for the device, for
/// paginating its connection history.
#[utoipa::path(
	post,
	path = "/connection_count",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = DeviceIdArgs,
	responses(
		(status = 200, body = u64, content_type = "application/json"),
	),
)]
pub async fn connection_count(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeviceIdArgs>,
) -> Result<Json<u64>> {
	let mut conn = state.db.get().await?;
	Ok(Json(
		DeviceConnection::get_connection_count_for_device(&mut conn, args.device_id)
			.await?
			.try_into()
			.unwrap_or_default(),
	))
}

/// Identifies a device and the role to trust it at.
#[derive(Deserialize, ToSchema)]
pub struct TrustArgs {
	/// The device to update.
	pub device_id: Uuid,
	/// The role to assign to the device.
	pub role: DeviceRole,
}

/// List registered devices, paginated.
///
/// Returns a page of the device registry, newest first, plus the total
/// count for rendering a pager. Every registered device is included,
/// whatever its role.
#[utoipa::path(
	post,
	path = "/list_trusted",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = PaginationArgs,
	responses(
		(status = 200, body = Page<DeviceInfo>),
	),
)]
pub async fn list_trusted(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<PaginationArgs>,
) -> Result<Json<Page<DeviceInfo>>> {
	let mut conn = state.db.get().await?;
	let total = Device::count_trusted(&mut conn)
		.await?
		.try_into()
		.unwrap_or(0);
	let devices_with_info = Device::list_trusted_with_info_paginated(
		&mut conn,
		args.limit.unwrap_or(10).try_into().unwrap_or(10),
		args.offset.try_into().unwrap_or(0),
	)
	.await?;
	let mut items = Vec::with_capacity(devices_with_info.len());
	for d in devices_with_info {
		items.push(DeviceInfo::from_db(d, &state).await);
	}
	Ok(Json(Page { items, total }))
}

/// Disable every active key on a device, so none of them can authenticate.
///
/// The keys stay in the device's history and can be re-enabled individually.
/// Any Tailscale identity attached to the device is untouched — detach it
/// separately.
#[utoipa::path(
	post,
	path = "/disable_all_keys",
	operation_id = "device_disable_all_keys",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = DeviceIdArgs,
	responses(
		(status = 200),
	),
)]
pub async fn disable_all_keys(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeviceIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Device::deactivate_keys(&mut conn, args.device_id).await?;
	Ok(Json(()))
}

/// Change the role a device is trusted at.
///
/// Immediately changes what the device is permitted to do system-wide.
#[utoipa::path(
	post,
	path = "/update_role",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = TrustArgs,
	responses(
		(status = 200),
		(status = 400, body = ProblemDetailsSchema),
	),
)]
pub async fn update_role(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<TrustArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	Device::trust(&mut conn, args.device_id, args.role).await?;
	Ok(Json(()))
}

/// Request to mint a new device credential.
#[derive(Deserialize, ToSchema)]
pub struct ProvisionArgs {
	/// Role to trust the device at.
	pub role: DeviceRole,
	/// Attach the new credential to this existing device instead of
	/// creating a new one. When omitted, a new device is created at
	/// `role`.
	#[serde(default)]
	pub device_id: Option<Uuid>,
	/// Display name for the new key. Defaults to "Provisioned key".
	#[serde(default)]
	pub key_name: Option<String>,
}

/// The one-time result of provisioning a device credential. The plaintext
/// private key lives only inside `key_age_base64`; Canopy never persists it.
#[derive(Serialize, ToSchema)]
pub struct ProvisionedCredential {
	/// The device the credential was issued for.
	pub device_id: Uuid,
	/// Unique identifier for the newly-created key record.
	pub key_id: Uuid,
	/// Lowercase hex SHA-256 of the stored public key, to correlate the
	/// credential with the device's key list.
	pub fingerprint: String,
	/// Suggested filename for the downloaded encrypted key.
	pub filename: String,
	/// Base64 (standard) of the age-encrypted PKCS#8 PEM private key. Decrypt
	/// with `passphrase` (e.g. `bestool crypto reveal`) to recover the PEM.
	pub key_age_base64: String,
	/// Freshly-generated passphrase that decrypts `key_age_base64`. Share it
	/// out-of-band, on a separate channel from the file.
	pub passphrase: String,
}

/// Generate a new device credential and return the private key once.
///
/// Generates a keypair, registers its public key as an active key on a new
/// or existing device at the chosen role, and returns the private key
/// encrypted under a freshly-generated passphrase. Canopy keeps only the
/// public key — the private key is never stored or logged after this
/// response. Returns 404 if `device_id` is given but doesn't match an
/// existing device.
#[utoipa::path(
	post,
	path = "/provision_credential",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = ProvisionArgs,
	responses(
		(status = 200, body = ProvisionedCredential),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn provision_credential(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ProvisionArgs>,
) -> Result<Json<ProvisionedCredential>> {
	let mut conn = state.db.get().await?;
	let key_name = args
		.key_name
		.filter(|n| !n.trim().is_empty())
		.unwrap_or_else(|| "Provisioned key".to_string());
	let credential =
		mint_provisioned_credential(&mut conn, args.role, args.device_id, &key_name, false).await?;
	Ok(Json(credential))
}

/// Mint a device credential and return the private key once, the shared core of
/// [`provision_credential`] and cluster registration.
///
/// With `device_id` set, the credential is attached to that existing device
/// (created via [`DeviceKey::create`], which inserts a second active key rather
/// than refusing one); with it unset, a new device is created at `role`. Canopy
/// keeps only the public key — the private key is encrypted under a fresh
/// passphrase and never stored or logged after this returns.
///
/// `deactivate_superseded` retires the device's existing keys before minting the
/// new one, for re-issuing a credential that was lost before it reached its
/// destination: the superseded key was never deployed, so retiring it costs
/// nothing and keeps a device from accumulating active keys.
pub(crate) async fn mint_provisioned_credential(
	conn: &mut database::diesel_async::AsyncPgConnection,
	role: DeviceRole,
	device_id: Option<Uuid>,
	key_name: &str,
	deactivate_superseded: bool,
) -> Result<ProvisionedCredential> {
	use algae_cli::{
		passphrases::{Passphrase, SecretString},
		streams::encrypt_stream,
	};
	use base64::Engine as _;

	let generated = keygen::generate_device_key()?;

	let (device_id, key) = match device_id {
		Some(device_id) => {
			// 404s if the device doesn't exist.
			let existing = Device::get_with_info(conn, device_id).await?;
			if deactivate_superseded {
				Device::deactivate_keys(conn, device_id).await?;
			}
			let key = DeviceKey::create(
				conn,
				device_id,
				generated.spki_der,
				Some(key_name.to_string()),
			)
			.await?;
			if existing.device.role != role {
				Device::trust(conn, device_id, role).await?;
			}
			(device_id, key)
		}
		None => {
			let device =
				Device::create_at_role(conn, generated.spki_der, role, Some(key_name.to_string()))
					.await?;
			let key = DeviceKey::find_by_device(conn, device.id)
				.await?
				.into_iter()
				.next()
				.ok_or_else(|| AppError::custom("provisioned device has no key"))?;
			(device.id, key)
		}
	};

	// Encrypt the private PEM with a fresh passphrase (age/scrypt), the same
	// primitives bestool's `crypto reveal` reads. The ciphertext is base64'd
	// for transport; the passphrase travels out-of-band.
	let passphrase = crate::fns::generate_passphrase();
	let encryptor = Passphrase::new(SecretString::from(passphrase.clone()));
	let mut encrypted = Vec::new();
	encrypt_stream(
		generated.private_key_pem.as_bytes(),
		futures::io::Cursor::new(&mut encrypted),
		Box::new(encryptor),
	)
	.await
	.map_err(|e| AppError::custom(format!("encrypting device key: {e}")))?;
	let key_age_base64 = base64::engine::general_purpose::STANDARD.encode(&encrypted);

	let filename = format!("canopy-{}-{}.pem.age", role, &generated.fingerprint[..12]);

	Ok(ProvisionedCredential {
		device_id,
		key_id: key.id,
		fingerprint: generated.fingerprint,
		filename,
		key_age_base64,
		passphrase,
	})
}

/// Request to register an existing public key on a device.
#[derive(Deserialize, ToSchema)]
pub struct AddKeyArgs {
	/// The device to add the key to.
	pub device_id: Uuid,
	/// The device's public key, PEM-encoded `SubjectPublicKeyInfo`
	/// (`-----BEGIN PUBLIC KEY-----`). Bare base64 (no armor) is also accepted.
	pub public_key_pem: String,
	/// Display name for the key. Defaults to "Added key".
	#[serde(default)]
	pub name: Option<String>,
}

/// Register an externally-generated public key as an active key on a device.
///
/// Unlike the `provision_credential` endpoint, Canopy never generates or
/// sees a private key here — the operator supplies the public half of a
/// keypair they already hold. Returns 400 if the supplied value isn't a
/// valid public key, and 404 if the device doesn't exist.
#[utoipa::path(
	post,
	path = "/add_key",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = AddKeyArgs,
	responses(
		(status = 200, body = DeviceInfo),
		(status = 400, body = ProblemDetailsSchema),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn add_key(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<AddKeyArgs>,
) -> Result<Json<DeviceInfo>> {
	use base64::Engine as _;

	// Accept PEM armor or bare base64; decode to DER and validate it's an SPKI.
	let b64: String = args
		.public_key_pem
		.lines()
		.filter(|l| !l.trim_start().starts_with("-----"))
		.collect::<Vec<_>>()
		.join("");
	let der = base64::engine::general_purpose::STANDARD
		.decode(b64.trim())
		.map_err(|e| AppError::BadRequest(format!("public key is not valid base64/PEM: {e}")))?;
	keygen::validate_spki_der(&der)?;

	let name = args
		.name
		.filter(|n| !n.trim().is_empty())
		.unwrap_or_else(|| "Added key".to_string());

	let mut conn = state.db.get().await?;
	// 404s if the device doesn't exist.
	Device::get_with_info(&mut conn, args.device_id).await?;
	DeviceKey::create(&mut conn, args.device_id, der, Some(name)).await?;

	let info = Device::get_with_info(&mut conn, args.device_id).await?;
	Ok(Json(DeviceInfo::from_db(info, &state).await))
}

/// Identifies a single device key by id.
#[derive(Deserialize, ToSchema)]
pub struct KeyIdArgs {
	/// The key to operate on.
	pub key_id: Uuid,
}

/// Disable a single key so it can no longer authenticate.
///
/// The key record is kept and can be re-enabled later. Disabling one key
/// while others remain active lets a device rotate keys with no downtime:
/// add the new key first, then disable the old one.
#[utoipa::path(
	post,
	path = "/deactivate_key",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = KeyIdArgs,
	responses((status = 200)),
)]
pub async fn deactivate_key(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<KeyIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	DeviceKey::deactivate(&mut conn, args.key_id).await?;
	Ok(Json(()))
}

/// Re-enable a previously disabled device key.
///
/// The key can authenticate again immediately.
#[utoipa::path(
	post,
	path = "/reactivate_key",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = KeyIdArgs,
	responses((status = 200)),
)]
pub async fn reactivate_key(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<KeyIdArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	DeviceKey::reactivate(&mut conn, args.key_id).await?;
	Ok(Json(()))
}

/// Free-text search query for devices.
#[derive(Deserialize, ToSchema)]
pub struct DeviceSearchArgs {
	/// Search text, matched against key material, key names, connection
	/// IP addresses, and Tailscale identity fields. Also resolved
	/// against the live tailnet directory (when configured) to catch
	/// devices identified by an IP, node id, or DNS name they haven't
	/// connected with yet.
	pub query: String,
}

/// Search for devices by key, name, IP address, or Tailscale identity.
///
/// Returns an empty list if the query is blank.
#[utoipa::path(
	post,
	path = "/search",
	operation_id = "device_search",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = DeviceSearchArgs,
	responses(
		(status = 200, body = Vec<DeviceInfo>),
	),
)]
pub async fn search(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeviceSearchArgs>,
) -> Result<Json<Vec<DeviceInfo>>> {
	if args.query.trim().is_empty() {
		return Ok(Json(vec![]));
	}
	let mut conn = state.db.get().await?;
	let devices_by_key = Device::search_by_key(&mut conn, &args.query).await?;
	let devices_by_key_name = Device::search_by_key_name(&mut conn, &args.query).await?;
	let devices_by_ip = Device::search_by_connection_ip(&mut conn, &args.query).await?;
	let devices_by_tailscale = Device::search_by_tailscale_fields(&mut conn, &args.query).await?;

	// If the directory is configured, also resolve the query as a
	// Tailscale IP / node id / DNS name and surface any device
	// attached to the resolved node. This catches the case where the
	// operator pastes an identifier the device hasn't connected with
	// yet (so search_by_connection_ip would miss it).
	let directory_match = if let Some(directory) = &state.tailnet_directory {
		match directory.resolve_identifier(&args.query).await {
			Ok(Some(entry)) => Device::get_with_info_by_node_id(&mut conn, &entry.node_id).await?,
			_ => None,
		}
	} else {
		None
	};

	let mut seen: HashMap<Uuid, DeviceWithInfo> = HashMap::new();
	for d in devices_by_key {
		seen.insert(d.device.id, d);
	}
	for d in devices_by_key_name {
		seen.insert(d.device.id, d);
	}
	for d in devices_by_ip {
		seen.insert(d.device.id, d);
	}
	for d in devices_by_tailscale {
		seen.insert(d.device.id, d);
	}
	if let Some(d) = directory_match {
		seen.insert(d.device.id, d);
	}

	let mut out = Vec::with_capacity(seen.len());
	for d in seen.into_values() {
		out.push(DeviceInfo::from_db(d, &state).await);
	}
	Ok(Json(out))
}

/// Request to rename (or clear the name of) a device key.
#[derive(Deserialize, ToSchema)]
pub struct UpdateKeyNameArgs {
	/// The key to rename.
	pub key_id: Uuid,
	/// New display name for the key, or null to clear it.
	pub name: Option<String>,
}

/// Rename a device key, or clear its name.
///
/// Sets the key's display name to the given value, or removes the name
/// when `null` is passed.
#[utoipa::path(
	post,
	path = "/update_key_name",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = UpdateKeyNameArgs,
	responses(
		(status = 200),
	),
)]
pub async fn update_key_name(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<UpdateKeyNameArgs>,
) -> Result<Json<()>> {
	let mut conn = state.db.get().await?;
	DeviceKey::update_name(&mut conn, args.key_id, args.name).await?;
	Ok(Json(()))
}

/// Request to attach a device to a Tailscale network node.
#[derive(Deserialize, ToSchema)]
pub struct AttachTailscaleArgs {
	/// The device to attach a tailnet identity to.
	pub device_id: Uuid,
	/// Any of: a Tailscale CGNAT/ULA IP, a node id, or a DNS name —
	/// the operator pastes whichever is most convenient from the
	/// Tailscale admin console. The server resolves it via the
	/// cached directory to the canonical `(node_id, name, tailnet)`
	/// tuple before persisting.
	pub identifier: String,
}

/// Attach a device to a Tailscale network node.
///
/// Resolves the given identifier against the cached tailnet directory and
/// records the canonical node identity on the device. Returns the updated
/// device record, including its live tailnet snapshot when available.
#[utoipa::path(
	post,
	path = "/attach_tailscale",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = AttachTailscaleArgs,
	responses(
		(status = 200, body = DeviceInfo),
		(status = 404, description = "Identifier does not resolve to a known tailnet node.", body = ProblemDetailsSchema),
		(status = 409, description = "Another device already claims this node id.", body = ProblemDetailsSchema),
		(status = 503, description = "Tailnet directory not configured or unreachable.", body = ProblemDetailsSchema),
	),
)]
pub async fn attach_tailscale(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<AttachTailscaleArgs>,
) -> Result<Json<DeviceInfo>> {
	let directory = state
		.tailnet_directory
		.as_ref()
		.ok_or(AppError::AuthTailnetDirectoryUnavailable)?;
	let entry = directory
		.resolve_identifier(&args.identifier)
		.await
		.map_err(|_| AppError::AuthTailnetDirectoryUnavailable)?
		.ok_or_else(|| AppError::NotFound("no tailnet device matches that identifier".into()))?;

	let mut conn = state.db.get().await?;
	Device::attach_tailscale(
		&mut conn,
		args.device_id,
		TailscaleIdentity {
			node_id: entry.node_id.clone(),
			node_name: Some(entry.node_name.clone()),
			tailnet: Some(entry.tailnet.clone()),
		},
	)
	.await?;

	let device_with_info = Device::get_with_info(&mut conn, args.device_id).await?;
	let info = DeviceInfo::from_db(device_with_info, &state).await;
	Ok(Json(info))
}

/// Detach a device from its Tailscale network node.
///
/// Returns the updated device record, with its tailnet identity cleared.
/// Returns 404 if the device doesn't exist.
#[utoipa::path(
	post,
	path = "/detach_tailscale",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = DeviceIdArgs,
	responses(
		(status = 200, body = DeviceInfo),
		(status = 404, body = ProblemDetailsSchema),
	),
)]
pub async fn detach_tailscale(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<DeviceIdArgs>,
) -> Result<Json<DeviceInfo>> {
	let mut conn = state.db.get().await?;
	Device::detach_tailscale(&mut conn, args.device_id).await?;
	let device_with_info = Device::get_with_info(&mut conn, args.device_id).await?;
	let info = DeviceInfo::from_db(device_with_info, &state).await;
	Ok(Json(info))
}

/// Request to merge two device records into one.
#[derive(Deserialize, ToSchema)]
pub struct MergeIntoArgs {
	/// The device to merge away — usually the automatically-discovered,
	/// tailnet-only device that doesn't yet have its own certificate
	/// key. This device is removed once the merge completes.
	pub source_id: Uuid,
	/// The device that remains after the merge — usually the existing
	/// device that already owns the server attachment and connection
	/// history.
	pub target_id: Uuid,
}

/// Merge one device record into another.
///
/// Combines keys, connections, and tailnet/server attachments onto the
/// target device and removes the source. Used to reconcile a device that
/// was auto-discovered via the tailnet directory with the device it
/// actually corresponds to, once that device authenticates with its own
/// key. Returns 409 if both devices independently hold a Tailscale
/// identity or a server attachment, since the merge can't decide which one
/// to keep.
#[utoipa::path(
	post,
	path = "/merge_into",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = MergeIntoArgs,
	responses(
		(status = 200, body = DeviceInfo),
		(status = 404, body = ProblemDetailsSchema),
		(status = 409, description = "Both source and target hold tailscale identity or a server attachment.", body = ProblemDetailsSchema),
	),
)]
pub async fn merge_into(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<MergeIntoArgs>,
) -> Result<Json<DeviceInfo>> {
	let mut conn = state.db.get().await?;
	Device::merge_into(&mut conn, args.source_id, args.target_id).await?;
	let device_with_info = Device::get_with_info(&mut conn, args.target_id).await?;
	let info = DeviceInfo::from_db(device_with_info, &state).await;
	Ok(Json(info))
}

/// A Tailscale identifier to resolve against the live tailnet directory.
#[derive(Deserialize, ToSchema)]
pub struct ResolveTailnetIdentifierArgs {
	/// A Tailscale IP address, node id, or DNS name to look up.
	pub identifier: String,
}

/// Result of resolving a Tailscale identifier.
#[derive(Serialize, ToSchema)]
pub struct ResolveTailnetIdentifierResponse {
	/// The resolved node's live details, or null if the identifier
	/// didn't match any node in the tailnet.
	pub matched: Option<TailnetLiveInfo>,
}

/// Look up a Tailscale IP address, node id, or DNS name.
///
/// Resolves the identifier against the live tailnet directory and returns
/// its canonical node identity if found. Useful for confirming what an
/// identifier resolves to before using it to attach a device.
#[utoipa::path(
	post,
	path = "/resolve_tailnet_identifier",
	tag = "devices",
	security(("tailscale-admin" = [])),
	request_body = ResolveTailnetIdentifierArgs,
	responses(
		(status = 200, body = ResolveTailnetIdentifierResponse),
		(status = 503, description = "Tailnet directory not configured or unreachable.", body = ProblemDetailsSchema),
	),
)]
pub async fn resolve_tailnet_identifier(
	State(state): State<AppState>,
	_admin: TailscaleAdmin,
	Json(args): Json<ResolveTailnetIdentifierArgs>,
) -> Result<Json<ResolveTailnetIdentifierResponse>> {
	let directory = state
		.tailnet_directory
		.as_ref()
		.ok_or(AppError::AuthTailnetDirectoryUnavailable)?;
	let matched = directory
		.resolve_identifier(&args.identifier)
		.await
		.map_err(|_| AppError::AuthTailnetDirectoryUnavailable)?
		.map(TailnetLiveInfo::from);
	Ok(Json(ResolveTailnetIdentifierResponse { matched }))
}
