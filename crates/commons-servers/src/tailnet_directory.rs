//! Read-through cache mapping a tailnet IP → its node identity, backed
//! by the Tailscale control-plane REST API.
//!
//! The private-server's device-auth extractor uses this to translate an
//! incoming request's `X-Forwarded-For` (set by the Tailscale Operator
//! ingress proxy to the caller's CGNAT v4 or ULA v6 address) into a
//! stable `node_id`, which then keys into the `devices.tailscale_node_id`
//! column.
//!
//! The directory is constructed only on private-server (public-server's
//! `AppState` doesn't carry one), so the tailnet auth path can't fire on
//! the internet edge by type-system construction.

use std::{
	collections::{HashMap, HashSet},
	net::IpAddr,
	sync::Arc,
	time::{Duration, Instant},
};

use commons_errors::{AppError, Result};
use jiff::Timestamp;
use reqwest::Client;
use serde::Deserialize;
use tokio::sync::RwLock;

/// Resolved node identity for a tailnet IP. `addresses` carries every
/// IP the control plane reports for this node, so a UI can surface
/// the current IPs of a device that's already attached by node id.
#[derive(Clone, Debug)]
pub struct DirectoryEntry {
	pub node_id: String,
	pub node_name: String,
	pub tailnet: String,
	pub tags: Vec<String>,
	pub addresses: Vec<IpAddr>,
	/// When the Tailscale control plane last saw this node. `None` if
	/// the API didn't return a value or the value didn't parse.
	pub last_seen: Option<Timestamp>,
	/// True if the node's key has been pinned not to expire. Headless
	/// canopy-managed devices should always have this on; if it's
	/// false the node will drop off the tailnet when its key expires.
	pub key_expiry_disabled: bool,
}

/// Configuration for constructing a [`TailnetDirectory`].
#[derive(Clone, Debug)]
pub struct TailnetDirectoryConfig {
	pub oauth_client_id: String,
	pub oauth_client_secret: String,
	/// Tailnet identifier — typically the email-like ID shown in the
	/// admin console (e.g. `example.com` or `tailbeef-bes.au`).
	pub tailnet: String,
	/// Base URL for the Tailscale REST API. Defaults to
	/// `https://api.tailscale.com`; overridable for testing.
	pub api_base: String,
	pub refresh_period: Duration,
	pub miss_cooldown: Duration,
}

impl TailnetDirectoryConfig {
	/// Read the config from environment variables. Returns `Ok(None)` when
	/// the required vars are unset — that's the "tailnet auth disabled"
	/// path, and callers should treat it as a normal state.
	pub fn from_env() -> Result<Option<Self>> {
		let client_id = std::env::var("TAILSCALE_OAUTH_CLIENT_ID").ok();
		let client_secret = std::env::var("TAILSCALE_OAUTH_CLIENT_SECRET").ok();
		let tailnet = std::env::var("TAILSCALE_TAILNET").ok();

		match (client_id, client_secret, tailnet) {
			(Some(id), Some(secret), Some(net)) => Ok(Some(Self {
				oauth_client_id: id,
				oauth_client_secret: secret,
				tailnet: net,
				api_base: std::env::var("TAILSCALE_API_BASE")
					.unwrap_or_else(|_| "https://api.tailscale.com".into()),
				refresh_period: Duration::from_secs(60),
				miss_cooldown: Duration::from_secs(5),
			})),
			(None, None, None) => Ok(None),
			_ => Err(AppError::custom(
				"TAILSCALE_OAUTH_CLIENT_ID, TAILSCALE_OAUTH_CLIENT_SECRET, and TAILSCALE_TAILNET must all be set together",
			)),
		}
	}
}

#[derive(Clone, Debug)]
pub struct TailnetDirectory {
	inner: Arc<Inner>,
}

#[derive(Debug)]
struct Inner {
	client: Client,
	config: TailnetDirectoryConfig,
	cache: RwLock<Cache>,
	oauth: RwLock<OAuthState>,
}

#[derive(Debug, Default)]
struct Cache {
	by_ip: HashMap<IpAddr, DirectoryEntry>,
	by_node_id: HashMap<String, DirectoryEntry>,
	last_refresh: Option<Instant>,
	/// Permissions derived from the tailnet policy's `bes.au/cap/canopy`
	/// grants, recomputed on each successful policy read. Empty until the
	/// first successful read; a failed read leaves the previous value in
	/// place so a control-plane outage never withdraws a policy-granted
	/// permission.
	permissions: PermissionGrants,
}

