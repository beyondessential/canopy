//! The domains pod's sweeps, driven against a fake zone and a fake certificate
//! authority.
//!
//! Both fakes record what they were asked to do, so these tests read the zone
//! writes Canopy made — which is the part a database-level test cannot see and
//! the part an operator cares about: that a withdrawn address stops resolving,
//! that a challenge record is taken back down, and that a name Canopy has no
//! business acting on is left alone.

use commons_servers::acme::{Acme, Fault};
use commons_servers::dns_provider::{DnsProvider, RecordChange, RecordKind};
use commons_tests::db::TestDb;
use commons_types::dns::ManagedZone;
use database::application_certificates::OrderState;
use database::diesel_async::AsyncPgConnection;
use database::{ApplicationCertificate, ApplicationName, ServerGroupDomain};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
use jobs::domains::{reconcile_addresses, start_renewals, work_orders};
use std::net::IpAddr;
use uuid::Uuid;

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

const KEY_A: &str = "aa00000000000000000000000000000000000000000000000000000000000000";
const KEY_B: &str = "bb00000000000000000000000000000000000000000000000000000000000000";

fn zones() -> Vec<ManagedZone> {
	ManagedZone::parse_list("tamanu.app=Z1", None).expect("zones")
}

fn addr(raw: &str) -> IpAddr {
	raw.parse().expect("address")
}

/// A server with both grants, in a group that controls `domain`.
async fn entitled_server(conn: &mut AsyncPgConnection, domain: &str) -> Uuid {
	let group = sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(format!("group-{}", Uuid::new_v4()))
		.get_result::<RowId>(conn)
		.await
		.expect("insert group")
		.id;
	ServerGroupDomain::claim(conn, group, domain, None, &zones())
		.await
		.expect("claim domain");

	let host = format!("https://{}.example.invalid", Uuid::new_v4());
	sql_query(
		"WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $3) RETURNING id) INSERT INTO applications (name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) SELECT $1, $2, 'tamanu-central', $3, true, true, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Text, _>("entitled")
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group)
	.get_result::<RowId>(conn)
	.await
	.expect("insert server")
	.id
}

/// The record sets written at `name`, by kind, in the order they were written.
fn upserts(dns: &DnsProvider, name: &str) -> Vec<(RecordKind, Vec<String>)> {
	dns.recorded()
		.into_iter()
		.filter_map(|change| match change {
			RecordChange::Upsert { set, .. } if set.name == name => Some((set.kind, set.values)),
			_ => None,
		})
		.collect()
}

/// The record sets removed at `name`, by kind.
fn deletes(dns: &DnsProvider, name: &str) -> Vec<RecordKind> {
	dns.recorded()
		.into_iter()
		.filter_map(|change| match change {
			RecordChange::Delete { set, .. } if set.name == name => Some(set.kind),
			_ => None,
		})
		.collect()
}

