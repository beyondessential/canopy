//! Thin wrapper around the generated `bes_canopy_api::CanopyClient`.
//!
//! Owns construction (the tailnet transport, reached through the Tailscale
//! sidecar's SOCKS5 proxy) and exposes the endpoints pgro consumes:
//! `restore_capabilities`, `restore_worklist`, `restore_credentials`,
//! `restore_verification`, and artifact registration. Each is a one-line
//! forward except the last, which the generated client cannot express; the
//! wrapper exists as the integration seam tests inject a stub at, and as the
//! place to hang pgro-specific logging / retry / cache concerns later.
//!
//! Spec: `.workhorse/specs/restore-operator/overview.md` (id `PGRO`).

use bes_canopy_api::{
	CanopyClient, CanopyTransport,
	bytes::Bytes,
	http::{Method, Request, header::CONTENT_TYPE},
	schema::{
		BackupPurpose, IntentDescriptor, ProgressArgs, RestoreCapabilitiesArgs, RestoreCredentials,
		RestoreCredentialsArgs, VerificationArgs, WorklistEntry,
	},
};
use reqwest::Url;
use uuid::Uuid;

use crate::error::{Error, Result};

mod transport;

pub use transport::{TailnetEndpoint, TailnetTransport};

/// One cumulative progress sample from a restore run in flight, as the
/// canopy-proxy sidecar POSTs it to the operator.
///
/// The sidecar measures S3 traffic and nothing else, so this carries only the
/// four byte counters plus what canopy needs to identify the run. Shared with
/// `bin/canopy_proxy` rather than duplicated, so the wire shape can't drift.
///
/// Counters are cumulative from the start of the run, never interval deltas:
/// canopy's contract makes a dropped or repeated sample cost resolution rather
/// than the accuracy of a total.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ProgressSample {
	pub run_id: Uuid,
	#[serde(rename = "type")]
	pub type_: String,
	pub sent_raw_bytes: u64,
	pub sent_payload_bytes: u64,
	pub received_raw_bytes: u64,
	pub received_payload_bytes: u64,
}

impl ProgressSample {
	/// Build the canopy request body.
	///
	/// Every counter pgro doesn't measure is left unset rather than sent as
	/// zero — canopy stores what it's given, and a zero would read as "hashed
	/// nothing" rather than "doesn't report hashing".
	pub fn to_args(&self) -> ProgressArgs {
		let to_i64 = |n: u64| i64::try_from(n).unwrap_or(i64::MAX);
		ProgressArgs::builder()
			.run_id(self.run_id)
			.type_(self.type_.clone())
			.purpose(BackupPurpose::Restore)
			.s3_sent_raw_bytes(to_i64(self.sent_raw_bytes))
			.s3_sent_payload_bytes(to_i64(self.sent_payload_bytes))
			.s3_received_raw_bytes(to_i64(self.received_raw_bytes))
			.s3_received_payload_bytes(to_i64(self.received_payload_bytes))
			// bestool rides the verbatim kopia status line along here; pgro's
			// sidecar sees only S3 traffic, so there is nothing to add.
			.extra(serde_json::Map::new())
			.build()
	}
}

/// Default SOCKS5 proxy the operator's Tailscale sidecar listens on (IPv6
/// loopback). Override via `CANOPY_SOCKS5_PROXY` for tests / non-sidecar
/// dev setups; set to empty to disable proxy entirely.
pub const DEFAULT_SOCKS5_PROXY: &str = "socks5://[::1]:1055";

/// Configuration for the canopy client at startup.
#[derive(Debug, Clone)]
pub struct CanopyConfig {
	/// Canopy's tailnet URL, the only way pgro reaches it.
	pub tailnet_url: Url,
	/// SOCKS5 URL of the Tailscale sidecar's userspace proxy. Empty string
	/// means no proxy.
	pub socks5_proxy: String,
	/// A tailnet address to connect to in place of resolving the URL's host,
	/// for where the proxy cannot resolve it.
	pub tailnet_addr: Option<std::net::IpAddr>,
}

/// pgro's canopy client wrapper around the generated `CanopyClient`.
pub struct Client {
	inner: CanopyClient<TailnetTransport>,
}

