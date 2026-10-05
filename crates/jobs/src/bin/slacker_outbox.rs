//! Slack outbox drainer.
//!
//! Phase A: posts to Slack Workflow Builder webhooks. One workflow (and one
//! webhook URL) per outbox kind, because Workflow Builder webhooks bind 1:1
//! to a workflow with a fixed variable set. The payload column in each
//! `slack_outbox` row is the flat JSON the workflow expects. An opening,
//! escalation, or reminder is re-rendered from the incident as it stands when
//! claimed, and the row is updated to what was posted; every other kind is
//! POSTed verbatim. Reminders post to the open workflow.
//!
//! Each loop iteration drains up to [`BATCH`] rows, one per transaction: a
//! row is claimed with `FOR UPDATE SKIP LOCKED` (the lock is what keeps
//! concurrent drainers off it), posted, and marked delivered or failed, then
//! that transaction commits before the next row is claimed. Per-row rather
//! than per-batch because the POST is irreversible — one transaction around
//! the whole batch meant a late DB error rolled back the `delivered_at` of
//! rows already posted, and the next tick posted them again.

use std::sync::Arc;
use std::sync::atomic::{AtomicI64, Ordering};
use std::time::Duration;

use clap::Parser;
use commons_types::status::CheckResult;
use database::slack_outbox::{
	KIND_INCIDENT_OPEN, KIND_INCIDENT_REMINDER, KIND_INCIDENT_RESOLVE, KIND_MAINTENANCE_DECLARED,
	KIND_MAINTENANCE_ENDED, KIND_SELF_ALERT_OPEN, KIND_SELF_ALERT_RESOLVE, SlackOutbox,
};
use diesel_async::AsyncConnection;
use lloggs::{LoggingArgs, PreArgs};
use miette::IntoDiagnostic;
use tokio::{
	task::{self, JoinHandle},
	time::sleep,
};
use tracing::{debug, error, info, warn};

const BATCH: i64 = 10;
const TICK: Duration = Duration::from_secs(5);
const MAX_ATTEMPTS: i32 = 10;
/// Cap on a single HTTP POST to Slack. Slack typically responds in well
/// under a second; anything past this is almost certainly a black-holed
/// destination and would otherwise wedge the whole drain loop.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(15);
/// How long the main loop is allowed to go without ticking before the
/// watchdog declares it deadlocked and exits the process. K8s restarts us
/// from there. Generously sized: TICK + worst-case batch (BATCH * REQUEST_TIMEOUT)
/// + slack.
const WATCHDOG_STALE_AFTER: Duration =
	Duration::from_secs(TICK.as_secs() + (BATCH as u64) * REQUEST_TIMEOUT.as_secs() + 30);
/// How often the watchdog wakes to compare the heartbeat to `now`.
const WATCHDOG_CHECK_EVERY: Duration = Duration::from_secs(30);

/// Drainer configuration. One webhook URL per outbox kind (each Slack
/// workflow binds to a single variable set declared in its trigger, so
/// `incident_open` and `incident_resolve` need separate workflows), plus
/// the `PRIVATE_URL` base that the `link` variable points at.
///
/// Validation policy: either **all** incident webhook URLs are set (along
/// with `PRIVATE_URL`), or **none** of them are (no-op mode for dev).
/// Partial configuration is a hard error at startup — silently dropping rows
/// for an unconfigured kind is exactly how we missed an entire month of
/// resolve notifications previously, so a noisy startup failure is preferred.
#[derive(Clone, Debug, Default)]
struct Config {
	open: Option<String>,
	resolve: Option<String>,
	/// Maintenance hooks are optional independently of the incident ones: a
	/// Canopy instance that hasn't built the workflows still records its windows
	/// and suspends on them, it just doesn't announce them.
	maintenance_declared: Option<String>,
	maintenance_ended: Option<String>,
	private_url: Option<String>,
}

/// Empty reads as unset. A Canopy instance that renders an unconfigured URL into
/// `value: ""` sets the variable rather than omitting it, and an empty URL
/// would have the drainer post to nowhere and retry the row forever.
fn present(value: Option<String>) -> Option<String> {
	value.filter(|v| !v.trim().is_empty())
}