/// The Canopy application capability whose payload confers permissions, and the
/// service tag a conferring grant must target. The value's `admin` key confers
/// administrator and its `danger` key confers danger (see the ADM spec); a value
/// may carry either, both, or neither.
const CANOPY_CAP: &str = "bes.au/cap/canopy";
const CANOPY_SERVICE_TAG: &str = "tag:server-canopy";

/// The two permissions resolved from the tailnet policy. The keys are
/// independent — a grant carrying only `danger` confers danger and not
/// administrator, and vice versa.
#[derive(Debug, Default, Clone)]
struct PermissionGrants {
	admin: GrantSet,
	danger: GrantSet,
}

/// The callers a set of conferring grants covers for one permission.
#[derive(Debug, Default, Clone)]
struct GrantSet {
	/// Explicit logins, from group members and bare user sources, lowercased
	/// for case-insensitive comparison.
	logins: HashSet<String>,
	/// True when a conferring grant's source covers every tailnet user
	/// identity (`autogroup:member`).
	all_members: bool,
}

impl GrantSet {
	fn covers(&self, login: &str) -> bool {
		self.all_members || self.logins.contains(login)
	}
}

/// The subset of the tailnet policy file this directory reads.
#[derive(Debug, Deserialize)]
struct PolicyFile {
	#[serde(default)]
	groups: HashMap<String, Vec<String>>,
	#[serde(default)]
	grants: Vec<Grant>,
}

#[derive(Debug, Deserialize)]
struct Grant {
	#[serde(default)]
	src: Vec<String>,
	#[serde(default)]
	dst: Vec<String>,
	#[serde(default)]
	app: HashMap<String, Vec<serde_json::Value>>,
}

/// Whether a `bes.au/cap/canopy` capability value array sets a given key true in
/// any of its entries.
fn cap_confers(values: &[serde_json::Value], key: &str) -> bool {
	values
		.iter()
		.any(|v| v.get(key).and_then(serde_json::Value::as_bool) == Some(true))
}

/// Add a grant's sources to a permission's [`GrantSet`], resolving each against
/// the policy's groups.
fn extend_from_sources(set: &mut GrantSet, sources: &[String], policy: &PolicyFile) {
	for src in sources {
		match src.as_str() {
			"autogroup:member" => set.all_members = true,
			// Tagged devices never reach the administrative surface, so a
			// tagged source contributes no login.
			"autogroup:tagged" => {}
			s if s.starts_with("autogroup:") => {
				tracing::warn!(source = %s, "canopy grant uses an unsupported autogroup; ignoring");
			}
			s if s.starts_with("group:") => match policy.groups.get(s) {
				Some(members) => {
					set.logins
						.extend(members.iter().map(|m| m.to_ascii_lowercase()));
				}
				None => {
					tracing::warn!(group = %s, "canopy grant references an undefined group; ignoring");
				}
			},
			s if s.contains('@') => {
				set.logins.insert(s.to_ascii_lowercase());
			}
			s => {
				tracing::warn!(source = %s, "canopy grant uses an unsupported source; ignoring");
			}
		}
	}
}

/// Resolve the permissions a policy file confers through `bes.au/cap/canopy`
/// grants targeting the Canopy service tag. Each grant's value is inspected for
/// the `admin` and `danger` keys independently, so one grant can confer either
/// or both to its sources.
fn resolve_permissions(policy: &PolicyFile) -> PermissionGrants {
	let mut out = PermissionGrants::default();
	for grant in &policy.grants {
		let Some(values) = grant.app.get(CANOPY_CAP) else {
			continue;
		};
		if !grant.dst.iter().any(|d| d == CANOPY_SERVICE_TAG) {
			continue;
		}
		if cap_confers(values, "admin") {
			extend_from_sources(&mut out.admin, &grant.src, policy);
		}
		if cap_confers(values, "danger") {
			extend_from_sources(&mut out.danger, &grant.src, policy);
		}
	}
	out
}

#[derive(Debug, Default)]
struct OAuthState {
	token: Option<OAuthToken>,
}

#[derive(Clone, Debug)]
struct OAuthToken {
	access_token: String,
	expires_at: Instant,
}