// ── Addresses ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_registered_name_is_published_and_settles() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationName::register(
			&mut conn,
			server,
			"a.fiji.tamanu.app",
			&[addr("192.0.2.1"), addr("2001:db8::1")],
		)
		.await
		.expect("register");

		let done = reconcile_addresses(&pool, &dns, &zones())
			.await
			.expect("reconcile");
		assert_eq!(done, 1);

		assert_eq!(
			upserts(&dns, "a.fiji.tamanu.app"),
			vec![
				(RecordKind::A, vec!["192.0.2.1".to_string()]),
				(RecordKind::Aaaa, vec!["2001:db8::1".to_string()]),
			]
		);

		let row = ApplicationName::for_name(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read")
			.expect("present");
		assert!(row.is_reconciled(), "published caught up with wanted");
		assert!(row.published_at.is_some());
		assert!(row.last_error.is_none());

		// And a second pass has nothing to do, rather than rewriting the zone
		// every fifteen seconds for the life of the process.
		assert_eq!(
			reconcile_addresses(&pool, &dns, &zones())
				.await
				.expect("reconcile"),
			0
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_family_no_longer_wanted_is_taken_down() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationName::register(
			&mut conn,
			server,
			"a.fiji.tamanu.app",
			&[addr("192.0.2.1"), addr("2001:db8::1")],
		)
		.await
		.expect("register");
		reconcile_addresses(&pool, &dns, &zones())
			.await
			.expect("first reconcile");

		// The server drops its IPv6 address. Rewriting only the A set would leave
		// the AAAA pointing at an address the server no longer answers on.
		ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[addr("192.0.2.1")])
			.await
			.expect("re-register");
		reconcile_addresses(&pool, &dns, &zones())
			.await
			.expect("second reconcile");

		assert_eq!(
			deletes(&dns, "a.fiji.tamanu.app"),
			vec![RecordKind::Aaaa],
			"the family that went away is removed, and only that one"
		);
		let row = ApplicationName::for_name(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read")
			.expect("present");
		assert_eq!(row.published(), vec![addr("192.0.2.1")]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_withdrawn_name_is_taken_down_and_freed() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[addr("192.0.2.1")])
			.await
			.expect("register");
		reconcile_addresses(&pool, &dns, &zones())
			.await
			.expect("publish");

		ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[])
			.await
			.expect("withdraw");
		reconcile_addresses(&pool, &dns, &zones())
			.await
			.expect("withdraw reconcile");

		assert_eq!(deletes(&dns, "a.fiji.tamanu.app"), vec![RecordKind::A]);
		assert!(
			ApplicationName::for_name(&mut conn, "a.fiji.tamanu.app")
				.await
				.expect("read")
				.is_none(),
			"the registration is forgotten, which is what frees the name"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_outside_every_configured_zone_is_reported_and_kept() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[addr("192.0.2.1")])
			.await
			.expect("register");

		// The zone the name sits under has been dropped from the configuration.
		let done = reconcile_addresses(&pool, &dns, &[])
			.await
			.expect("reconcile");
		assert_eq!(done, 0);
		assert!(dns.recorded().is_empty(), "nothing to write into");

		let row = ApplicationName::for_name(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read")
			.expect("the intent is kept: a configuration Canopy cannot read is not a withdrawal");
		assert!(
			row.last_error
				.as_deref()
				.is_some_and(|e| e.contains("no configured DNS zone")),
			"the reason is recorded for an operator: {:?}",
			row.last_error
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_zone_write_leaves_the_intent_to_retry() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		dns.fail_with("route53 is having a day");
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[addr("192.0.2.1")])
			.await
			.expect("register");

		assert_eq!(
			reconcile_addresses(&pool, &dns, &zones())
				.await
				.expect("reconcile"),
			0
		);

		let row = ApplicationName::for_name(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read")
			.expect("present");
		assert!(row.last_error.is_some());
		assert!(row.published().is_empty(), "nothing was published");
		assert!(
			!ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("work list")
				.is_empty(),
			"still on the work list, so the next tick tries again"
		);
	})
	.await;
}