impl Config {
	fn from_env() -> miette::Result<Self> {
		Self::build(
			std::env::var("SLACK_WEBHOOK_OPEN_URL").ok(),
			std::env::var("SLACK_WEBHOOK_RESOLVE_URL").ok(),
			std::env::var("SLACK_WEBHOOK_MAINTENANCE_DECLARED_URL").ok(),
			std::env::var("SLACK_WEBHOOK_MAINTENANCE_ENDED_URL").ok(),
			std::env::var("PRIVATE_URL").ok(),
		)
	}

	fn build(
		open: Option<String>,
		resolve: Option<String>,
		maintenance_declared: Option<String>,
		maintenance_ended: Option<String>,
		private_url: Option<String>,
	) -> miette::Result<Self> {
		let (open, resolve, private_url) = (present(open), present(resolve), present(private_url));
		let maintenance_declared = present(maintenance_declared);
		let maintenance_ended = present(maintenance_ended);
		let inputs: [(&str, &Option<String>); 2] = [
			("SLACK_WEBHOOK_OPEN_URL", &open),
			("SLACK_WEBHOOK_RESOLVE_URL", &resolve),
		];
		let set_count = inputs.iter().filter(|(_, v)| v.is_some()).count();
		if set_count == 0 {
			return Ok(Self::default());
		}
		if set_count != 0 && set_count != inputs.len() {
			let missing: Vec<&str> = inputs
				.iter()
				.filter(|(_, v)| v.is_none())
				.map(|(name, _)| *name)
				.collect();
			return Err(miette::miette!(
				"slack outbox drainer requires every incident SLACK_WEBHOOK_*_URL to \
				 be set when any is set (missing: {}). Set all of them, or leave them \
				 all unset for no-op mode.",
				missing.join(", ")
			));
		}
		let Some(private_url) = private_url else {
			return Err(miette::miette!(
				"PRIVATE_URL must be set when any SLACK_WEBHOOK_*_URL is set \
				 — it's the base of the operator-facing admin UI that the \
				 `link` variable in each Slack message points at",
			));
		};
		Ok(Self {
			open,
			resolve,
			maintenance_declared,
			maintenance_ended,
			private_url: Some(private_url),
		})
	}

	fn any_hook(&self) -> bool {
		self.open.is_some() || self.resolve.is_some()
	}

	/// `Ok(Some(url))` — post there; `Ok(None)` — known kind with no hook
	/// (mark delivered without posting); `Err` — unknown kind.
	fn url_for(&self, kind: &str) -> Result<Option<&str>, ()> {
		match kind {
			// A reminder is the incident's summary again, so it shares the
			// open workflow's variables and webhook.
			KIND_INCIDENT_OPEN | KIND_INCIDENT_REMINDER => Ok(self.open.as_deref()),
			KIND_INCIDENT_RESOLVE => Ok(self.resolve.as_deref()),
			KIND_MAINTENANCE_DECLARED => Ok(self.maintenance_declared.as_deref()),
			KIND_MAINTENANCE_ENDED => Ok(self.maintenance_ended.as_deref()),
			// Legacy: nothing enqueues self-alert rows anymore; stragglers
			// from before an upgrade drain as delivered without posting.
			KIND_SELF_ALERT_OPEN | KIND_SELF_ALERT_RESOLVE => Ok(None),
			_ => Err(()),
		}
	}

	/// The `link` variable for a row: the fleet's open maintenance windows
	/// for a maintenance row, its incident page for one carrying an
	/// incident, or the self-alerts view otherwise.
	fn link_for(&self, row: &SlackOutbox) -> Option<String> {
		let base = self.private_url.as_deref()?.trim_end_matches('/');
		Some(match (row.kind.as_str(), row.incident_id) {
			(KIND_MAINTENANCE_DECLARED | KIND_MAINTENANCE_ENDED, _) => {
				format!("{base}/maintenance")
			}
			(_, Some(incident_id)) => format!("{base}/incidents/{incident_id}"),
			(_, None) => format!("{base}/alerts"),
		})
	}
}