#[derive(Debug, Deserialize)]
struct OAuthResponse {
	access_token: String,
	#[serde(default)]
	expires_in: u64,
}

#[derive(Debug, Deserialize)]
struct DevicesResponse {
	devices: Vec<DeviceRecord>,
}

#[derive(Debug, Deserialize)]
struct DeviceRecord {
	#[serde(rename = "nodeId")]
	node_id: String,
	#[serde(default)]
	name: String,
	#[serde(default)]
	addresses: Vec<String>,
	#[serde(default)]
	tags: Vec<String>,
	#[serde(rename = "tailnetName", default)]
	tailnet_name: String,
	#[serde(rename = "lastSeen", default)]
	last_seen: Option<String>,
	#[serde(rename = "keyExpiryDisabled", default)]
	key_expiry_disabled: bool,
}

impl TailnetDirectory {
	/// Build a directory and kick off the background refresh loop. The
	/// first refresh is awaited synchronously so the directory has a
	/// populated cache before the server starts serving — operators who
	/// misconfigure the credentials get a clear startup error rather than
	/// per-request 503s.
	pub async fn new(config: TailnetDirectoryConfig) -> Result<Self> {
		let client = Client::builder()
			.timeout(Duration::from_secs(15))
			.build()
			.map_err(|e| AppError::custom(format!("building reqwest client: {e}")))?;

		let directory = Self {
			inner: Arc::new(Inner {
				client,
				config,
				cache: RwLock::default(),
				oauth: RwLock::default(),
			}),
		};

		directory.refresh().await?;
		// Non-fatal: without the policy read (e.g. the OAuth client lacks the
		// policy-file read scope) admin access falls back to the allowlist.
		if let Err(e) = directory.refresh_policy().await {
			tracing::warn!(error = %e, "initial tailnet policy read failed; admin access falls back to the allowlist");
		}

		let bg = directory.clone();
		tokio::spawn(async move {
			let period = bg.inner.config.refresh_period;
			loop {
				tokio::time::sleep(period).await;
				if let Err(e) = bg.refresh().await {
					tracing::warn!(error = %e, "tailnet directory refresh failed");
				}
				if let Err(e) = bg.refresh_policy().await {
					tracing::warn!(error = %e, "tailnet policy refresh failed");
				}
			}
		});

		Ok(directory)
	}

	/// True when the login is granted administrative access by the tailnet
	/// policy — either explicitly (a group member or named user) or because
	/// a conferring grant covers every tailnet member. Case-insensitive.
	pub async fn is_admin_by_policy(&self, login: &str) -> bool {
		let login = login.to_ascii_lowercase();
		let cache = self.inner.cache.read().await;
		cache.permissions.admin.covers(&login)
	}

	/// True when the login is granted the danger permission by the tailnet
	/// policy (see the ADM spec). Independent of administrator: a grant carrying
	/// only `danger` confers this and not administrator. Case-insensitive.
	pub async fn has_danger_by_policy(&self, login: &str) -> bool {
		let login = login.to_ascii_lowercase();
		let cache = self.inner.cache.read().await;
		cache.permissions.danger.covers(&login)
	}

	/// Lookup a tailnet IP. Returns `None` for unknown IPs. On a miss the
	/// directory will at most one refresh per `miss_cooldown` to pick up
	/// newly-joined nodes.
	pub async fn lookup(&self, ip: IpAddr) -> Result<Option<DirectoryEntry>> {
		{
			let cache = self.inner.cache.read().await;
			if let Some(entry) = cache.by_ip.get(&ip) {
				return Ok(Some(entry.clone()));
			}
			if let Some(last) = cache.last_refresh
				&& last.elapsed() < self.inner.config.miss_cooldown
			{
				return Ok(None);
			}
		}

		self.refresh().await?;

		let cache = self.inner.cache.read().await;
		Ok(cache.by_ip.get(&ip).cloned())
	}

	/// Reverse-lookup by stable node id. Same miss-cooldown discipline
	/// as [`Self::lookup`]. Used by the admin UI to surface the current
	/// tailnet IPs / display name of an already-attached device.
	pub async fn find_by_node_id(&self, node_id: &str) -> Result<Option<DirectoryEntry>> {
		{
			let cache = self.inner.cache.read().await;
			if let Some(entry) = cache.by_node_id.get(node_id) {
				return Ok(Some(entry.clone()));
			}
			if let Some(last) = cache.last_refresh
				&& last.elapsed() < self.inner.config.miss_cooldown
			{
				return Ok(None);
			}
		}

		self.refresh().await?;

		let cache = self.inner.cache.read().await;
		Ok(cache.by_node_id.get(node_id).cloned())
	}