// ── Certificates ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn an_order_becomes_a_certificate_the_server_can_collect() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order = ApplicationCertificate::request(
			&mut conn,
			server,
			"a.fiji.tamanu.app",
			KEY_A,
			b"csr-a",
		)
		.await
		.expect("request");
		assert_eq!(order.order_state(), OrderState::Pending);

		let round = work_orders(&pool, &dns, &acme, &zones())
			.await
			.expect("work orders");
		assert_eq!(round.issued, 1);
		assert_eq!(round.fault, None, "nothing to report against Canopy");
		assert_eq!(acme.signed(), vec!["a.fiji.tamanu.app"]);

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(after.order_state(), OrderState::Issued);
		assert!(after.chain.is_some());
		assert!(after.is_collectable(), "the server can collect it now");
		assert!(after.not_after.is_some_and(|at| at > Timestamp::now()));
		assert!(
			after.renew_after.is_some(),
			"a renewal time is set even where the authority names none"
		);
		assert_eq!(after.attempts, 0);
		assert!(after.last_error.is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_challenge_record_is_published_under_the_name_and_removed() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
			.await
			.expect("request");

		work_orders(&pool, &dns, &Acme::fake(), &zones())
			.await
			.expect("work orders");

		let challenge = "_acme-challenge.a.fiji.tamanu.app";
		assert_eq!(
			upserts(&dns, challenge).len(),
			1,
			"the proof is published: {:?}",
			dns.recorded()
		);
		assert_eq!(
			deletes(&dns, challenge),
			vec![RecordKind::Txt],
			"and taken back down, so it cannot help authorise the next order"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failing_authority_backs_the_order_off_rather_than_giving_up() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let acme = Acme::fake();
		acme.fail_with("service unavailable");
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		let round = work_orders(&pool, &dns, &acme, &zones())
			.await
			.expect("work orders");
		assert_eq!(round.issued, 0);
		assert_eq!(
			round.fault,
			Some(Fault::Order),
			"an authority that is up but unhappy is this order's problem, not the fleet's"
		);

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(
			after.order_state(),
			OrderState::Pending,
			"an order Canopy was asked for is worth continuing to try"
		);
		assert_eq!(after.attempts, 1);
		assert!(after.next_attempt_at > Timestamp::now(), "backed off");
		assert!(
			after
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("service unavailable")),
			"the authority's own words are kept: {:?}",
			after.last_error
		);

		// Nothing to claim until the backoff has run out, so a broken authority is
		// not hammered.
		assert!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("claim")
				.is_empty()
		);

		// And once it recovers, the same order goes through.
		acme.recover();
		sql_query("UPDATE application_certificates SET next_attempt_at = now() WHERE id = $1")
			.bind::<sql_types::Uuid, _>(order.id)
			.execute(&mut conn)
			.await
			.expect("bring the attempt forward");
		assert_eq!(
			work_orders(&pool, &dns, &acme, &zones())
				.await
				.expect("work orders")
				.issued,
			1
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_order_for_a_server_that_lost_the_grant_is_stopped() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		sql_query("UPDATE applications SET may_manage_tls = false WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("withdraw the grant");

		work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(after.order_state(), OrderState::Failed);
		assert!(
			acme.signed().is_empty(),
			"nothing was asked of the authority"
		);
		assert!(
			after
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("no longer allowed")),
			"got {:?}",
			after.last_error
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_order_for_a_name_the_group_released_is_stopped() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		let claim = ServerGroupDomain::controlling(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read claim")
			.expect("present");
		ServerGroupDomain::release(&mut conn, claim.id)
			.await
			.expect("release");

		work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(after.order_state(), OrderState::Failed);
		assert!(acme.signed().is_empty());
		assert!(
			after
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("no longer controls")),
			"got {:?}",
			after.last_error
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_the_authority_withdrew_is_reported_rather_than_requested() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		sql_query("UPDATE applications SET certificate_profile = 'retired-profile' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("set the profile");
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert!(acme.signed().is_empty(), "not requested and refused");
		assert_eq!(
			after.order_state(),
			OrderState::Pending,
			"the order waits for the configuration to be corrected"
		);
		assert!(
			after
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("no longer offers") && e.contains("retired-profile")),
			"the message names the profile and what is on offer: {:?}",
			after.last_error
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_profile_the_authority_offers_is_requested_and_recorded() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		sql_query("UPDATE applications SET certificate_profile = 'shortlived' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("set the profile");
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(after.order_state(), OrderState::Issued);
		assert_eq!(after.profile.as_deref(), Some("shortlived"));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_due_to_renew_is_reissued_without_being_asked() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		// The authority names a renewal time that has already come, which is what a
		// certificate well into its life looks like.
		acme.set_renew_after(Timestamp::now() - SignedDuration::from_hours(1));
		work_orders(&pool, &dns, &acme, &zones())
			.await
			.expect("first issuance");
		let first = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");

		assert_eq!(
			start_renewals(&pool).await.expect("renewal sweep"),
			1,
			"due, so marked for the order sweep"
		);
		let pending = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(pending.order_state(), OrderState::Pending);
		assert!(
			pending.renewing,
			"told apart from a first issuance, so a failure reads differently"
		);
		assert!(
			pending.is_collectable(),
			"the old chain is still served while the new one is being obtained"
		);

		// Nothing further from the authority about renewal this time, so the
		// fallback fraction of the certificate's own life applies.
		acme.set_lifetime(std::time::Duration::from_secs(90 * 24 * 60 * 60));
		work_orders(&pool, &dns, &acme, &zones())
			.await
			.expect("renewal");

		let renewed = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(renewed.order_state(), OrderState::Issued);
		assert!(!renewed.renewing);
		assert_ne!(
			renewed.chain, first.chain,
			"a new chain, not the one it already had"
		);
		assert_eq!(acme.signed().len(), 2);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_challenge_goes_to_the_zone_that_covers_the_name() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let dns = DnsProvider::fake();
		// Two zones, one inside the other: the proof belongs in the more specific
		// one, because that is the zone whose records a resolver will read.
		let zones = ManagedZone::parse_list("tamanu.app=Zwide, fiji.tamanu.app=Znarrow", None)
			.expect("zones");
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_B, b"csr")
			.await
			.expect("request");

		work_orders(&pool, &dns, &Acme::fake(), &zones)
			.await
			.expect("work orders");

		let written: Vec<String> = dns
			.recorded()
			.into_iter()
			.map(|change| match change {
				RecordChange::Upsert { zone, .. } | RecordChange::Delete { zone, .. } => zone,
			})
			.collect();
		assert!(
			written.iter().all(|zone| zone == "fiji.tamanu.app"),
			"got {written:?}"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_no_configured_zone_covers_keeps_its_order_waiting() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		// The zone has been dropped from the configuration since the request.
		work_orders(&pool, &DnsProvider::fake(), &acme, &[])
			.await
			.expect("work orders");

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert_eq!(
			after.order_state(),
			OrderState::Pending,
			"a configuration problem is not a decision to stop"
		);
		assert!(
			after
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("no configured DNS zone")),
			"got {:?}",
			after.last_error
		);
		assert!(acme.signed().is_empty());
	})
	.await;
}

// ── Whose fault it is ───────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn an_unreachable_authority_is_reported_as_canopys_own_problem() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		acme.fail_with_fault(Fault::Unreachable, "connection refused");
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		let round = work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");
		assert_eq!(round.fault, Some(Fault::Unreachable));
		assert!(round.fault.is_some_and(Fault::is_canopys));

		// Still recorded against the order too: an operator looking at the server
		// wants to know why its certificate has not arrived.
		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("read");
		assert!(
			after
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("connection refused")),
			"got {:?}",
			after.last_error
		);
		assert_eq!(after.order_state(), OrderState::Pending, "not given up on");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_throttled_round_abandons_the_rest_rather_than_spending_the_allowance() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		acme.fail_with_fault(Fault::Throttled, "too many certificates already issued");
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		// More orders than the concurrency limit, so there is a queue to abandon.
		for index in 0..12 {
			let key =
				format!("{index:02}00000000000000000000000000000000000000000000000000000000000000");
			ApplicationCertificate::request(
				&mut conn,
				server,
				&format!("n{index}.fiji.tamanu.app"),
				&key[..64],
				b"csr",
			)
			.await
			.expect("request");
		}

		let round = work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");
		assert_eq!(round.fault, Some(Fault::Throttled));

		// The queue behind the first refusal is dropped: those limits are shared
		// across every group in the zone, so asking harder is the one thing not to
		// do. Some orders are already in flight when the first refusal lands, so
		// the assertion is that most were spared rather than exactly one attempted.
		let attempted = count_attempted(&mut conn, server).await;
		assert!(
			attempted < 12,
			"expected the queue to be abandoned, but all {attempted} were attempted"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_round_that_goes_through_says_nothing_is_wrong_with_the_authority() {
	TestDb::run(async |mut conn, url| {
		let pool = database::init_to(&url);
		let acme = Acme::fake();
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
			.await
			.expect("request");

		let round = work_orders(&pool, &DnsProvider::fake(), &acme, &zones())
			.await
			.expect("work orders");
		assert_eq!(round.fault, None);

		// Which is what the self-alerting reads to recover all three conditions.
		database::self_alerts::sweep_certificate_authority(&mut conn, round.fault, None)
			.await
			.expect("authority sweep");
		for r#ref in [
			database::self_alerts::CA_UNREACHABLE_REF,
			database::self_alerts::CA_ACCOUNT_REF,
			database::self_alerts::CA_THROTTLED_REF,
		] {
			let current = database::self_alerts::current(&mut conn, r#ref)
				.await
				.expect("read");
			assert!(
				current.is_none_or(|issue| !issue.active),
				"{ref} should not be standing"
			);
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_three_authority_conditions_are_reported_apart() {
	TestDb::run(async |mut conn, _url| {
		use database::self_alerts::{
			CA_ACCOUNT_REF, CA_THROTTLED_REF, CA_UNREACHABLE_REF, current,
			sweep_certificate_authority,
		};

		for (fault, expected) in [
			(Fault::Unreachable, CA_UNREACHABLE_REF),
			(Fault::Account, CA_ACCOUNT_REF),
			(Fault::Throttled, CA_THROTTLED_REF),
		] {
			sweep_certificate_authority(&mut conn, Some(fault), Some("the authority said no"))
				.await
				.expect("sweep");

			for r#ref in [CA_UNREACHABLE_REF, CA_ACCOUNT_REF, CA_THROTTLED_REF] {
				let standing = current(&mut conn, r#ref)
					.await
					.expect("read")
					.is_some_and(|issue| issue.active);
				assert_eq!(
					standing,
					r#ref == expected,
					"{fault:?} should raise {expected} and nothing else, but {ref} is \
					 standing={standing}"
				);
			}
		}

		// And an order that failed on its own merits says the authority is fine.
		sweep_certificate_authority(&mut conn, Some(Fault::Order), None)
			.await
			.expect("sweep");
		for r#ref in [CA_UNREACHABLE_REF, CA_ACCOUNT_REF, CA_THROTTLED_REF] {
			assert!(
				current(&mut conn, r#ref)
					.await
					.expect("read")
					.is_none_or(|issue| !issue.active),
				"{ref} should have recovered"
			);
		}
	})
	.await;
}

/// How many of a server's orders have been attempted at least once.
async fn count_attempted(conn: &mut AsyncPgConnection, server_id: Uuid) -> usize {
	ApplicationCertificate::for_server(conn, server_id)
		.await
		.expect("read certificates")
		.into_iter()
		.filter(|cert| cert.attempts > 0)
		.count()
}