fn spawn(cfg: Config) -> JoinHandle<()> {
	let pool = database::init();
	if !cfg.any_hook() {
		info!("no SLACK_WEBHOOK_*_URL set; slack outbox drainer running in no-op mode");
	}
	let client = reqwest::Client::builder()
		.timeout(REQUEST_TIMEOUT)
		.build()
		.expect("build reqwest client");
	let heartbeat = Arc::new(AtomicI64::new(now_ms()));
	spawn_watchdog(heartbeat.clone());
	task::spawn(async move {
		loop {
			sleep(TICK).await;
			heartbeat.store(now_ms(), Ordering::Relaxed);
			let Ok(mut db) = pool.get().await else {
				error!("Failed to get database connection");
				continue;
			};

			// One transaction per row, not one per batch. The HTTP POST is
			// irreversible, so a row's `delivered_at` must commit on its own:
			// batching them meant a late DB error (or a failed commit) rolled
			// back the stamps of rows already posted, and the next tick posted
			// them again — duplicate incident pages from a single hiccup.
			//
			// The transaction still wraps claim-post-mark, because
			// `claim_pending` claims by row lock (`FOR UPDATE SKIP LOCKED`)
			// and that lock is what keeps concurrent drainers off the same
			// row. Claiming one row at a time keeps that exclusion while
			// bounding the blast radius of a failure to the row that failed.
			for _ in 0..BATCH {
				let result = db
					.transaction::<_, commons_errors::AppError, _>(async |conn| {
						drain_one(conn, &client, &cfg).await
					})
					.await;
				match result {
					// Queue drained (or every remaining row is another
					// drainer's); nothing to do until the next tick.
					Ok(false) => break,
					Ok(true) => {}
					Err(err) => {
						// This row is untouched — its claim lock is released
						// and it stays pending, so the next tick retries it.
						// Rows already delivered in this tick keep their
						// stamps.
						error!("slack outbox row failed: {err}");
						break;
					}
				}
			}
		}
	})
}

/// Claim, post, and mark one pending row. Returns `false` when there was no
/// row to claim. The caller wraps this in the transaction the claim's row lock
/// lives in.
async fn drain_one(
	conn: &mut diesel_async::AsyncPgConnection,
	client: &reqwest::Client,
	cfg: &Config,
) -> Result<bool, commons_errors::AppError> {
	let Some(mut row) = SlackOutbox::claim_pending(conn, 1).await?.pop() else {
		return Ok(false);
	};
	// spec: INC#notification
	let payload = row.render_current(conn).await?;
	if payload != row.payload {
		SlackOutbox::set_payload(conn, row.id, &payload).await?;
		row.payload = payload;
	}
	match deliver(client, cfg, &row).await {
		Ok(body) => {
			info!(
				id = %row.id,
				kind = %row.kind,
				incident_id = ?row.incident_id,
				response = %body,
				"slack delivered"
			);
			SlackOutbox::mark_delivered(conn, row.id, &body).await?;
		}
		Err(err) => {
			let next_attempts = row.attempts + 1;
			if next_attempts >= MAX_ATTEMPTS {
				error!(
					id = %row.id,
					kind = %row.kind,
					incident_id = ?row.incident_id,
					attempts = next_attempts,
					err = %err.msg,
					response = ?err.body,
					"slack delivery permanently failed; giving up"
				);
				SlackOutbox::mark_given_up(
					conn,
					row.id,
					&format!("giving up after {next_attempts} attempts: {}", err.msg),
				)
				.await?;
				// Surface the failure as a canopy-self
				// issue (nil-server) so it shows up in
				// the UI alongside everything else.
				// `enqueue_slack_*` skips nil-server
				// incidents to avoid feeding back into
				// the very loop that's failing.
				file_self_event(conn, &row, next_attempts, &err).await?;
			} else {
				warn!(
					id = %row.id,
					kind = %row.kind,
					incident_id = ?row.incident_id,
					attempts = next_attempts,
					retry_in = %database::slack_outbox::retry_backoff(
						next_attempts
					),
					err = %err.msg,
					response = ?err.body,
					"slack delivery failed; will retry"
				);
				SlackOutbox::mark_failed(conn, row.id, &err.msg, err.body.as_deref()).await?;
			}
		}
	}
	Ok(true)
}

