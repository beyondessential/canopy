//! Self-alerts: canopy reporting problems with its own operation.
//!
//! Spec: `.workhorse/specs/private-server/self-alerts.md` (id `SELF`).
//!
//! Each condition is one coalescing canopy-wide issue — scoped to
//! neither a server nor a group. Notification rides the incident
//! machinery: an error-or-worse condition opens an incident on the
//! canopy-wide target, with the same grace, escalation, and recovery
//! rules as any group incident (see
//! [`crate::issues::raise_global_event`]).

use commons_errors::Result;
use commons_types::acme::AuthorityFault;
use commons_types::dns::ManagedZone;
use commons_types::status::CheckResult;
use diesel_async::AsyncPgConnection;

use crate::issues::{CheckFiling, Issue, Scope, file_check, get_global_issue};

/// An operator-notification delivery permanently failed (the drainer gave up
/// on an outbox row). No automatic recovery: stays until operator-resolved.
pub const SLACK_DELIVERY_FAILURE_REF: &str = "slack-delivery-failure";

pub const SLACK_DELIVERY_FAILURE_DOC: &str = "## Description

The Slack outbox drainer gave up delivering a notification after exhausting its retries — operators may be missing incident notices.

## Results

- **fail** — at least one outbox row was abandoned; recovers when a later delivery succeeds.

## Solve

Check the abandoned row's last error and response in the slack_outbox table, and the webhook URLs in the drainer's configuration. Slack workflow-trigger changes are the usual cause.";

/// One or more catalogued checks have gone unreported across the whole
/// fleet for the stale-alert window. Coalescing: one alert lists them all.
/// Recovers when none remain — each having been reported again or
/// decommissioned.
pub const STALE_CHECKS_REF: &str = "stale-healthchecks";

pub const STALE_CHECKS_DOC: &str = "## Description

One or more catalogued healthchecks have not been reported by any server in the fleet for 30 days. A check that has gone away everywhere is usually a reporter that was retired or renamed; its stale state lingers until an operator decommissions it.

## Results

- **warn** — at least one check is unreported fleet-wide for 30 days; recovers when none remain (each reported again or decommissioned).

## Solve

Review the checks listed in the operator UI's healthcheck settings and decommission the ones that are gone for good; a decommissioned check's stale issues are cleared fleet-wide.";

/// A history has nearly run out of the weekly range it is written into.
/// Coalescing: one alert covers every short history. Recovers once all of
/// them are provisioned ahead again.
// spec: HST#running-short
pub const PARTITION_RUNWAY_REF: &str = "history-partition-runway";

pub const PARTITION_RUNWAY_DOC: &str = "## Description

Status and connection history are stored in weekly ranges, and a write only lands if a range covering its timestamp exists. Canopy provisions ranges ahead of itself while it runs, so this alert means that provisioning has stopped happening — and there is a deadline attached, because a history with no range left cannot be written at all.

What breaks at the deadline is writing, not reading: status pushes and connection records start failing while queries over existing history keep working.

## Results

- **warn** — less than two weeks of range remain.
- **fail** — less than one week remains.

Both recover on the next successful provisioning pass, which needs nothing but a working monitor pod and a database that accepts DDL.

## Solve

The monitor pod provisions ranges every minute, so a runway that keeps shrinking means those passes are failing: check its logs for the week it could not provision and why. A permissions change on the database role, or a lock it cannot get, are the usual causes. Provisioning by hand is `SELECT ensure_weekly_partitions('statuses', 4)` (and the same for `device_connections`), which is idempotent and safe to run at any time.";

/// Canopy's configured DNS zones don't cover the domains groups have been
/// given — either the configuration is unreadable, or a zone has left it while
/// claims inside it stand. Coalescing: one alert lists every affected group.
/// Recovers when every live group's claims sit in a configured zone again.
// spec: DOM#when-the-zone-configuration-changes
pub const DNS_ZONE_COVERAGE_REF: &str = "dns-zone-coverage";

pub const DNS_ZONE_COVERAGE_DOC: &str = "## Description

