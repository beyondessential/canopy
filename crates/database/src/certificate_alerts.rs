//! Reporting what has gone wrong with a server's own names and certificates.
//!
//! These are facts about a group rather than about Canopy, so each is filed
//! against the server like any other check: it joins that server's group's
//! incident and reaches the people who run it. Canopy's own inability to issue is
//! not any one server's fault and is reported against Canopy instead — see
//! [`crate::self_alerts`].
//!
//! Every sweep here closes as well as opens. A work list of what is wrong now is
//! not enough on its own: without asking what was wrong last time, a certificate
//! that renewed successfully would leave its alert standing forever.
// spec: CRT#when-issuance-fails

use commons_errors::Result;
use commons_types::status::CheckResult;
use diesel_async::AsyncPgConnection;
use uuid::Uuid;

use crate::application_certificates::{ApplicationCertificate, Risk};
use crate::application_names::ApplicationName;
use crate::issues::{CheckFiling, Issue, Scope, file_check};

/// The source Canopy files its own determinations under.
pub const CANOPY_SOURCE: &str = "canopy";

/// One check per server for every certificate of its that is running out.
pub const EXPIRY_REF: &str = "certificate-expiry";

pub const EXPIRY_DOC: &str = "## Description

A TLS certificate Canopy holds for this server is running out of life, and the renewal that should have replaced it has not happened. Canopy renews well before expiry and retries on its own, so this means renewal has been failing for a while — the name may no longer resolve to this server, or the DNS challenge may not be completing.

Both thresholds are fractions of the certificate's own lifetime rather than fixed durations, so a six-day certificate and a ninety-day one report at the same point in their lives.

A certificate for a name the server is no longer entitled to is not reported here at all: Canopy deliberately stopped renewing it, so its running out is the intended outcome.

## Results

- **warn** — past the point Canopy meant to renew, with room left to recover.
- **fail** — most of the renewal window is gone, or the certificate has expired outright.

## Solve

Check the server's certificates on its page in Canopy for the recorded error. Common causes: the group no longer controls the domain the name sits under, the server's TLS permission was withdrawn, the name's address records point elsewhere, or the certificate authority is refusing the order (which Canopy reports separately against itself).";

/// One check per server for a first issuance that keeps failing.
pub const ISSUANCE_REF: &str = "certificate-issuance";

pub const ISSUANCE_DOC: &str = "## Description

This server asked for a certificate and Canopy has never managed to obtain it. Told apart from a certificate about to expire on purpose: this is a server that never came up, not one about to go dark, and the two want different responses.

Canopy keeps retrying with a growing interval, so the order is not lost — but nothing will start serving TLS on that name until it succeeds.

## Results

- **fail** — an order has failed repeatedly without ever producing a certificate. Recovers when it succeeds, or when Canopy stops pursuing it.

## Solve

Read the recorded error on the server's page in Canopy. A first issuance usually fails because the name's DNS zone is not one Canopy manages, because the group's claim on the domain does not cover the name, or because the address records the challenge needs are not published yet.";

/// One check per server for address records Canopy could not publish.
pub const ADDRESS_REF: &str = "dns-records";

pub const ADDRESS_DOC: &str = "## Description

This server registered a public name with the addresses it is reachable at, and Canopy could not write those records into the DNS zone. The name will not resolve — or will keep resolving to wherever it pointed before — until the write succeeds.

Canopy retries every pass and keeps the server's stated intent, so nothing is lost; what is reported is that the zone has not caught up.

## Results

- **fail** — a registration's records have not been published and the last attempt failed. Recovers when the records are published.

## Solve

Read the recorded error on the server's page in Canopy. Usually either the zone the name sits under is not in Canopy's configuration, or Canopy's credentials for that zone have stopped working — the latter affects every name in the zone and is worth checking against the other applications in the group.";