/// Raise the `slack-delivery-failure` self-alert when the drainer abandons
/// a row. Coalesces into one issue, so a flapping drainer becomes a single
/// long-lived alert with rising event counts rather than many small ones.
/// This can't feed back into itself: the raise only enqueues a Slack row on
/// the not-alerting → alerting transition, so the follow-up failure of that
/// row re-raises an already-active alert and enqueues nothing.
async fn file_self_event(
	conn: &mut diesel_async::AsyncPgConnection,
	row: &SlackOutbox,
	attempts: i32,
	err: &DeliveryError,
) -> Result<(), commons_errors::AppError> {
	let target = match row.incident_id {
		Some(id) => Some(
			database::issues::Incident::get_by_id(conn, id)
				.await?
				.target_label(conn)
				.await?,
		),
		None => None,
	};
	database::self_alerts::raise(
		conn,
		database::self_alerts::SLACK_DELIVERY_FAILURE_REF,
		CheckResult::Failed,
		CheckResult::Failed,
		false,
		Some(database::self_alerts::SLACK_DELIVERY_FAILURE_DOC),
		&format!("Slack delivery permanently failed ({})", row.kind),
		&format!(
			"{} notification{}: gave up after {attempts} attempts. Last error: {}. Last response: {}",
			row.kind,
			target.map(|t| format!(" for {t}")).unwrap_or_default(),
			err.msg,
			err.body.as_deref().unwrap_or("<none>"),
		),
	)
	.await?;
	Ok(())
}

fn now_ms() -> i64 {
	std::time::SystemTime::now()
		.duration_since(std::time::UNIX_EPOCH)
		.map(|d| d.as_millis() as i64)
		.unwrap_or(0)
}

/// Watchdog: every [`WATCHDOG_CHECK_EVERY`] verify that the main loop has
/// ticked at least once within [`WATCHDOG_STALE_AFTER`]. If not, it's
/// almost certainly wedged (deadlocked DB pool, hung reqwest connection
/// past its timeout, runtime stuck). Log and exit so Kubernetes restarts
/// us — visible to operators via the pod restart count.
fn spawn_watchdog(heartbeat: Arc<AtomicI64>) -> JoinHandle<()> {
	task::spawn(async move {
		loop {
			sleep(WATCHDOG_CHECK_EVERY).await;
			let last = heartbeat.load(Ordering::Relaxed);
			let age = now_ms().saturating_sub(last);
			if age > WATCHDOG_STALE_AFTER.as_millis() as i64 {
				error!(
					stale_ms = age,
					threshold_ms = WATCHDOG_STALE_AFTER.as_millis() as i64,
					"slack outbox drainer heartbeat is stale; assuming deadlock and exiting"
				);
				std::process::exit(1);
			}
		}
	})
}

/// Returned by [`deliver`] on failure. Carries both the human-readable
/// error and (when there was one) the raw HTTP body Slack sent, so the
/// row can record both for postmortem use.
#[derive(Debug)]
struct DeliveryError {
	msg: String,
	body: Option<String>,
}

impl std::fmt::Display for DeliveryError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.msg)
	}
}

impl std::error::Error for DeliveryError {}

impl From<reqwest::Error> for DeliveryError {
	fn from(e: reqwest::Error) -> Self {
		Self {
			msg: e.to_string(),
			body: None,
		}
	}
}