	/// Find an entry by DNS name (exact match against `name`, or
	/// `name` with the tailnet suffix stripped). Linear scan over the
	/// cached entries — at typical fleet sizes that's well under a
	/// millisecond and the alternative is a third index to maintain.
	pub async fn find_by_name(&self, name: &str) -> Result<Option<DirectoryEntry>> {
		let matcher = name.trim().to_ascii_lowercase();
		let needle = matcher.trim_end_matches('.');

		let scan = |cache: &Cache| -> Option<DirectoryEntry> {
			for entry in cache.by_node_id.values() {
				let entry_name = entry.node_name.to_ascii_lowercase();
				let short = entry_name.split('.').next().unwrap_or("");
				if entry_name == needle || short == needle {
					return Some(entry.clone());
				}
			}
			None
		};

		{
			let cache = self.inner.cache.read().await;
			if let Some(hit) = scan(&cache) {
				return Ok(Some(hit));
			}
			if let Some(last) = cache.last_refresh
				&& last.elapsed() < self.inner.config.miss_cooldown
			{
				return Ok(None);
			}
		}

		self.refresh().await?;
		let cache = self.inner.cache.read().await;
		Ok(scan(&cache))
	}

	/// Resolve any of: a Tailscale IP, a node id, or a DNS name. Used
	/// by the admin attach-tailscale endpoint so an operator can paste
	/// whichever identifier they grabbed from the Tailscale admin
	/// console.
	pub async fn resolve_identifier(&self, raw: &str) -> Result<Option<DirectoryEntry>> {
		let trimmed = raw.trim();
		if trimmed.is_empty() {
			return Ok(None);
		}
		if let Ok(ip) = trimmed.parse::<IpAddr>() {
			return self.lookup(ip).await;
		}
		if let Some(hit) = self.find_by_node_id(trimmed).await? {
			return Ok(Some(hit));
		}
		self.find_by_name(trimmed).await
	}

	/// Snapshot of every cached node, keyed by `node_id`. Cheap clone
	/// of the inner map; the caller is free to iterate without holding
	/// the cache lock. Used by sweeps that need to check directory
	/// state against persisted devices.
	pub async fn snapshot_by_node_id(&self) -> HashMap<String, DirectoryEntry> {
		self.inner.cache.read().await.by_node_id.clone()
	}

	/// Force-refresh the cache from the Tailscale control plane.
	pub async fn refresh(&self) -> Result<()> {
		let token = self.access_token().await?;
		let url = format!(
			"{}/api/v2/tailnet/{}/devices",
			self.inner.config.api_base.trim_end_matches('/'),
			self.inner.config.tailnet,
		);
		let resp = self
			.inner
			.client
			.get(&url)
			.bearer_auth(&token)
			.send()
			.await
			.map_err(|e| AppError::custom(format!("tailscale devices request: {e}")))?;

		if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
			// Token expired between the time we minted it and now. Drop and
			// retry exactly once.
			self.inner.oauth.write().await.token = None;
			let token = self.access_token().await?;
			let retry = self
				.inner
				.client
				.get(&url)
				.bearer_auth(&token)
				.send()
				.await
				.map_err(|e| AppError::custom(format!("tailscale devices retry: {e}")))?;
			return self.ingest(retry).await;
		}

