//! Canopy domains pod: everything Canopy does inside the DNS zones it manages.
//!
//! Two jobs, one pod, because both need the same thing nothing else in Canopy
//! has — write access to the zones:
//!
//! - **addresses** — publish the A/AAAA records for the names applications have
//!   registered, and take them down when a name is withdrawn.
//! - **certificates** — drive orders at the certificate authority: publish the
//!   challenge record that proves control of a name, hand over the server's
//!   signing request, store the chain, and renew before it runs out.
//!
//! Neither is on a request path. A server asks for a certificate and collects it
//! later (see CRT), so the work here is allowed to take the minutes a DNS-01
//! challenge takes, and the pod being briefly down delays issuance rather than
//! failing it.
//!
//! Configuration decides how much of this runs. Without `CANOPY_DNS_ZONES` there
//! is nothing to write into and both sweeps idle; without
//! `CANOPY_ACME_ACCOUNT_KEY` there is no account to order with and the
//! certificate sweeps idle while addresses carry on. Either is a legitimate
//! configuration rather than an error, so each is reported once at startup and not
//! per tick.
// spec: CRT

use std::collections::HashSet;
use std::time::Duration;

use commons_errors::{AppError, Result};
use commons_servers::acme::{Acme, Failure, Fault};
use commons_servers::dns_provider::{DnsProvider, RecordKind, RecordSet};
use commons_types::dns::{ManagedZone, is_within, match_zone};
use database::application_certificates::ApplicationCertificate;
use database::application_names::ApplicationName;
use database::applications::Application;
use database::server_domains::ServerGroupDomain;
use futures::StreamExt;
use tracing::{debug, error, info, warn};

/// How often addresses and outstanding orders are looked at. Short, because a
/// server that has just registered a name is waiting for it to resolve, and both
/// sweeps cost one indexed read when there is nothing to do.
pub const WORK_INTERVAL: Duration = Duration::from_secs(15);

/// How often certificates are checked for being due to renew. Long, because
/// renewal is due over hours or days and the sweep is fleet-wide.
pub const RENEWAL_INTERVAL: Duration = Duration::from_secs(300);

/// Registrations reconciled per tick. Each is a couple of zone writes, so a
/// large batch is affordable; the cap is a backstop against one tick hogging the
/// pod after a bulk change.
pub const NAME_BATCH: i64 = 64;

/// Orders claimed per tick. Small, because each one waits on a certificate
/// authority.
pub const ORDER_BATCH: i64 = 16;

/// How many orders are driven at once. An order spends nearly all its time
/// waiting for a resolver or an authority, so several in flight cost little; the
/// cap is there to stay well inside the authority's concurrency and to bound the
/// connections taken from the pool.
pub const ORDER_CONCURRENCY: usize = 4;

/// Publish (or withdraw) the records for the registrations that have changed.
pub async fn reconcile_addresses(
	pool: &database::Db,
	dns: &DnsProvider,
	zones: &[ManagedZone],
) -> Result<usize> {
	let mut db = pool.get().await?;
	let due = ApplicationName::needing_publish(&mut db, NAME_BATCH).await?;
	let mut reconciled = 0;

	for row in due {
		match publish_addresses(&mut db, dns, zones, &row).await {
			Ok(()) => reconciled += 1,
			Err(err) => {
				// The intent stays where it is and the next tick tries again: a
				// zone write failing is nearly always transient, and a server that
				// asked to be reachable has not changed its mind.
				warn!(name = %row.name, "could not publish addresses: {err}");
				ApplicationName::record_publish_error(&mut db, row.id, &err.to_string()).await?;
			}
		}
	}

	Ok(reconciled)
}