Canopy holds the DNS zones it may write records in as its own instance configuration, and a group domain is only actionable while a configured zone covers it. This alert means at least one group is now depending on a domain Canopy cannot reach — usually a zone removed from the configuration while its claims stood, or a configuration that no longer parses.

Claims are never dropped for this: the group keeps the domain, and it keeps excluding other groups from overlapping it. What stops is Canopy acting on any name beneath it.

## Results

- **warn** — some claims fall outside the configured zones while others are covered, which is what removing one zone of several looks like. Either restore the zone to the configuration or release the claims that no longer belong.
- **fail** — the configuration is unreadable, or there are no zones at all while domains stand claimed. Canopy can serve no DNS or TLS for any group until it is fixed.

## Solve

Compare the zone list in the Canopy instance's configuration against the domains reported in the alert message. Restore the missing zone if its removal was accidental; if it was deliberate, release the claims on each group's page. A configuration that does not parse is reported with the parse error — fix the entry and restart.";

/// Canopy cannot reach the certificate authority at all.
// spec: CRT#when-issuance-fails
pub const CA_UNREACHABLE_REF: &str = "certificate-authority-unreachable";

pub const CA_UNREACHABLE_DOC: &str = "## Description

Canopy could not reach the certificate authority it is configured to use. No certificate can be obtained or renewed while this stands, for any server in any group.

Requests already accepted are not lost: they stay pending and are worked when the authority comes back. What is at risk is anything whose renewal falls due in the meantime.

## Results

- **fail** — the authority did not answer. Recovers on the next successful conversation with it.

## Solve

Check the authority's own status page, then Canopy's egress: the domains pod needs outbound HTTPS to the directory URL it is configured with. A directory URL that is wrong rather than unreachable shows up here too, so check it against the authority's documentation.";

/// Canopy reached the authority but its account there is not usable.
// spec: CRT#when-issuance-fails
pub const CA_ACCOUNT_REF: &str = "certificate-authority-account";

pub const CA_ACCOUNT_DOC: &str = "## Description

Canopy reached the certificate authority, and the authority will not act on Canopy's account. Reported apart from being unreachable because the fix is different: this is a credential or a terms-of-service problem, not a network one.

No certificate can be obtained or renewed while this stands, for any server in any group.

## Results

- **fail** — the authority rejected Canopy's account, its key, or asked for an action to be taken on it. Recovers on the next successful conversation.

## Solve

Check the account key Canopy is configured with against the account registered at the authority, and whether the authority is asking for terms to be re-accepted or a contact to be verified. A key that does not match any account at the authority produces this, as does one that has been deactivated.";

/// The authority's rate limits are exhausted.
// spec: CRT#when-issuance-fails
pub const CA_THROTTLED_REF: &str = "certificate-authority-throttled";

pub const CA_THROTTLED_DOC: &str = "## Description

The certificate authority has told Canopy it is issuing too fast. Those limits are shared across every group whose domain sits in the same zone, so running them down is a fleet-wide fault rather than one group's — and retrying hard would spend what is left of the allowance on whichever name happened to fail last.

So Canopy stops working orders for a while rather than continuing to ask. Nothing is lost; issuance is slower until the allowance recovers.

## Results

- **fail** — the authority refused an order for rate limiting. Recovers once orders are going through again.

## Solve

Usually this resolves itself as the authority's window rolls forward. If it does not, look for a name being ordered repeatedly — a server asking for a certificate it cannot use, or a request loop — since one name failing over and over is the usual way an allowance is spent. The authority's documentation lists which limit applies.";