		self.ingest(resp).await
	}

	async fn ingest(&self, resp: reqwest::Response) -> Result<()> {
		if !resp.status().is_success() {
			let status = resp.status();
			let body = resp.text().await.unwrap_or_default();
			return Err(AppError::custom(format!(
				"tailscale devices: {status}: {body}"
			)));
		}
		let parsed: DevicesResponse = resp
			.json()
			.await
			.map_err(|e| AppError::custom(format!("decoding devices response: {e}")))?;

		let mut by_ip = HashMap::with_capacity(parsed.devices.len() * 2);
		let mut by_node_id = HashMap::with_capacity(parsed.devices.len());
		for d in parsed.devices {
			let addresses: Vec<IpAddr> = d
				.addresses
				.iter()
				.filter_map(|a| a.parse::<IpAddr>().ok())
				.collect();
			let last_seen = d.last_seen.as_deref().and_then(|s| s.parse().ok());
			let entry = DirectoryEntry {
				node_id: d.node_id.clone(),
				node_name: d.name,
				tailnet: d.tailnet_name,
				tags: d.tags,
				addresses: addresses.clone(),
				last_seen,
				key_expiry_disabled: d.key_expiry_disabled,
			};
			for ip in &addresses {
				by_ip.insert(*ip, entry.clone());
			}
			by_node_id.insert(d.node_id, entry);
		}

		let mut cache = self.inner.cache.write().await;
		cache.by_ip = by_ip;
		cache.by_node_id = by_node_id;
		cache.last_refresh = Some(Instant::now());
		Ok(())
	}

	/// Force-refresh the administrator set from the tailnet policy file.
	pub async fn refresh_policy(&self) -> Result<()> {
		let token = self.access_token().await?;
		let url = format!(
			"{}/api/v2/tailnet/{}/acl",
			self.inner.config.api_base.trim_end_matches('/'),
			self.inner.config.tailnet,
		);
		let resp = self
			.inner
			.client
			.get(&url)
			.bearer_auth(&token)
			.header(reqwest::header::ACCEPT, "application/json")
			.send()
			.await
			.map_err(|e| AppError::custom(format!("tailscale policy request: {e}")))?;

		if resp.status() == reqwest::StatusCode::UNAUTHORIZED {
			self.inner.oauth.write().await.token = None;
			let token = self.access_token().await?;
			let retry = self
				.inner
				.client
				.get(&url)
				.bearer_auth(&token)
				.header(reqwest::header::ACCEPT, "application/json")
				.send()
				.await
				.map_err(|e| AppError::custom(format!("tailscale policy retry: {e}")))?;
			return self.ingest_policy(retry).await;
		}

		self.ingest_policy(resp).await
	}

	async fn ingest_policy(&self, resp: reqwest::Response) -> Result<()> {
		if !resp.status().is_success() {
			let status = resp.status();
			let body = resp.text().await.unwrap_or_default();
			return Err(AppError::custom(format!(
				"tailscale policy: {status}: {body}"
			)));
		}
		let policy: PolicyFile = resp
			.json()
			.await
			.map_err(|e| AppError::custom(format!("decoding policy response: {e}")))?;

		let permissions = resolve_permissions(&policy);
		let mut cache = self.inner.cache.write().await;
		cache.permissions = permissions;
		Ok(())
	}

	async fn access_token(&self) -> Result<String> {
		{
			let state = self.inner.oauth.read().await;
			if let Some(tok) = &state.token
				&& tok.expires_at > Instant::now() + Duration::from_secs(30)
			{
				return Ok(tok.access_token.clone());
			}
		}

		let url = format!(
			"{}/api/v2/oauth/token",
			self.inner.config.api_base.trim_end_matches('/'),
		);
		let body = serde_urlencoded::to_string([
			("grant_type", "client_credentials"),
			("client_id", &self.inner.config.oauth_client_id),
			("client_secret", &self.inner.config.oauth_client_secret),
		])
		.map_err(|e| AppError::custom(format!("encoding oauth body: {e}")))?;

		let resp = self
			.inner
			.client
			.post(&url)
			.header("Content-Type", "application/x-www-form-urlencoded")
			.body(body)
			.send()
			.await
			.map_err(|e| AppError::custom(format!("tailscale oauth request: {e}")))?;

		if !resp.status().is_success() {
			let status = resp.status();
			let body = resp.text().await.unwrap_or_default();
			return Err(AppError::custom(format!(
				"tailscale oauth: {status}: {body}"
			)));
		}

		let parsed: OAuthResponse = resp
			.json()
			.await
			.map_err(|e| AppError::custom(format!("decoding oauth response: {e}")))?;

		let expires_at = Instant::now() + Duration::from_secs(parsed.expires_in.max(60));
		let token = OAuthToken {
			access_token: parsed.access_token.clone(),
			expires_at,
		};

		let mut state = self.inner.oauth.write().await;
		state.token = Some(token);
		Ok(parsed.access_token)
	}

	/// Construct a directory pre-populated with fixed entries, for tests.
	/// No background refresh, no API calls. Each entry is indexed both
	/// by the IP key passed in and by its `node_id`.
	pub fn for_test(entries: impl IntoIterator<Item = (IpAddr, DirectoryEntry)>) -> Self {
		let mut by_ip = HashMap::new();
		let mut by_node_id = HashMap::new();
		for (ip, entry) in entries {
			by_node_id.insert(entry.node_id.clone(), entry.clone());
			by_ip.insert(ip, entry);
		}
		let cache = Cache {
			by_ip,
			by_node_id,
			last_refresh: Some(Instant::now()),
			permissions: PermissionGrants::default(),
		};
		Self {
			inner: Arc::new(Inner {
				client: Client::new(),
				config: TailnetDirectoryConfig {
					oauth_client_id: String::new(),
					oauth_client_secret: String::new(),
					tailnet: String::new(),
					api_base: String::new(),
					refresh_period: Duration::from_secs(3600),
					miss_cooldown: Duration::from_secs(3600),
				},
				cache: RwLock::new(cache),
				oauth: RwLock::default(),
			}),
		}
	}
}