/// Bring one registration's records into line with what the server asked for.
// spec: ADR#registering
async fn publish_addresses(
	db: &mut database::diesel_async::AsyncPgConnection,
	dns: &DnsProvider,
	zones: &[ManagedZone],
	row: &ApplicationName,
) -> Result<()> {
	let Some(zone) = match_zone(&row.name, zones) else {
		return Err(AppError::Conflict(format!(
			"no configured DNS zone covers {}, so Canopy cannot publish records for it",
			row.name
		)));
	};

	let wanted = row.wanted();
	let wanted_sets = RecordSet::addresses(&row.name, &wanted);
	let wanted_kinds: HashSet<RecordKind> = wanted_sets.iter().map(|set| set.kind).collect();

	// A family that was published and is no longer wanted has to be removed
	// explicitly: replacing the sets Canopy still writes says nothing about the
	// one it has stopped writing, and a server that has dropped its IPv6 address
	// would otherwise keep resolving to it.
	for stale in RecordSet::addresses(&row.name, &row.published()) {
		if !wanted_kinds.contains(&stale.kind) {
			dns.delete(zone, &stale).await?;
		}
	}

	for set in &wanted_sets {
		dns.upsert(zone, set).await?;
	}

	if row.is_withdrawing() {
		// Nothing wanted, and now nothing published. Forgetting the registration
		// is what frees the name for another server in the group.
		ApplicationName::forget(db, row.id).await?;
		info!(name = %row.name, "withdrew the name and freed it");
	} else {
		ApplicationName::record_published(db, row.id, &wanted).await?;
		debug!(name = %row.name, addresses = ?wanted, "published addresses");
	}

	Ok(())
}

/// Drive the orders that are due, several at a time.
pub async fn work_orders(
	pool: &database::Db,
	dns: &DnsProvider,
	acme: &Acme,
	zones: &[ManagedZone],
) -> Result<Round> {
	let claimed = {
		let mut db = pool.get().await?;
		ApplicationCertificate::claim_due(&mut db, ORDER_BATCH).await?
	};
	if claimed.is_empty() {
		return Ok(Round::default());
	}

	// Set as soon as the authority says it is being asked too fast. Every order
	// still queued behind it is abandoned for this round rather than worked:
	// those limits are shared across every group in the zone, so spending what is
	// left retrying a name that has just failed is the one thing not to do.
	// spec: CRT#when-issuance-fails
	let throttled = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

	let outcomes: Vec<Option<Fault>> = futures::stream::iter(claimed)
		.map(|cert| {
			let throttled = throttled.clone();
			async move {
				if throttled.load(std::sync::atomic::Ordering::Relaxed) {
					debug!(name = %cert.name, "skipped: the authority is throttling us");
					return Skipped;
				}
				// Its own connection, so one order waiting on the authority does not
				// hold a connection the others need.
				let mut db = match pool.get().await {
					Ok(db) => db,
					Err(err) => {
						error!(name = %cert.name, "no database connection to work the order: {err}");
						return Skipped;
					}
				};
				let outcome = work_order(&mut db, dns, acme, zones, cert).await;
				if outcome == Faulted(Some(Fault::Throttled)) {
					throttled.store(true, std::sync::atomic::Ordering::Relaxed);
				}
				outcome
			}
		})
		.buffer_unordered(ORDER_CONCURRENCY)
		.filter_map(|outcome| {
			std::future::ready(match outcome {
				Issued => Some(None),
				Faulted(fault) => Some(fault),
				Skipped => None,
			})
		})
		.collect()
		.await;

	Ok(Round {
		issued: outcomes.iter().filter(|fault| fault.is_none()).count(),
		// The worst thing the round hit, which is what the fleet-wide alerting
		// reports. A round with one throttled order and ten ordinary refusals is
		// a throttled round.
		fault: outcomes.iter().flatten().copied().max(),
	})
}

/// What one round of order work saw.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Round {
	/// Certificates obtained.
	pub issued: usize,
	/// The most serious fault the round hit, if any. `None` means every order
	/// either succeeded or was not worked, which is what recovers the fleet-wide
	/// authority alerts.
	pub fault: Option<Fault>,
}

/// How one order ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OrderOutcome {
	Issued,
	/// Failed or stopped, with whose fault it was where the authority was involved.
	Faulted(Option<Fault>),
	/// Not attempted, so it says nothing about the authority either way.
	Skipped,
}
use OrderOutcome::{Faulted, Issued, Skipped};