/// Raise or recover the three fleet-wide authority alerts from what the last
/// round of orders saw.
///
/// `fault` is the most serious thing the round hit, or `None` where the round
/// went through — which is also what recovers all three, since a certificate
/// obtained is proof the authority is reachable, the account works, and there is
/// allowance left. Passing `AuthorityFault::Order` recovers them too: an order
/// that failed on its own merits says the authority is fine.
///
/// The three are mutually exclusive by construction — one round reports one
/// condition — so raising any of them recovers the other two.
// spec: CRT#when-issuance-fails
pub async fn sweep_certificate_authority(
	conn: &mut AsyncPgConnection,
	fault: Option<AuthorityFault>,
	detail: Option<&str>,
) -> Result<()> {
	/// Ref, documentation, and headline for each fleet-wide authority condition.
	const CONDITIONS: [(AuthorityFault, &str, &str, &str); 3] = [
		(
			AuthorityFault::Unreachable,
			CA_UNREACHABLE_REF,
			CA_UNREACHABLE_DOC,
			"Certificate authority unreachable",
		),
		(
			AuthorityFault::Account,
			CA_ACCOUNT_REF,
			CA_ACCOUNT_DOC,
			"Certificate authority will not accept Canopy's account",
		),
		(
			AuthorityFault::Throttled,
			CA_THROTTLED_REF,
			CA_THROTTLED_DOC,
			"Certificate authority rate limits exhausted",
		),
	];

	for (condition, r#ref, doc, title) in CONDITIONS {
		if fault == Some(condition) {
			raise(
				conn,
				r#ref,
				CheckResult::Failed,
				CheckResult::Failed,
				// Escalating: nothing in the fleet can obtain a certificate, and the
				// people who can fix it are not the ones watching a group's incidents.
				true,
				Some(doc),
				title,
				detail.unwrap_or(title),
			)
			.await?;
		} else {
			recover(
				conn,
				r#ref,
				"the certificate authority is answering normally",
			)
			.await?;
		}
	}

	Ok(())
}

/// A server has been paused long enough that a certificate has gone stale
/// underneath the pause. Coalescing: one alert lists every server and name.
/// Recovers when every paused server's certificates are current again — which in
/// practice means the pause was lifted, since nothing renews under one.
// spec: CRT#pausing-a-server
pub const FORGOTTEN_PAUSE_REF: &str = "name-management-pause-forgotten";

pub const FORGOTTEN_PAUSE_DOC: &str = "## Description

Pausing a server stops Canopy doing anything new on its behalf, including renewing its certificates. That is the point of a pause — but it also suppresses the per-server alerting that would otherwise chase a certificate running out, so a pause nobody remembers is exactly how certificates quietly expire.

This alert is that forgetting, reported against Canopy rather than against the group: only an operator can lift a pause, and Canopy never lifts one itself however much is expiring underneath it.

A certificate for a name the server is no longer entitled to is not counted: Canopy stopped renewing that on purpose, and the pause is not why it is running out.

## Results

- **warn** — a paused server holds a certificate past the point Canopy would have renewed it. There is still room to recover by lifting the pause.
- **fail** — a paused server holds a certificate that has expired. Whatever it serves on that name is now being rejected by clients.

## Solve

Look at each server named in the alert. Finish whatever the pause was for — the recorded reason is on the server's page — and unpause it; Canopy then works the renewals that fell due while it was paused. If the pause is no longer needed at all, lifting it is the whole fix. If it is gone for good, archive the server or release the group's claim on the domain instead, which stops the certificates being Canopy's business.";