/// Post the row's payload — augmented with a `link` derived from the row's
/// `incident_id` plus the configured `PRIVATE_URL` — to the workflow
/// webhook for this row's kind. Returns the raw HTTP response body so the
/// caller can stamp it on the row.
///
/// In no-op mode (no webhook URLs configured at all — dev) returns an
/// empty body without posting so the table doesn't grow unbounded. With
/// any hooks configured, an unknown / unconfigured kind is a hard error
/// instead of a silent drop — that silence is what made resolve-hook
/// misconfigurations invisible in production.
async fn deliver(
	client: &reqwest::Client,
	cfg: &Config,
	row: &SlackOutbox,
) -> Result<String, DeliveryError> {
	if !cfg.any_hook() {
		debug!(id = %row.id, kind = %row.kind, "no-op mode; row marked delivered without posting");
		return Ok(String::new());
	}
	let url = match cfg.url_for(&row.kind) {
		Err(()) => {
			return Err(DeliveryError {
				msg: format!("unknown outbox kind {:?}", row.kind),
				body: None,
			});
		}
		Ok(Some(url)) => url,
		// Legacy self-alert rows have no hook anymore: drain them as
		// delivered without posting. Incident kinds unset while any hook
		// is configured stay a hard error — that silence has bitten before.
		Ok(None) if row.kind == KIND_SELF_ALERT_OPEN || row.kind == KIND_SELF_ALERT_RESOLVE => {
			debug!(id = %row.id, kind = %row.kind, "legacy self-alert row; marked delivered without posting");
			return Ok(String::new());
		}
		// A Canopy instance with no maintenance workflow records its windows
		// without announcing them, rather than failing every row forever.
		Ok(None) if row.kind == KIND_MAINTENANCE_DECLARED || row.kind == KIND_MAINTENANCE_ENDED => {
			debug!(id = %row.id, kind = %row.kind, "no maintenance webhook configured; marked delivered without posting");
			return Ok(String::new());
		}
		Ok(None) => {
			return Err(DeliveryError {
				msg: format!("no webhook url configured for kind {:?}", row.kind),
				body: None,
			});
		}
	};
	let mut payload = row.payload.clone();
	if let Some(obj) = payload.as_object_mut()
		&& let Some(link) = cfg.link_for(row)
	{
		obj.insert("link".to_string(), serde_json::Value::String(link));
	}
	let resp = client.post(url).json(&payload).send().await?;
	let status = resp.status();
	let body = resp.text().await.unwrap_or_default();
	if status.is_success() {
		Ok(body)
	} else {
		Err(DeliveryError {
			msg: format!("slack returned {status}"),
			body: Some(body),
		})
	}
}

#[derive(Debug, Parser)]
struct Args {
	#[command(flatten)]
	logging: LoggingArgs,
}

#[cfg(test)]
mod tests {
	use super::*;
	use jiff::Timestamp;
	use serde_json::Value;
	use uuid::Uuid;

	fn row(kind: &str, payload: Value) -> SlackOutbox {
		SlackOutbox {
			id: Uuid::nil(),
			created_at: Timestamp::now(),
			kind: kind.into(),
			incident_id: Some(Uuid::nil()),
			issue_id: None,
			note_id: None,
			payload,
			delivered_at: None,
			attempts: 0,
			last_error: None,
			last_response: None,
			gave_up_at: None,
			deliver_after: Timestamp::now(),
		}
	}

	/// Spawns a tiny single-shot HTTP listener on a free port. The returned
	/// `(url, join, recorded)` lets a test point the drainer at it, await
	/// the request, and inspect the parsed JSON body. One std::net listener
	/// per test is enough — we don't need wiremock for this.
	fn one_shot_server() -> (
		String,
		std::thread::JoinHandle<()>,
		std::sync::Arc<std::sync::Mutex<Option<Value>>>,
	) {
		let recorded: std::sync::Arc<std::sync::Mutex<Option<Value>>> =
			std::sync::Arc::new(std::sync::Mutex::new(None));
		let recorded_clone = recorded.clone();
		let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
		let url = format!("http://{}/hook", listener.local_addr().unwrap());
		let join = std::thread::spawn(move || {
			let (mut stream, _) = listener.accept().unwrap();
			use std::io::{Read, Write};
			let mut buf = vec![0u8; 8192];
			let mut total = 0;
			loop {
				let n = stream.read(&mut buf[total..]).unwrap();
				if n == 0 {
					break;
				}
				total += n;
				if let Some(he) = buf[..total].windows(4).position(|w| w == b"\r\n\r\n") {
					let headers = std::str::from_utf8(&buf[..he]).unwrap();
					let len = headers
						.lines()
						.find_map(|l| {
							l.strip_prefix("content-length: ")
								.or_else(|| l.strip_prefix("Content-Length: "))
						})
						.unwrap_or("0")
						.parse::<usize>()
						.unwrap();
					if total - he - 4 >= len {
						let body = &buf[he + 4..he + 4 + len];
						let v: Value = serde_json::from_slice(body).unwrap();
						*recorded_clone.lock().unwrap() = Some(v);
						stream
							.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
							.unwrap();
						break;
					}
				}
			}
		});
		(url, join, recorded)
	}

	#[test]
	fn config_rejects_partial_webhook_set() {
		let err = Config::build(
			Some("http://example/open".into()),
			None,
			None,
			None,
			Some("https://canopy.test".into()),
		)
		.expect_err("must require every incident SLACK_WEBHOOK_*_URL when any is set");
		assert!(
			err.to_string().contains("SLACK_WEBHOOK_RESOLVE_URL"),
			"error names the missing var; got: {err}"
		);
	}