impl std::fmt::Debug for Client {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.debug_struct("canopy::Client").finish_non_exhaustive()
	}
}

impl Client {
	/// Build a client for the configured tailnet endpoint. Nothing is probed:
	/// a call that cannot reach Canopy fails when it is made.
	pub fn from_config(cfg: CanopyConfig) -> anyhow::Result<Self> {
		let socks5_proxy = match cfg.socks5_proxy.trim() {
			"" => None,
			proxy => Some(
				proxy
					.parse()
					.map_err(|err| anyhow::anyhow!("SOCKS5 proxy URL {proxy:?}: {err}"))?,
			),
		};
		let transport = TailnetTransport::new(TailnetEndpoint {
			url: cfg.tailnet_url,
			socks5_proxy,
			resolve_to: cfg.tailnet_addr,
		})?;
		Ok(Self {
			inner: CanopyClient::new(transport),
		})
	}

	/// Register the intent descriptors this consumer supports. Replaces the
	/// registered set wholesale (per canopy's semantics). Each descriptor
	/// carries the intent name, the canopy semantics it opts into, and its
	/// typed parameter schema.
	pub async fn restore_capabilities(&self, intents: &[IntentDescriptor]) -> Result<()> {
		let body = RestoreCapabilitiesArgs::builder()
			.intents(intents.to_vec())
			.build();
		self.inner
			.restore_capabilities(&body)
			.await
			.map_err(|err| Error::Canopy(format!("restore_capabilities: {err}")))
	}

	/// Fetch the consumer's desired-state worklist. Each entry is one
	/// concrete replica to maintain.
	pub async fn worklist(&self) -> Result<Vec<WorklistEntry>> {
		self.inner
			.restore_worklist()
			.await
			.map_err(|err| Error::Canopy(format!("restore_worklist: {err}")))
	}

	/// Fetch short-lived read-only STS creds plus the repo password for a
	/// `(group, type)`. Authorized iff a declaration covers it. `run_id` is the
	/// canopy run-uuid when the fetch belongs to a restore run (the restore
	/// job's sidecar); `None` for non-run fetches such as the reconcile-time
	/// repo-password poll.
	pub async fn restore_credentials(
		&self,
		backup_type: &str,
		group: Uuid,
		run_id: Option<Uuid>,
	) -> Result<RestoreCredentials> {
		let body = RestoreCredentialsArgs::builder()
			.group(group)
			.type_(backup_type.to_string())
			.maybe_run_id(run_id)
			.build();
		self.inner.restore_credentials(&body).await.map_err(|err| {
			Error::Canopy(format!(
				"restore_credentials({backup_type}, {group}): {err}"
			))
		})
	}

	/// Report a cumulative progress sample for a run still in flight, so canopy
	/// can show a long restore advancing rather than a figureless in-progress
	/// row. Best-effort telemetry: callers log failures and carry on.
	pub async fn backup_progress(&self, args: &ProgressArgs) -> Result<()> {
		self.inner
			.backup_progress(args)
			.await
			.map_err(|err| Error::Canopy(format!("backup_progress: {err}")))
	}

	/// Report a restore outcome (signal 3, restore-verification). `args` is
	/// the typed [`bes_canopy_api::schema::VerificationArgs`], including the
	/// free-form `health_details`.
	pub async fn restore_verification_typed(&self, args: &VerificationArgs) -> Result<()> {
		self.inner
			.restore_verification(args)
			.await
			.map_err(|err| Error::Canopy(format!("restore_verification: {err}")))
	}