/// File (or close) the per-server certificate-expiry check.
///
/// Coalescing per server: one check lists every certificate of that server's that
/// is running out, graded by the worst of them, because "this server's
/// certificates need attention" is one thing to act on rather than several.
// spec: CRT#when-issuance-fails
pub async fn sweep_certificate_expiry(db: &mut AsyncPgConnection) -> Result<usize> {
	let at_risk = ApplicationCertificate::at_risk(db).await?;
	let previously = Issue::active_server_ids_by_source_ref(db, CANOPY_SOURCE, EXPIRY_REF).await?;

	// Grouped in memory: the work list is small (a fleet's worth of certificates
	// in trouble at once), and grouping in SQL would mean re-deriving `risk()`,
	// which is deliberately Rust so the fractions live in one place.
	let mut by_server: std::collections::HashMap<Uuid, Vec<(ApplicationCertificate, Risk)>> =
		std::collections::HashMap::new();
	for (cert, risk) in at_risk {
		by_server
			.entry(cert.application_id)
			.or_default()
			.push((cert, risk));
	}

	let mut filed = 0;
	for (server_id, certificates) in &by_server {
		let mut certificates = certificates.clone();
		certificates.sort_by_key(|(cert, _)| cert.not_after);
		let worst = certificates
			.iter()
			.map(|(_, risk)| *risk)
			.max_by_key(|risk| match risk {
				Risk::Critical => 2,
				Risk::AtRisk => 1,
				Risk::None => 0,
			})
			.unwrap_or(Risk::None);

		let observed = match worst {
			Risk::Critical => CheckResult::Failed,
			Risk::AtRisk => CheckResult::Warning,
			Risk::None => continue,
		};

		let listed: Vec<String> = certificates
			.iter()
			.map(|(cert, _)| match cert.not_after {
				Some(at) => format!("{} (expires {at})", cert.name),
				None => cert.name.clone(),
			})
			.collect();
		let message = format!(
			"{} certificate(s) running out and not renewed: {}",
			listed.len(),
			listed.join(", "),
		);

		file_check(
			db,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(*server_id),
				device_id: None,
				check: EXPIRY_REF,
				observed,
				title: Some("TLS certificate running out"),
				message: &message,
				detail: Some(crate::check_detail! {
					"names": certificates.iter().map(|(c, _)| &c.name).collect::<Vec<_>>(),
					"expires_at": certificates
						.iter()
						.map(|(c, _)| c.not_after.map(|at| at.to_string()))
						.collect::<Vec<_>>(),
				}),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: Some(EXPIRY_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	// Whatever was reported last time and is healthy now: a renewal that came
	// through, a domain released, a server paused.
	for server_id in previously {
		if by_server.contains_key(&server_id) {
			continue;
		}
		file_check(
			db,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(server_id),
				device_id: None,
				check: EXPIRY_REF,
				observed: CheckResult::Passed,
				title: None,
				message: "every certificate Canopy holds for this server is current",
				detail: None,
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: Some(EXPIRY_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	Ok(filed)
}

/// How many failed attempts make a first issuance worth reporting rather than
/// worth waiting out. With the doubling backoff this is a bit over half an hour
/// of trying, which clears the transient causes — a challenge record not yet
/// visible, an authority briefly unavailable — without leaving a server that
/// never came up unreported for a shift.
pub const STUCK_AFTER_ATTEMPTS: i32 = 6;

/// File (or close) the per-server first-issuance check.
// spec: CRT#when-issuance-fails
pub async fn sweep_stuck_issuance(db: &mut AsyncPgConnection) -> Result<usize> {
	let stuck = ApplicationCertificate::stuck_first_issuances(db, STUCK_AFTER_ATTEMPTS).await?;
	let previously =
		Issue::active_server_ids_by_source_ref(db, CANOPY_SOURCE, ISSUANCE_REF).await?;

	let mut by_server: std::collections::HashMap<Uuid, Vec<ApplicationCertificate>> =
		std::collections::HashMap::new();
	for cert in stuck {
		by_server.entry(cert.application_id).or_default().push(cert);
	}

	let mut filed = 0;
	for (server_id, orders) in &by_server {
		let listed: Vec<String> = orders
			.iter()
			.map(|order| match order.last_error.as_deref() {
				Some(error) => format!("{} ({error})", order.name),
				None => order.name.clone(),
			})
			.collect();
		let message = format!(
			"{} certificate request(s) have never succeeded after {} or more attempts: {}",
			listed.len(),
			STUCK_AFTER_ATTEMPTS,
			listed.join("; "),
		);

		file_check(
			db,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(*server_id),
				device_id: None,
				check: ISSUANCE_REF,
				observed: CheckResult::Failed,
				title: Some("TLS certificate never obtained"),
				message: &message,
				detail: Some(crate::check_detail! {
					"names": orders.iter().map(|o| &o.name).collect::<Vec<_>>(),
					"attempts": orders.iter().map(|o| o.attempts).collect::<Vec<_>>(),
				}),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: Some(ISSUANCE_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	for server_id in previously {
		if by_server.contains_key(&server_id) {
			continue;
		}
		file_check(
			db,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(server_id),
				device_id: None,
				check: ISSUANCE_REF,
				observed: CheckResult::Passed,
				title: None,
				message: "no outstanding certificate request for this server is stuck",
				detail: None,
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: Some(ISSUANCE_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	Ok(filed)
}

/// File (or close) the per-server address-records check.
// spec: ADR#registering
pub async fn sweep_address_records(db: &mut AsyncPgConnection) -> Result<usize> {
	let failing = ApplicationName::failing_to_publish(db).await?;
	let previously = Issue::active_server_ids_by_source_ref(db, CANOPY_SOURCE, ADDRESS_REF).await?;

	let mut by_server: std::collections::HashMap<Uuid, Vec<ApplicationName>> =
		std::collections::HashMap::new();
	for row in failing {
		by_server.entry(row.application_id).or_default().push(row);
	}

	let mut filed = 0;
	for (server_id, names) in &by_server {
		let listed: Vec<String> = names
			.iter()
			.map(|row| match row.last_error.as_deref() {
				Some(error) => format!("{} ({error})", row.name),
				None => row.name.clone(),
			})
			.collect();
		let message = format!(
			"{} name(s) whose address records Canopy could not publish: {}",
			listed.len(),
			listed.join("; "),
		);

		file_check(
			db,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(*server_id),
				device_id: None,
				check: ADDRESS_REF,
				observed: CheckResult::Failed,
				title: Some("DNS records not published"),
				message: &message,
				detail: Some(crate::check_detail! {
					"names": names.iter().map(|n| &n.name).collect::<Vec<_>>(),
				}),
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: Some(ADDRESS_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	for server_id in previously {
		if by_server.contains_key(&server_id) {
			continue;
		}
		file_check(
			db,
			CheckFiling {
				source: CANOPY_SOURCE,
				scope: Scope::Application(server_id),
				device_id: None,
				check: ADDRESS_REF,
				observed: CheckResult::Passed,
				title: None,
				message: "every name this server registered is published",
				detail: None,
				default_ceiling: CheckResult::Failed,
				default_escalates: false,
				documentation: Some(ADDRESS_DOC),
			},
		)
		.await?;
		filed += 1;
	}

	Ok(filed)
}

/// Run every per-server name and certificate sweep, returning how many events
/// were filed. Grouped so the monitor pod has one call to make.
pub async fn sweep(db: &mut AsyncPgConnection) -> Result<usize> {
	let mut filed = sweep_certificate_expiry(db).await?;
	filed += sweep_stuck_issuance(db).await?;
	filed += sweep_address_records(db).await?;
	Ok(filed)
}