	#[test]
	fn config_rejects_missing_private_url_when_any_hook_set() {
		let err = Config::build(
			Some("http://example/open".into()),
			Some("http://example/resolve".into()),
			None,
			None,
			None,
		)
		.expect_err("must require PRIVATE_URL");
		assert!(err.to_string().contains("PRIVATE_URL"));
	}

	#[test]
	fn config_ok_when_no_hooks_set_even_without_private_url() {
		let cfg = Config::build(None, None, None, None, None).expect("no-op mode is fine");
		assert!(!cfg.any_hook());
	}

	#[test]
	fn empty_urls_read_as_unset() {
		let cfg = Config::build(
			Some(String::new()),
			Some(String::new()),
			Some("  ".into()),
			None,
			Some(String::new()),
		)
		.expect("empty is unset, so this is no-op mode rather than a partial config");
		assert!(!cfg.any_hook());
		assert_eq!(cfg.url_for(KIND_MAINTENANCE_DECLARED), Ok(None));
	}

	#[test]
	fn config_ok_when_all_set() {
		let cfg = Config::build(
			Some("http://example/open".into()),
			Some("http://example/resolve".into()),
			None,
			None,
			Some("https://canopy.test".into()),
		)
		.expect("complete config is fine");
		assert!(cfg.any_hook());
		assert_eq!(
			cfg.url_for(KIND_INCIDENT_OPEN),
			Ok(Some("http://example/open"))
		);
		assert_eq!(
			cfg.url_for(KIND_INCIDENT_RESOLVE),
			Ok(Some("http://example/resolve"))
		);
		assert_eq!(
			cfg.url_for(KIND_INCIDENT_REMINDER),
			Ok(Some("http://example/open")),
			"a reminder posts to the open workflow",
		);
		assert_eq!(cfg.url_for(KIND_MAINTENANCE_DECLARED), Ok(None));
		assert_eq!(cfg.url_for(KIND_MAINTENANCE_ENDED), Ok(None));
		assert_eq!(cfg.url_for(KIND_SELF_ALERT_OPEN), Ok(None));
		assert_eq!(cfg.url_for(KIND_SELF_ALERT_RESOLVE), Ok(None));
		assert_eq!(cfg.url_for("mystery"), Err(()));
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_with_no_hooks_is_a_noop() {
		let r = row(KIND_INCIDENT_OPEN, serde_json::json!({}));
		deliver(&reqwest::Client::new(), &Config::default(), &r)
			.await
			.expect("noop ok");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_injects_link_from_private_url() {
		let (url, server, recorded) = one_shot_server();
		let incident_id = Uuid::new_v4();
		let cfg = Config {
			open: Some(url),
			resolve: None,
			private_url: Some("https://canopy.example.ts.net".into()),
			..Default::default()
		};
		let mut r = row(
			KIND_INCIDENT_OPEN,
			serde_json::json!({
				"server": "Prod",
				"severity": "Error",
				"source_ref": "canopy/reachability",
				"message": "boom",
			}),
		);
		r.incident_id = Some(incident_id);
		let body = deliver(&reqwest::Client::new(), &cfg, &r)
			.await
			.expect("deliver ok");
		server.join().unwrap();

		assert_eq!(
			body, "ok",
			"deliver returns Slack's response body so the row can stamp it",
		);
		let got = recorded.lock().unwrap().clone().expect("got a request");
		assert_eq!(got["server"], "Prod");
		assert_eq!(got["severity"], "Error");
		assert_eq!(got["source_ref"], "canopy/reachability");
		assert_eq!(got["message"], "boom");
		assert_eq!(
			got["link"],
			format!("https://canopy.example.ts.net/incidents/{incident_id}"),
			"link injected at delivery time from PRIVATE_URL and row.incident_id",
		);
		assert!(got.get("blocks").is_none(), "no blocks wrapper");
		assert!(got.get("text").is_none(), "no text wrapper");
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_overrides_stale_link_in_payload() {
		// A row with a `link` already in its payload (e.g. from an older
		// drainer version) gets that link overwritten by the freshly
		// computed one.
		let (url, server, recorded) = one_shot_server();
		let incident_id = Uuid::new_v4();
		let cfg = Config {
			open: Some(url),
			resolve: None,
			private_url: Some("https://new.example/".into()),
			..Default::default()
		};
		let mut r = row(
			KIND_INCIDENT_OPEN,
			serde_json::json!({ "link": "https://stale.example/old" }),
		);
		r.incident_id = Some(incident_id);
		deliver(&reqwest::Client::new(), &cfg, &r)
			.await
			.expect("deliver ok");
		server.join().unwrap();

		let got = recorded.lock().unwrap().clone().unwrap();
		assert_eq!(
			got["link"],
			format!("https://new.example/incidents/{incident_id}")
		);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_routes_resolve_kind_to_resolve_url() {
		// Open URL points at a port nothing's listening on; if the drainer
		// accidentally routed by anything other than `kind`, the test would
		// fail by hanging or by hitting connection-refused.
		let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
		let resolve_url = format!("http://{}/hook", listener.local_addr().unwrap());
		let server = std::thread::spawn(move || {
			let (mut stream, _) = listener.accept().unwrap();
			use std::io::{Read, Write};
			let mut buf = [0u8; 4096];
			let _ = stream.read(&mut buf);
			stream
				.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\nok")
				.unwrap();
		});

		let cfg = Config {
			open: Some("http://127.0.0.1:1/open-should-not-be-hit".into()),
			resolve: Some(resolve_url),
			private_url: Some("https://canopy.test".into()),
			..Default::default()
		};
		let r = row(
			KIND_INCIDENT_RESOLVE,
			serde_json::json!({"server": "x", "by": "me"}),
		);
		deliver(&reqwest::Client::new(), &cfg, &r)
			.await
			.expect("deliver ok");
		server.join().unwrap();
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_returns_error_on_non_2xx() {
		let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
		let url = format!("http://{}/hook", listener.local_addr().unwrap());
		let server =
			std::thread::spawn(move || {
				let (mut stream, _) = listener.accept().unwrap();
				use std::io::{Read, Write};
				let mut buf = [0u8; 4096];
				let _ = stream.read(&mut buf);
				stream
				.write_all(b"HTTP/1.1 500 Internal Application Error\r\nContent-Length: 5\r\n\r\nnope!")
				.unwrap();
			});

		let cfg = Config {
			open: Some(url),
			resolve: None,
			private_url: Some("https://canopy.test".into()),
			..Default::default()
		};
		let r = row(KIND_INCIDENT_OPEN, serde_json::json!({}));
		let err = deliver(&reqwest::Client::new(), &cfg, &r)
			.await
			.expect_err("should error");
		server.join().unwrap();
		assert!(err.to_string().contains("500"), "error mentions status");
		assert_eq!(
			err.body.as_deref(),
			Some("nope!"),
			"failure body is captured for the row's last_response column",
		);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_errors_on_unknown_kind_when_hooks_configured() {
		// In configured (non-noop) mode, an unknown kind is a loud failure
		// rather than a silent drop — the silent path is what previously
		// hid missing resolve-hook configuration in production.
		let cfg = Config {
			open: Some("http://127.0.0.1:1/should-not-be-hit".into()),
			resolve: Some("http://127.0.0.1:1/should-not-be-hit".into()),
			private_url: Some("https://canopy.test".into()),
			..Default::default()
		};
		let r = row("bogus_kind", serde_json::json!({}));
		let err = deliver(&reqwest::Client::new(), &cfg, &r)
			.await
			.expect_err("unknown kind must error in configured mode");
		assert!(
			err.to_string().contains("unknown outbox kind"),
			"error explains why; got: {err}"
		);
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_legacy_self_alert_row_is_a_per_kind_noop() {
		// Nothing enqueues self-alert rows anymore, but stragglers from
		// before an upgrade must drain as delivered rather than erroring
		// forever.
		let cfg = Config {
			open: Some("http://127.0.0.1:1/should-not-be-hit".into()),
			resolve: Some("http://127.0.0.1:1/should-not-be-hit".into()),
			private_url: Some("https://canopy.test".into()),
			..Default::default()
		};
		let mut r = row(KIND_SELF_ALERT_OPEN, serde_json::json!({}));
		r.incident_id = None;
		let body = deliver(&reqwest::Client::new(), &cfg, &r)
			.await
			.expect("per-kind noop ok");
		assert_eq!(body, "");
	}

	async fn fail(conn: &mut diesel_async::AsyncPgConnection, application: Uuid, check: &str) {
		let stamp = database::issues::CheckStateStamp {
			check: check.into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instanced: None,
		};
		database::issues::NewEvent {
			source: "test".into(),
			r#ref: check.into(),
			description: None,
			message: format!("{check} is failing"),
			active: Some(true),
			occurred_at: None,
		}
		.save_with_state(conn, application, None, Some(&stamp), false)
		.await
		.expect("file check");
	}

	/// The opening posts the incident as it stands when the drainer claims
	/// it, including an issue that joined during the grace, and the row
	/// records what was posted.
	// spec: INC#notification
	#[tokio::test(flavor = "multi_thread")]
	async fn drain_posts_the_incident_as_it_stands_and_records_it() {
		use diesel::{QueryableByName, sql_query, sql_types};
		use diesel_async::RunQueryDsl;

		#[derive(QueryableByName)]
		struct RowId {
			#[diesel(sql_type = sql_types::Uuid)]
			id: Uuid,
		}

		commons_tests::db::TestDb::run(async |mut conn, _| {
			let application: RowId = sql_query(
				"WITH g AS (INSERT INTO server_groups (name) VALUES ('site') RETURNING id), \
				 m AS (INSERT INTO machines (name, group_id) SELECT 'box', id FROM g RETURNING id, group_id) \
				 INSERT INTO applications (type, name, group_id, machine_id) \
				 SELECT 'tamanu-central', 'central-1', m.group_id, m.id FROM m RETURNING id",
			)
			.get_result(&mut conn)
			.await
			.expect("application");
			fail(&mut conn, application.id, "app-down").await;
			fail(&mut conn, application.id, "db-down").await;
			let open: RowId = sql_query(
				"UPDATE slack_outbox SET deliver_after = NOW() WHERE kind = 'incident_open' \
				 RETURNING id",
			)
			.get_result(&mut conn)
			.await
			.expect("make the open due");

			let (url, server, recorded) = one_shot_server();
			let cfg = Config {
				open: Some(url),
				resolve: Some("http://127.0.0.1:1/should-not-be-hit".into()),
				private_url: Some("https://canopy.test".into()),
				..Default::default()
			};
			assert!(
				drain_one(&mut conn, &reqwest::Client::new(), &cfg)
					.await
					.expect("drain")
			);
			server.join().unwrap();

			let got = recorded.lock().unwrap().clone().expect("got a request");
			assert_eq!(got["server"], "site");
			assert_eq!(got["source_ref"], "2 failed");
			assert_eq!(
				got["message"],
				"• Failed: db-down on central-1\n• Failed: app-down on central-1"
			);

			let row: SlackOutbox = {
				use database::schema::slack_outbox::dsl;
				use diesel::{ExpressionMethods, QueryDsl, SelectableHelper};
				dsl::slack_outbox
					.select(SlackOutbox::as_select())
					.filter(dsl::id.eq(open.id))
					.first(&mut conn)
					.await
					.expect("row")
			};
			assert!(row.delivered_at.is_some());
			assert_eq!(
				row.payload["source_ref"], "2 failed",
				"the row records what was posted, not the snapshot it was queued with"
			);
		})
		.await
	}

	#[tokio::test(flavor = "multi_thread")]
	async fn deliver_noop_swallows_unknown_kind() {
		// In dev / no-op mode (no hooks configured) any kind is a no-op —
		// drainer-less environments shouldn't accumulate undelivered rows.
		let r = row("bogus_kind", serde_json::json!({}));
		deliver(&reqwest::Client::new(), &Config::default(), &r)
			.await
			.expect("noop ok");
	}
}

#[tokio::main]
async fn main() -> miette::Result<()> {
	let mut _guard = PreArgs::parse().setup()?;
	let args = Args::parse();
	if _guard.is_none() {
		_guard = Some(args.logging.setup(|v| match v {
			0 => "info",
			1 => "debug",
			_ => "trace",
		})?);
	}

	let cfg = Config::from_env()?;
	spawn(cfg).await.into_diagnostic()?;
	Ok(())
}