/// Evaluate the forgotten-pause condition and raise or recover the coalescing
/// [`FORGOTTEN_PAUSE_REF`] self-alert.
///
/// Severity splits on whether anything has actually run out: a certificate past
/// renewal under a pause is a nudge, and one that has expired is a fault, because
/// only the second means something has already stopped working.
// spec: CRT#pausing-a-server
pub async fn sweep_forgotten_pauses(conn: &mut AsyncPgConnection) -> Result<Option<Issue>> {
	let lapsing =
		crate::application_certificates::ApplicationCertificate::lapsing_under_pause(conn).await?;

	if lapsing.is_empty() {
		return recover(
			conn,
			FORGOTTEN_PAUSE_REF,
			"no paused server is holding a certificate that has gone stale",
		)
		.await;
	}

	let mut applications: Vec<&str> = lapsing.iter().map(|l| l.server_name.as_str()).collect();
	applications.sort_unstable();
	applications.dedup();

	let expired = lapsing.iter().filter(|l| l.expired).count();
	let listed: Vec<String> = lapsing
		.iter()
		.map(|lapse| {
			let state = if lapse.expired { "expired" } else { "overdue" };
			match (lapse.not_after, lapse.pause_reason.as_deref()) {
				(Some(at), Some(reason)) => {
					format!(
						"{} on {} ({state} {at}; paused: {reason})",
						lapse.name, lapse.server_name
					)
				}
				(Some(at), None) => {
					format!("{} on {} ({state} {at})", lapse.name, lapse.server_name)
				}
				(None, _) => format!("{} on {} ({state})", lapse.name, lapse.server_name),
			}
		})
		.collect();

	let (observed, message) = if expired > 0 {
		(
			CheckResult::Failed,
			format!(
				"{expired} certificate(s) have expired under a pause across {} server(s), and {} \
				 more are overdue for renewal{}",
				applications.len(),
				lapsing.len() - expired,
				list_suffix(&listed),
			),
		)
	} else {
		(
			CheckResult::Warning,
			format!(
				"{} certificate(s) are past renewal under a pause across {} server(s), and nothing \
				 renews while a server is paused{}",
				lapsing.len(),
				applications.len(),
				list_suffix(&listed),
			),
		)
	};

	// Ceiling at `fail` whatever this raise carries, so an overdue certificate
	// that later expires isn't capped at warning by the policy the first sighting
	// seeded.
	raise(
		conn,
		FORGOTTEN_PAUSE_REF,
		observed,
		CheckResult::Failed,
		false,
		Some(FORGOTTEN_PAUSE_DOC),
		"Certificates lapsing under a forgotten pause",
		&message,
	)
	.await
	.map(Some)
}

/// Evaluate the fleet-wide check-liveness condition and raise or recover
/// the coalescing [`STALE_CHECKS_REF`] self-alert. Runs after liveness is
/// reconciled so it reads fresh `last_seen`. Returns the affected issue,
/// if any.
pub async fn sweep_stale_healthchecks(conn: &mut AsyncPgConnection) -> Result<Option<Issue>> {
	use crate::check_policies::{CheckPolicy, STALE_ALERT_HOURS};

	let cutoff = jiff::Timestamp::now() - jiff::SignedDuration::from_hours(STALE_ALERT_HOURS);
	let quiet = CheckPolicy::gone_quiet(conn, cutoff).await?;
	if quiet.is_empty() {
		return recover(
			conn,
			STALE_CHECKS_REF,
			"all catalogued checks are reporting again or decommissioned",
		)
		.await;
	}

	// The qualified name, not the bare one. A check is identified by a
	// namespace and a name, so `alertd/db_version` names as many entries as
	// there are types reporting it — and an operator told to decommission one
	// of them cannot tell which from the message.
	let names: Vec<String> = quiet
		.iter()
		.map(|p| format!("{}/{}", p.source, p.qualified_name()))
		.collect();
	let message = format!(
		"{} healthcheck(s) unreported fleet-wide for 30 days: {}",
		quiet.len(),
		names.join(", "),
	);
	// The same set, structured, so the operator surface can link each one to
	// its own policy page — which is where the decommission this alert asks
	// for actually lives.
	let detail = serde_json::json!({
		"checks": quiet
			.iter()
			.map(|p| {
				let namespace = p.namespace().ok();
				serde_json::json!({
					"source": p.source,
					"check": p.check_name,
					"qualified_name": p.qualified_name(),
					"subject": namespace
						.as_ref()
						.and_then(|ns| ns.to_columns().0),
					"application_type": namespace
						.as_ref()
						.and_then(|ns| ns.to_columns().1),
				})
			})
			.collect::<Vec<_>>(),
	});
	raise_with_detail(
		conn,
		STALE_CHECKS_REF,
		CheckResult::Warning,
		CheckResult::Warning,
		false,
		Some(STALE_CHECKS_DOC),
		"Healthchecks gone quiet",
		&message,
		Some(detail),
	)
	.await
	.map(Some)
}