	/// Register a built reporting schema as an artifact of `version`, scoped to
	/// `group`, sending the SQL over the connection pgro already holds.
	///
	/// Canopy issues no credential to any store for this, so being authorised to
	/// register for the group is the whole of what publishing into it takes.
	///
	/// This goes through the transport rather than a generated method, because
	/// the generator emits path parameters only and JSON bodies only, and this
	/// endpoint takes a query parameter and the artifact's bytes.
	///
	/// Answers the id canopy filed the artifact under, where the answer carries
	/// one: a build reports the artifacts it registered, and this is the only
	/// place their ids exist.
	pub async fn register_reporting_schema(
		&self,
		version: &str,
		group: Uuid,
		run_id: Option<Uuid>,
		sql: Bytes,
	) -> Result<Option<Uuid>> {
		if !is_path_safe_version(version) {
			return Err(Error::Canopy(format!(
				"{version:?} is not a version a schema can be registered against"
			)));
		}

		let uri = registration_uri(version, group, run_id);

		let request = Request::builder()
			.method(Method::POST)
			.uri(&uri)
			.header(CONTENT_TYPE, "application/sql")
			.body(sql)
			.map_err(|err| Error::Canopy(format!("building artifact registration: {err}")))?;

		let response =
			self.inner.transport().call(request).await.map_err(|err| {
				Error::Canopy(format!("register_reporting_schema({version}): {err}"))
			})?;

		if !response.status().is_success() {
			return Err(Error::Canopy(format!(
				"register_reporting_schema({version}): canopy answered {}",
				response.status()
			)));
		}

		Ok(registered_artifact_id(response.body()))
	}
}

/// Where a reporting schema is registered.
///
/// Canopy authorises this exact path, and a build whose registration misses it
/// reports a healthy restore and publishes nothing, so the drift is silent.
fn registration_uri(version: &str, group: Uuid, run_id: Option<Uuid>) -> String {
	let mut uri = format!("/artifacts/groups/{group}/{version}/reporting-schema/any");
	if let Some(run) = run_id {
		uri.push_str(&format!("?run={run}"));
	}
	uri
}

/// The artifact id out of a registration's answer.
///
/// A registration canopy took is not undone by an answer this cannot read, so
/// an unreadable body costs the report its id rather than failing the build.
fn registered_artifact_id(body: &Bytes) -> Option<Uuid> {
	#[derive(serde::Deserialize)]
	struct Registered {
		id: Uuid,
	}

	serde_json::from_slice::<Registered>(body)
		.ok()
		.map(|registered| registered.id)
}

/// Whether a version is safe to place in a request path.
///
/// `migrate_to.version` is free-form text from the worklist entry or the CRD,
/// and the path is what canopy authorises: a value carrying `?`, `#`, `&` or a
/// slash rewrites the target, so the group the schema publishes under stops
/// being the one this replica is authorised for. A version has to open with an
/// alphanumeric for the same reason, since a dot segment is resolved away by
/// any proxy or router between here and canopy.
fn is_path_safe_version(version: &str) -> bool {
	version.starts_with(|c: char| c.is_ascii_alphanumeric())
		&& version
			.chars()
			.all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '+' | '-'))
}

#[cfg(test)]
mod tests {
	use super::*;

	/// The path is what canopy authorises, and `migrate_to.version` is
	/// free-form text from the worklist or the CRD. A value carrying `?`, `&`,
	/// a slash, or a leading dot segment rewrites the target, so the group a
	/// schema publishes under stops being the one this replica is authorised
	/// for.
	#[test]
	fn a_version_that_would_rewrite_the_path_is_not_a_version() {
		for version in ["2.60.0", "2.60.0-rc1", "2.60.0+build.4"] {
			assert!(is_path_safe_version(version), "{version}");
		}
		for version in [
			"",
			"2.60.0?group=00000000-0000-0000-0000-000000000000&x=",
			"../../devices",
			"..",
			".",
			".2.60.0",
			"-2.60.0",
			"2.60.0/any",
			"2.60.0#frag",
		] {
			assert!(!is_path_safe_version(version), "{version}");
		}
	}

	fn sample() -> ProgressSample {
		ProgressSample {
			run_id: "00000000-0000-0000-0000-000000000000".parse().unwrap(),
			type_: "tamanu-postgres".to_string(),
			sent_raw_bytes: 2_000_000,
			sent_payload_bytes: 1_950_000,
			received_raw_bytes: 500_000_000,
			received_payload_bytes: 480_000_000,
		}
	}

	#[test]
	fn sample_maps_onto_progress_args() {
		let args = sample().to_args();

		assert_eq!(args.purpose, Some(BackupPurpose::Restore));
		assert_eq!(args.type_, "tamanu-postgres");
		assert_eq!(args.s3_sent_raw_bytes, Some(2_000_000));
		assert_eq!(args.s3_sent_payload_bytes, Some(1_950_000));
		assert_eq!(args.s3_received_raw_bytes, Some(500_000_000));
		assert_eq!(args.s3_received_payload_bytes, Some(480_000_000));
	}