/// Work one order to a conclusion, recording whichever conclusion it reached.
/// Returns whether a certificate came out of it.
// spec: CRT#fulfilment-is-not-immediate
async fn work_order(
	db: &mut database::diesel_async::AsyncPgConnection,
	dns: &DnsProvider,
	acme: &Acme,
	zones: &[ManagedZone],
	cert: ApplicationCertificate,
) -> OrderOutcome {
	match attempt_order(db, dns, acme, zones, &cert).await {
		Ok(Outcome::Issued) => Issued,
		Ok(Outcome::Stopped) => Skipped,
		Err(failure) => {
			warn!(
				name = %cert.name,
				renewing = cert.renewing,
				fault = ?failure.fault,
				"certificate order failed, will retry: {failure}"
			);
			// Recorded against the order whoever's fault it is: an operator looking
			// at the server wants to know why its certificate has not arrived, even
			// when the answer is that Canopy cannot issue at all.
			if let Err(err) =
				ApplicationCertificate::record_failure(db, cert.id, &failure.message).await
			{
				error!(name = %cert.name, "could not record the failure: {err}");
			}
			Faulted(Some(failure.fault))
		}
	}
}

enum Outcome {
	Issued,
	/// The order is no longer something Canopy should pursue.
	Stopped,
}

async fn attempt_order(
	db: &mut database::diesel_async::AsyncPgConnection,
	dns: &DnsProvider,
	acme: &Acme,
	zones: &[ManagedZone],
	cert: &ApplicationCertificate,
) -> std::result::Result<Outcome, Failure> {
	// Still wanted? A grant revoked, a domain released, or a group changed all
	// mean Canopy stops here — an order is worked on a server's behalf, and the
	// authorisation for it is asked afresh rather than remembered from when the
	// request was accepted.
	// spec: CRT#renewal
	let server = Application::get_by_id(db, cert.application_id).await?;
	if !server.may_manage_tls {
		ApplicationCertificate::stop(
			db,
			cert.id,
			"this server is no longer allowed to obtain its own certificates",
		)
		.await?;
		info!(name = %cert.name, "stopped: the TLS grant has been withdrawn");
		return Ok(Outcome::Stopped);
	}
	let controlled = match server.group_id {
		Some(group) => ServerGroupDomain::controlling(db, &cert.name)
			.await?
			.is_some_and(|claim| claim.group_id == group && is_within(&cert.name, &claim.domain)),
		None => false,
	};
	if !controlled {
		ApplicationCertificate::stop(
			db,
			cert.id,
			"this server's group no longer controls a domain covering this name",
		)
		.await?;
		info!(name = %cert.name, "stopped: the name is no longer the group's");
		return Ok(Outcome::Stopped);
	}

	// A zone Canopy cannot write is a configuration problem, not a decision: the
	// order keeps its place and recovers when the configuration does.
	// spec: DOM#when-the-zone-configuration-changes
	let zone = match_zone(&cert.name, zones).ok_or_else(|| {
		Failure::from(AppError::Conflict(format!(
			"no configured DNS zone covers {}, so Canopy cannot prove control of it",
			cert.name
		)))
	})?;

	// A profile the authority has withdrawn is reported as unavailable rather
	// than requested and refused, so the message names the actual problem.
	// spec: CRT#lifetime
	let profile = server.certificate_profile.as_deref();
	if let Some(profile) = profile {
		let offered = acme.profiles();
		if !offered.iter().any(|name| name == profile) {
			return Err(Failure::from(AppError::Conflict(format!(
				"the authority no longer offers the {profile:?} profile this server is set to \
				 (it offers {})",
				if offered.is_empty() {
					"none".to_string()
				} else {
					offered.join(", ")
				}
			))));
		}
	}

	// Only a renewal tells the authority what it replaces; a first issuance has
	// nothing to replace, and the chain of a previous order for a different key
	// is not what this one extends.
	let replacing = if cert.renewing {
		cert.chain.as_deref()
	} else {
		None
	};

	let issued = acme
		.obtain(dns, zone, &cert.name, &cert.csr, profile, replacing)
		.await?;
	ApplicationCertificate::record_issued(
		db,
		cert.id,
		&issued.chain,
		issued.not_after,
		issued.profile.as_deref(),
		issued.renew_after,
	)
	.await?;
	info!(
		name = %cert.name,
		renewing = cert.renewing,
		not_after = %issued.not_after,
		profile = ?issued.profile,
		"certificate issued"
	);

	Ok(Outcome::Issued)
}

/// Mark the certificates that are due to renew, which the order sweep then
/// works like any other pending order.
// spec: CRT#renewal
pub async fn start_renewals(pool: &database::Db) -> Result<usize> {
	let mut db = pool.get().await?;
	let started = ApplicationCertificate::start_renewals(&mut db).await?;
	for cert in &started {
		debug!(name = %cert.name, "renewal due");
	}
	Ok(started.len())
}