/// Evaluate the history-runway condition and raise or recover the coalescing
/// [`PARTITION_RUNWAY_REF`] self-alert.
///
/// Reads the runway rather than the outcome of provisioning: a pass that fails
/// is only a problem while it keeps failing, and the runway is what says whether
/// it has been failing long enough to matter. That also covers the case where no
/// pass is running at all.
// spec: HST#running-short
pub async fn sweep_partition_runway(conn: &mut AsyncPgConnection) -> Result<Option<Issue>> {
	use crate::partitions;

	let histories = partitions::runway(conn).await?;
	let short: Vec<&partitions::Runway> = histories.iter().filter(|r| r.short()).collect();

	if short.is_empty() {
		return recover(
			conn,
			PARTITION_RUNWAY_REF,
			"every history has weekly range provisioned ahead",
		)
		.await;
	}

	let listed = short
		.iter()
		.map(|r| match &r.covered_to {
			Some(covered_to) => format!(
				"{} covered to {covered_to} ({} day(s) left)",
				r.parent, r.days_remaining
			),
			None => format!("{} has no range at all", r.parent),
		})
		.collect::<Vec<_>>()
		.join(", ");
	let message = format!(
		"{} history(ies) short of weekly range: {listed}",
		short.len()
	);

	let observed = if short.iter().any(|r| r.critical()) {
		CheckResult::Failed
	} else {
		CheckResult::Warning
	};

	raise(
		conn,
		PARTITION_RUNWAY_REF,
		observed,
		CheckResult::Failed,
		true,
		Some(PARTITION_RUNWAY_DOC),
		"History storage running out of range",
		&message,
	)
	.await
	.map(Some)
}

/// Evaluate the DNS zone coverage condition and raise or recover the coalescing
/// [`DNS_ZONE_COVERAGE_REF`] self-alert.
///
/// `zones` is what Canopy managed to read from its configuration, and
/// `config_error` is why it read nothing when the configuration was present but
/// unparseable. The two carry different messages and different severity, because
/// one is a Canopy fault and the other is a tidy-up after a deliberate change.
///
/// A Canopy instance with no zones configured and nothing claimed is not a problem:
/// that is the feature simply not in use.
// spec: DOM#when-the-zone-configuration-changes
pub async fn sweep_dns_zone_coverage(
	conn: &mut AsyncPgConnection,
	zones: &[ManagedZone],
	config_error: Option<&str>,
) -> Result<Option<Issue>> {
	let unzoned = crate::server_domains::ServerGroupDomain::unzoned(conn, zones).await?;

	if unzoned.is_empty() && config_error.is_none() {
		return recover(
			conn,
			DNS_ZONE_COVERAGE_REF,
			"every claimed group domain sits within a configured DNS zone",
		)
		.await;
	}

	let mut groups: Vec<&str> = unzoned.iter().map(|d| d.group_name.as_str()).collect();
	groups.sort_unstable();
	groups.dedup();
	let listed: Vec<String> = unzoned
		.iter()
		.map(|d| format!("{} ({})", d.domain, d.group_name))
		.collect();

	let (observed, message) = if let Some(error) = config_error {
		(
			CheckResult::Failed,
			format!(
				"Canopy's managed DNS zone configuration could not be read ({error}), so it is \
				 acting as though it has no zones. {} claimed domain(s) across {} group(s) are \
				 unusable until it is fixed{}",
				unzoned.len(),
				groups.len(),
				list_suffix(&listed),
			),
		)
	} else if zones.is_empty() {
		(
			CheckResult::Failed,
			format!(
				"Canopy has no managed DNS zones configured, but {} domain(s) across {} group(s) \
				 stand claimed{}",
				unzoned.len(),
				groups.len(),
				list_suffix(&listed),
			),
		)
	} else {
		(
			CheckResult::Warning,
			format!(
				"{} claimed domain(s) across {} group(s) fall outside every configured DNS zone \
				 ({}){}",
				unzoned.len(),
				groups.len(),
				zones
					.iter()
					.map(|z| z.apex.as_str())
					.collect::<Vec<_>>()
					.join(", "),
				list_suffix(&listed),
			),
		)
	};

	// The ceiling registers as `fail` whatever severity this raise carries, so a
	// warning that later becomes a fault isn't capped at warning by the policy
	// the first sighting seeded.
	raise(
		conn,
		DNS_ZONE_COVERAGE_REF,
		observed,
		CheckResult::Failed,
		false,
		Some(DNS_ZONE_COVERAGE_DOC),
		"Group domains outside Canopy's DNS zones",
		&message,
	)
	.await
	.map(Some)
}