	/// Canopy's contract is explicit that an unmeasured counter must be left
	/// out rather than sent as zero — a zero is a measurement, and would show
	/// in canopy as a run that has hashed nothing rather than one that doesn't
	/// report hashing at all. pgro measures none of the engine counters.
	#[test]
	fn unmeasured_counters_are_omitted_not_zeroed() {
		let args = sample().to_args();

		assert_eq!(args.bytes_hashed, None);
		assert_eq!(args.bytes_uploaded, None);
		assert_eq!(args.bytes_cached, None);
		assert_eq!(args.bytes_estimated, None);
		assert_eq!(args.bytes_read, None);
		assert_eq!(args.files_done, None);
		assert_eq!(args.files_estimated, None);
		assert_eq!(args.errors, None);
		// A restore has no freeze moment to report.
		assert_eq!(args.snapshot_taken_at, None);
	}

	const GROUP: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

	/// Canopy authorises `/artifacts/groups/{group}/{version}/reporting-schema/any`
	/// and nothing else. A build that posts anywhere near it publishes nothing
	/// while still reporting a healthy restore, so the drift never surfaces as
	/// a failure and this literal is the only thing holding it.
	#[test]
	fn a_schema_is_registered_at_the_path_canopy_authorises() {
		let uri = registration_uri("2.60.0", GROUP.parse().unwrap(), None);

		assert_eq!(
			uri,
			format!("/artifacts/groups/{GROUP}/2.60.0/reporting-schema/any")
		);
	}

	/// The exact version, never a range: a schema follows the migrations one
	/// version applies, so one built against a patch is not the schema another
	/// patch of the same minor describes.
	#[test]
	fn the_registration_names_an_exact_version() {
		let uri = registration_uri("2.60.2", GROUP.parse().unwrap(), None);

		assert!(uri.contains("/2.60.2/reporting-schema/"), "{uri}");
		assert!(
			!uri.contains(".x"),
			"a range would publish the wrong schema: {uri}"
		);
	}

	/// The run correlates the artifact with the build that made it, and rides
	/// the query beside the group the path names.
	#[test]
	fn a_run_is_carried_beside_the_group() {
		let run = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";
		let uri = registration_uri("2.60.0", GROUP.parse().unwrap(), Some(run.parse().unwrap()));

		assert_eq!(
			uri,
			format!("/artifacts/groups/{GROUP}/2.60.0/reporting-schema/any?run={run}")
		);
	}

	/// The build reports the artifacts it registered, and canopy's answer to the
	/// registration is the only place their ids appear.
	#[test]
	fn a_registration_answers_the_id_canopy_filed_it_under() {
		let body = Bytes::from_static(
			br#"{"id":"cccccccc-cccc-cccc-cccc-cccccccccccc","artifact_type":"reporting-schema"}"#,
		);

		assert_eq!(
			registered_artifact_id(&body),
			Some("cccccccc-cccc-cccc-cccc-cccccccccccc".parse().unwrap())
		);
	}

	/// Canopy has taken the schema in by the time the answer is read, so an
	/// answer this cannot parse costs the report an id rather than turning a
	/// registration that worked into a failed build.
	#[test]
	fn an_unreadable_answer_is_not_a_failed_registration() {
		assert_eq!(registered_artifact_id(&Bytes::from_static(b"")), None);
		assert_eq!(registered_artifact_id(&Bytes::from_static(b"{}")), None);
		assert_eq!(
			registered_artifact_id(&Bytes::from_static(br#"{"id":"not-a-uuid"}"#)),
			None
		);
	}

	#[test]
	fn default_socks5_proxy_is_v6_loopback() {
		assert_eq!(DEFAULT_SOCKS5_PROXY, "socks5://[::1]:1055");
	}

	#[test]
	fn default_socks5_proxy_parses_as_reqwest_proxy() {
		// Catches a typo in the constant — reqwest insists on a real URL.
		reqwest::Proxy::all(DEFAULT_SOCKS5_PROXY).expect("DEFAULT_SOCKS5_PROXY must parse");
	}
}