impl TailnetDirectory {
	/// Construct a directory that has read the given tailnet policy file, for
	/// tests. No devices, no background refresh, no API calls. Panics on a
	/// policy that does not parse, since that is a mistake in the test.
	pub fn for_test_with_policy(policy: serde_json::Value) -> Self {
		let policy: PolicyFile = serde_json::from_value(policy).expect("test policy parses");
		let directory = Self::for_test([]);
		directory
			.inner
			.cache
			.try_write()
			.expect("a fresh directory is uncontended")
			.permissions = resolve_permissions(&policy);
		directory
	}
}

/// True for any IP that's in Tailscale's CGNAT v4 range (100.64.0.0/10) or
/// its ULA v6 prefix (fd7a:115c:a1e0::/48). Used as the spoof-guard
/// for the dual-auth tailnet path: only consider X-Forwarded-For values
/// in these ranges as possibly identifying a tailnet caller.
pub fn is_tailnet_ip(ip: IpAddr) -> bool {
	match ip {
		IpAddr::V4(v4) => {
			let oct = v4.octets();
			oct[0] == 100 && (oct[1] & 0b1100_0000) == 0b0100_0000
		}
		IpAddr::V6(v6) => {
			let seg = v6.segments();
			seg[0] == 0xfd7a && seg[1] == 0x115c && seg[2] == 0xa1e0
		}
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::net::{Ipv4Addr, Ipv6Addr};

	#[test]
	fn cgnat_v4_is_tailnet() {
		assert!(is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
		assert!(is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(100, 127, 255, 254))));
		assert!(is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(100, 100, 50, 50))));
	}

	#[test]
	fn non_cgnat_v4_is_not_tailnet() {
		assert!(!is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(10, 0, 0, 1))));
		assert!(!is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 1))));
		assert!(!is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(100, 63, 255, 255))));
		assert!(!is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(100, 128, 0, 0))));
		assert!(!is_tailnet_ip(IpAddr::V4(Ipv4Addr::new(8, 8, 8, 8))));
	}

	#[test]
	fn tailscale_ula_is_tailnet() {
		assert!(is_tailnet_ip(IpAddr::V6(
			"fd7a:115c:a1e0::3701:2c8a".parse::<Ipv6Addr>().unwrap()
		)));
		assert!(is_tailnet_ip(IpAddr::V6(
			"fd7a:115c:a1e0:1:2:3:4:5".parse::<Ipv6Addr>().unwrap()
		)));
	}

	#[test]
	fn other_v6_is_not_tailnet() {
		assert!(!is_tailnet_ip(IpAddr::V6(
			"fd00:1234::1".parse::<Ipv6Addr>().unwrap()
		)));
		assert!(!is_tailnet_ip(IpAddr::V6(
			"::1".parse::<Ipv6Addr>().unwrap()
		)));
		assert!(!is_tailnet_ip(IpAddr::V6(
			"2001:db8::1".parse::<Ipv6Addr>().unwrap()
		)));
	}

	fn policy(json: serde_json::Value) -> PolicyFile {
		serde_json::from_value(json).unwrap()
	}

	#[test]
	fn group_source_resolves_to_members() {
		let perms = resolve_permissions(&policy(serde_json::json!({
			"groups": { "group:ops": ["Felix@BES.au", "sam@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:ops"],
			}],
		})));
		assert!(!perms.admin.all_members);
		assert!(perms.admin.logins.contains("felix@bes.au"));
		assert!(perms.admin.logins.contains("sam@bes.au"));
	}

	#[test]
	fn bare_user_and_autogroup_member_sources() {
		let perms = resolve_permissions(&policy(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["dana@bes.au", "autogroup:member"],
			}],
		})));
		assert!(perms.admin.all_members);
		assert!(perms.admin.logins.contains("dana@bes.au"));
	}

	#[test]
	fn grant_needs_the_service_tag_and_admin_true() {
		// Wrong dst: not conferring.
		let wrong_dst = resolve_permissions(&policy(serde_json::json!({
			"groups": { "group:ops": ["felix@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-other"],
				"src": ["group:ops"],
			}],
		})));
		assert!(wrong_dst.admin.logins.is_empty() && !wrong_dst.admin.all_members);

		// Payload without `admin: true`: not conferring admin.
		let no_admin = resolve_permissions(&policy(serde_json::json!({
			"groups": { "group:ops": ["felix@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": false }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:ops"],
			}],
		})));
		assert!(no_admin.admin.logins.is_empty() && !no_admin.admin.all_members);

		// Different capability: not conferring.
		let other_cap = resolve_permissions(&policy(serde_json::json!({
			"groups": { "group:ops": ["felix@bes.au"] },
			"grants": [{
				"app": { "bes.au/cap/other": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:ops"],
			}],
		})));
		assert!(other_cap.admin.logins.is_empty() && !other_cap.admin.all_members);
	}

	#[test]
	fn unsupported_sources_are_ignored() {
		let perms = resolve_permissions(&policy(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["autogroup:tagged", "tag:server-canopy", "*", "autogroup:admin"],
			}],
		})));
		assert!(perms.admin.logins.is_empty() && !perms.admin.all_members);
	}

	#[test]
	fn danger_and_admin_keys_resolve_independently() {
		// The danger key alone confers danger and not administrator.
		let danger_only = resolve_permissions(&policy(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "danger": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["dana@bes.au"],
			}],
		})));
		assert!(danger_only.danger.logins.contains("dana@bes.au"));
		assert!(danger_only.admin.logins.is_empty() && !danger_only.admin.all_members);

		// The admin key alone confers administrator and not danger.
		let admin_only = resolve_permissions(&policy(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["felix@bes.au"],
			}],
		})));
		assert!(admin_only.admin.logins.contains("felix@bes.au"));
		assert!(admin_only.danger.logins.is_empty() && !admin_only.danger.all_members);

		// A value carrying both keys confers both to its source.
		let both = resolve_permissions(&policy(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{ "admin": true, "danger": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["sam@bes.au"],
			}],
		})));
		assert!(both.admin.logins.contains("sam@bes.au"));
		assert!(both.danger.logins.contains("sam@bes.au"));

		// A value carrying neither key confers neither.
		let neither = resolve_permissions(&policy(serde_json::json!({
			"grants": [{
				"app": { "bes.au/cap/canopy": [{}] },
				"dst": ["tag:server-canopy"],
				"src": ["felix@bes.au"],
			}],
		})));
		assert!(neither.admin.logins.is_empty() && !neither.admin.all_members);
		assert!(neither.danger.logins.is_empty() && !neither.danger.all_members);
	}

	#[test]
	fn for_test_lookup() {
		let ip: IpAddr = "100.64.0.42".parse().unwrap();
		let entry = DirectoryEntry {
			node_id: "n1".into(),
			node_name: "alpha".into(),
			tailnet: "test".into(),
			tags: vec!["tag:server".into()],
			addresses: vec![ip],
			last_seen: None,
			key_expiry_disabled: true,
		};
		let dir = TailnetDirectory::for_test([(ip, entry.clone())]);
		let runtime = tokio::runtime::Runtime::new().unwrap();
		let resolved = runtime.block_on(dir.lookup(ip)).unwrap().unwrap();
		assert_eq!(resolved.node_id, "n1");
		assert_eq!(resolved.tags, vec!["tag:server".to_string()]);
		let missing: IpAddr = "100.64.0.99".parse().unwrap();
		assert!(runtime.block_on(dir.lookup(missing)).unwrap().is_none());
	}
}