/// `": a, b, c"`, or nothing at all for an empty list — a broken configuration
/// with no claims yet has nothing to name.
fn list_suffix(listed: &[String]) -> String {
	if listed.is_empty() {
		String::new()
	} else {
		format!(": {}", listed.join(", "))
	}
}

/// Raise (or re-affirm) a self-alert: file the coalescing canopy-wide
/// check with the given observation, registering its catalog entry with
/// the policy the condition warrants (first sight only — operator edits
/// stick). The incident machinery handles notification — a fresh
/// effective failure opens (or joins) the canopy-wide incident, and
/// repeated raises while alerting change nothing Slack-side.
#[allow(clippy::too_many_arguments)]
pub async fn raise(
	conn: &mut AsyncPgConnection,
	r#ref: &str,
	observed: CheckResult,
	default_ceiling: CheckResult,
	default_escalates: bool,
	documentation: Option<&str>,
	title: &str,
	message: &str,
) -> Result<Issue> {
	raise_with_detail(
		conn,
		r#ref,
		observed,
		default_ceiling,
		default_escalates,
		documentation,
		title,
		message,
		None,
	)
	.await
}

/// [`raise`], with structured detail attached to the filing.
///
/// A message is for reading; detail is for acting on. An alert that names
/// things an operator has to go and find needs to hand the UI what those
/// things are, rather than making it parse them back out of a sentence.
#[expect(clippy::too_many_arguments)]
pub async fn raise_with_detail(
	conn: &mut AsyncPgConnection,
	r#ref: &str,
	observed: CheckResult,
	default_ceiling: CheckResult,
	default_escalates: bool,
	documentation: Option<&str>,
	title: &str,
	message: &str,
	detail: Option<serde_json::Value>,
) -> Result<Issue> {
	file_check(
		conn,
		CheckFiling {
			source: crate::statuses::CANOPY_SOURCE,
			scope: Scope::Global,
			device_id: None,
			check: r#ref,
			observed,
			title: Some(title),
			message,
			detail,
			default_ceiling,
			default_escalates,
			documentation,
		},
	)
	.await
}

/// Recover a self-alert. Writes nothing when the alert isn't active. On
/// the active → inactive transition the issue leaves the canopy-wide
/// incident, which closes (and notifies, or cancels a still-pending open)
/// once its last contributor is gone.
pub async fn recover(
	conn: &mut AsyncPgConnection,
	r#ref: &str,
	message: &str,
) -> Result<Option<Issue>> {
	let Some(existing) = current(conn, r#ref).await? else {
		return Ok(None);
	};
	if !existing.active {
		return Ok(None);
	}

	// The catalog entry exists (the active issue implies a prior raise
	// registered it), so the defaults here are inert.
	let issue = file_check(
		conn,
		CheckFiling {
			source: crate::statuses::CANOPY_SOURCE,
			scope: Scope::Global,
			device_id: None,
			check: r#ref,
			observed: CheckResult::Passed,
			title: None,
			message,
			detail: None,
			default_ceiling: CheckResult::Warning,
			default_escalates: false,
			documentation: None,
		},
	)
	.await?;
	Ok(Some(issue))
}

/// The one issue for this condition, if it has ever been raised.
pub async fn current(conn: &mut AsyncPgConnection, r#ref: &str) -> Result<Option<Issue>> {
	get_global_issue(conn, r#ref).await
}

/// All self-alert issues (the canopy-wide ones), newest activity first.
/// The operator UI's alerts view; the fleet issue listings exclude these.
pub async fn list(conn: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Issue>> {
	use crate::schema::issues::dsl;
	use diesel::prelude::*;
	use diesel_async::RunQueryDsl;

	dsl::issues
		.select(Issue::as_select())
		.filter(
			dsl::application_id
				.is_null()
				.and(dsl::machine_id.is_null())
				.and(dsl::server_group_id.is_null())
				.and(dsl::kubernetes_cluster_id.is_null()),
		)
		.order(dsl::last_seen.desc())
		.limit(limit)
		.load(conn)
		.await
		.map_err(commons_errors::AppError::from)
}
