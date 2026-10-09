//! DB-layer tests for the CRT models: name registration and its exclusivity,
//! and the certificate order lifecycle — idempotent requests, retry backoff,
//! renewal, and the queries the alerts read.

use commons_tests::db::TestDb;
use commons_types::dns::ManagedZone;
use database::application_certificates::{OrderState, RevocationReason, Risk, default_renew_after};
use database::diesel_async::AsyncPgConnection;
use database::{ApplicationCertificate, ApplicationName, ServerGroupDomain};
use diesel::{sql_query, sql_types};
use diesel_async::{RunQueryDsl, SimpleAsyncConnection};
use jiff::{SignedDuration, Timestamp};
use std::net::IpAddr;
use uuid::Uuid;

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	let host = format!("https://{}.example.invalid", Uuid::new_v4());
	let machine = sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result::<RowId>(conn)
		.await
		.expect("insert machine")
		.id;
	sql_query(
		"INSERT INTO applications (name, host, type, machine_id) \
		 VALUES ($1, $2, 'tamanu-central', $3) RETURNING id",
	)
	.bind::<sql_types::Text, _>(name)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result::<RowId>(conn)
	.await
	.expect("insert server")
	.id
}

fn addr(raw: &str) -> IpAddr {
	raw.parse().expect("address")
}

// ── Names ───────────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn registering_normalises_and_starts_unpublished() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let row = ApplicationName::register(
			&mut conn,
			server,
			"Central.Fiji.Tamanu.App.",
			&[addr("192.0.2.1"), addr("2001:db8::1")],
		)
		.await
		.expect("register");

		assert_eq!(row.name, "central.fiji.tamanu.app");
		assert_eq!(row.wanted(), vec![addr("192.0.2.1"), addr("2001:db8::1")]);
		assert!(row.published().is_empty());
		assert!(!row.is_reconciled(), "nothing published yet");

		let work = ApplicationName::needing_publish(&mut conn, 10)
			.await
			.expect("work list");
		assert_eq!(work.len(), 1);
		assert_eq!(work[0].id, row.id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn publishing_settles_the_registration() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let row =
			ApplicationName::register(&mut conn, server, "a.tamanu.app", &[addr("192.0.2.1")])
				.await
				.expect("register");

		ApplicationName::record_published(&mut conn, row.id, &[addr("192.0.2.1")])
			.await
			.expect("record published");

		let after = ApplicationName::for_name(&mut conn, "a.tamanu.app")
			.await
			.expect("read")
			.expect("present");
		assert!(after.is_reconciled());
		assert!(after.published_at.is_some());
		assert!(
			ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("work list")
				.is_empty(),
			"a settled registration is not work"
		);

		// A change of address is work again.
		ApplicationName::register(&mut conn, server, "a.tamanu.app", &[addr("192.0.2.9")])
			.await
			.expect("re-register");
		assert_eq!(
			ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("work list")
				.len(),
			1
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn address_order_is_not_a_change() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let row = ApplicationName::register(
			&mut conn,
			server,
			"a.tamanu.app",
			&[addr("192.0.2.1"), addr("192.0.2.2")],
		)
		.await
		.expect("register");
		// Published in the other order: the same set, so no work.
		ApplicationName::record_published(
			&mut conn,
			row.id,
			&[addr("192.0.2.2"), addr("192.0.2.1")],
		)
		.await
		.expect("record published");
		assert!(
			ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("work list")
				.is_empty()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_belongs_to_one_server_at_a_time() {
	TestDb::run(async |mut conn, _url| {
		let one = insert_server(&mut conn, "one").await;
		let two = insert_server(&mut conn, "two").await;
		ApplicationName::register(&mut conn, one, "a.tamanu.app", &[addr("192.0.2.1")])
			.await
			.expect("first");

		let err = ApplicationName::register(&mut conn, two, "a.tamanu.app", &[addr("192.0.2.2")])
			.await
			.expect_err("the other server may not take it");
		// Word for word what a name nobody declares is refused with, so the
		// refusal is not a directory of what other machines serve. The routing
		// resolves declarations ahead of this, so only a race reaches it.
		assert!(
			matches!(
				&err,
				commons_errors::AppError::DnsNameUndeclared(m) if m == "a.tamanu.app"
			),
			"got {err:?}"
		);

		// The holder may keep changing its own.
		ApplicationName::register(&mut conn, one, "a.tamanu.app", &[addr("192.0.2.3")])
			.await
			.expect("holder updates");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_frees_the_name_once_records_are_gone() {
	TestDb::run(async |mut conn, _url| {
		let one = insert_server(&mut conn, "one").await;
		let two = insert_server(&mut conn, "two").await;
		let row = ApplicationName::register(&mut conn, one, "a.tamanu.app", &[addr("192.0.2.1")])
			.await
			.expect("register");
		ApplicationName::record_published(&mut conn, row.id, &[addr("192.0.2.1")])
			.await
			.expect("published");

		// No addresses: a withdrawal, which is work until the records are gone.
		let withdrawn = ApplicationName::register(&mut conn, one, "a.tamanu.app", &[])
			.await
			.expect("withdraw");
		assert!(withdrawn.is_withdrawing());
		assert!(!withdrawn.is_reconciled());

		// Still held until the reconcile has cleaned up.
		ApplicationName::register(&mut conn, two, "a.tamanu.app", &[addr("192.0.2.2")])
			.await
			.expect_err("not free yet");

		ApplicationName::record_published(&mut conn, row.id, &[])
			.await
			.expect("records removed");
		ApplicationName::forget(&mut conn, row.id)
			.await
			.expect("forget");

		ApplicationName::register(&mut conn, two, "a.tamanu.app", &[addr("192.0.2.2")])
			.await
			.expect("free now");
	})
	.await;
}

// ── Certificates ────────────────────────────────────────────────────────────

const KEY_A: &str = "aa00000000000000000000000000000000000000000000000000000000000000";
const KEY_B: &str = "bb00000000000000000000000000000000000000000000000000000000000000";

#[tokio::test(flavor = "multi_thread")]
async fn requesting_opens_one_order_per_name_and_key() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let first =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr-a")
				.await
				.expect("request");
		assert_eq!(first.order_state(), OrderState::Pending);
		assert!(!first.renewing, "a first issuance is not a renewal");

		// The same key again is the same order, not a second one.
		let again =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr-a")
				.await
				.expect("repeat");
		assert_eq!(again.id, first.id);

		// A different key for the same name is its own order.
		let other =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_B, b"csr-b")
				.await
				.expect("other key");
		assert_ne!(other.id, first.id);

		assert_eq!(
			ApplicationCertificate::for_server(&mut conn, server)
				.await
				.expect("list")
				.len(),
			2
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_repeat_request_is_answered_from_the_held_certificate() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		let expiry = Timestamp::now() + SignedDuration::from_hours(90 * 24);
		ApplicationCertificate::record_issued(
			&mut conn,
			order.id,
			"-----BEGIN...",
			expiry,
			Some("classic"),
			None,
		)
		.await
		.expect("issued");

		let again =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr")
				.await
				.expect("repeat");
		assert_eq!(again.id, order.id);
		assert_eq!(again.order_state(), OrderState::Issued);
		assert!(again.is_current());
		assert_eq!(again.chain.as_deref(), Some("-----BEGIN..."));
		assert!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.is_empty(),
			"an issued certificate is no longer work"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_certificate_is_ordered_again_on_request() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		let past = Timestamp::now() - SignedDuration::from_hours(1);
		ApplicationCertificate::record_issued(&mut conn, order.id, "chain", past, None, None)
			.await
			.expect("issued");

		let again =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr")
				.await
				.expect("repeat");
		assert_eq!(again.id, order.id);
		assert_eq!(again.order_state(), OrderState::Pending);
		assert!(again.renewing, "it had a chain, so this extends one");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_failed_attempt_backs_off_and_stays_pending() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		ApplicationCertificate::record_failure(&mut conn, order.id, "the authority said no")
			.await
			.expect("record failure");

		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("get");
		assert_eq!(after.attempts, 1);
		assert_eq!(
			after.order_state(),
			OrderState::Pending,
			"still worth trying"
		);
		assert_eq!(after.last_error.as_deref(), Some("the authority said no"));
		assert!(after.next_attempt_at > Timestamp::now(), "backed off");
		assert!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.is_empty(),
			"not due until the backoff has passed"
		);

		// The second failure waits longer than the first.
		ApplicationCertificate::record_failure(&mut conn, order.id, "again")
			.await
			.expect("record failure");
		let second = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("get");
		assert_eq!(second.attempts, 2);
		assert!(second.next_attempt_at > after.next_attempt_at);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn renewal_follows_the_stored_window_and_reuses_the_request() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;

		// Due: the authority named a window that has already opened.
		let soon =
			ApplicationCertificate::request(&mut conn, server, "soon.tamanu.app", KEY_A, b"csr-1")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			soon.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			Some("shortlived"),
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("issued");

		// Not due: a long-lived certificate issued just now.
		let later =
			ApplicationCertificate::request(&mut conn, server, "later.tamanu.app", KEY_B, b"csr-2")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			later.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(90 * 24),
			Some("classic"),
			None,
		)
		.await
		.expect("issued");

		let started = ApplicationCertificate::start_renewals(&mut conn)
			.await
			.expect("start renewals");
		assert_eq!(started.len(), 1, "only the one whose window has opened");
		assert_eq!(started[0].id, soon.id);
		assert!(started[0].renewing);
		assert_eq!(
			started[0].csr, b"csr-1",
			"renewal reuses the request the server sent, needing nothing from it"
		);
		assert!(
			started[0].chain.is_some(),
			"the old chain stays until the new one lands"
		);

		let due = ApplicationCertificate::claim_due(&mut conn, 10)
			.await
			.expect("due");
		assert_eq!(due.len(), 1);
		assert_eq!(due[0].id, soon.id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_renewal_window_scales_with_the_lifetime() {
	// A fixed window cannot serve both: it would leave a six-day certificate
	// permanently overdue, or renew a ninety-day one hundreds of times.
	let issued = Timestamp::now();

	let long = default_renew_after(issued, issued + SignedDuration::from_hours(90 * 24));
	let long_remaining: SignedDuration = (issued + SignedDuration::from_hours(90 * 24) - long)
		.try_into()
		.expect("duration");
	assert_eq!(long_remaining.as_hours(), 30 * 24, "a third of ninety days");

	let short = default_renew_after(issued, issued + SignedDuration::from_hours(6 * 24));
	let short_remaining: SignedDuration = (issued + SignedDuration::from_hours(6 * 24) - short)
		.try_into()
		.expect("duration");
	assert_eq!(short_remaining.as_hours(), 2 * 24, "a third of six days");

	// Already spent: renew at once rather than at some point in the past.
	assert_eq!(default_renew_after(issued, issued), issued);
	assert_eq!(
		default_renew_after(issued, issued - SignedDuration::from_hours(1)),
		issued
	);
}

/// A server with the TLS grant, in a group that controls `domain`.
async fn entitled_server(conn: &mut AsyncPgConnection, domain: &str) -> Uuid {
	let group = sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(format!("group-{}", Uuid::new_v4()))
		.get_result::<RowId>(conn)
		.await
		.expect("insert group")
		.id;
	let zones = ManagedZone::parse_list("tamanu.app=Z1", None).expect("zones");
	ServerGroupDomain::claim(conn, group, domain, None, &zones)
		.await
		.expect("claim domain");

	let machine =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Uuid, _>(group)
			.get_result::<RowId>(conn)
			.await
			.expect("insert machine")
			.id;

	let host = format!("https://{}.example.invalid", Uuid::new_v4());
	sql_query(
		"INSERT INTO applications (name, host, type, group_id, may_manage_tls, machine_id) \
		 VALUES ($1, $2, 'tamanu-central', $3, true, $4) RETURNING id",
	)
	.bind::<sql_types::Text, _>("entitled")
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result::<RowId>(conn)
	.await
	.expect("insert server")
	.id
}

#[tokio::test(flavor = "multi_thread")]
async fn risk_is_judged_against_the_certificates_own_lifetime() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;

		// Two days left. Comfortable on a ninety-day life, critical on a six-day
		// one — the same reading has to mean different things.
		let long = ApplicationCertificate::request(
			&mut conn,
			server,
			"long.fiji.tamanu.app",
			KEY_A,
			b"c1",
		)
		.await
		.expect("request");
		sql_query(
			"UPDATE application_certificates SET state = 'issued', chain = 'x', \
			 issued_at = now() - interval '88 days', not_after = now() + interval '2 days' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(long.id)
		.execute(&mut conn)
		.await
		.expect("age the long one");

		let short = ApplicationCertificate::request(
			&mut conn,
			server,
			"short.fiji.tamanu.app",
			KEY_B,
			b"c2",
		)
		.await
		.expect("request");
		sql_query(
			"UPDATE application_certificates SET state = 'issued', chain = 'x', \
			 issued_at = now() - interval '4 days', not_after = now() + interval '2 days' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(short.id)
		.execute(&mut conn)
		.await
		.expect("age the short one");

		let risks = ApplicationCertificate::at_risk(&mut conn)
			.await
			.expect("at risk");
		let by_id: std::collections::HashMap<Uuid, Risk> =
			risks.iter().map(|(cert, risk)| (cert.id, *risk)).collect();

		// 2 of 90 days left: well past even the critical fraction.
		assert_eq!(by_id.get(&long.id), Some(&Risk::Critical));
		// 2 of 6 days left: exactly the renewal point, so at risk but recoverable.
		assert_eq!(by_id.get(&short.id), Some(&Risk::AtRisk));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_expired_certificate_for_an_unentitled_name_raises_nothing() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		sql_query(
			"UPDATE application_certificates SET state = 'issued', chain = 'x', \
			 issued_at = now() - interval '91 days', not_after = now() - interval '1 day' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(cert.id)
		.execute(&mut conn)
		.await
		.expect("expire it");

		// Entitled and expired: reported.
		let risks = ApplicationCertificate::at_risk(&mut conn)
			.await
			.expect("at risk");
		assert_eq!(risks.len(), 1);
		assert_eq!(risks[0].1, Risk::Critical);

		// The group releases the domain. Canopy stopped renewing, so running out
		// is the intended outcome and there is nothing to report.
		sql_query("DELETE FROM server_group_domains")
			.execute(&mut conn)
			.await
			.expect("release the domain");
		assert!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.is_empty(),
			"an unentitled name must leave no alert behind"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_the_grant_or_archiving_the_server_silences_the_alert() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		sql_query(
			"UPDATE application_certificates SET state = 'issued', chain = 'x', \
			 issued_at = now() - interval '91 days', not_after = now() - interval '1 day' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(cert.id)
		.execute(&mut conn)
		.await
		.expect("expire it");
		assert_eq!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.len(),
			1
		);

		sql_query("UPDATE applications SET may_manage_tls = false WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("revoke");
		assert!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.is_empty(),
			"a revoked grant stops renewal, so expiry is expected"
		);

		// Granted again, and it is back in scope: entitlement is asked now, not
		// remembered from when renewal stopped.
		sql_query("UPDATE applications SET may_manage_tls = true WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("re-grant");
		assert_eq!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.len(),
			1
		);

		sql_query("UPDATE applications SET deleted_at = now() WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("archive");
		assert!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.is_empty(),
			"an archived server is not something to alert about"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_first_issuance_that_keeps_failing_is_told_apart() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let never =
			ApplicationCertificate::request(&mut conn, server, "never.tamanu.app", KEY_B, b"csr-2")
				.await
				.expect("request");
		for _ in 0..4 {
			ApplicationCertificate::record_failure(&mut conn, never.id, "nope")
				.await
				.expect("fail");
		}

		let stuck = ApplicationCertificate::stuck_first_issuances(&mut conn, 3)
			.await
			.expect("stuck");
		assert_eq!(stuck.len(), 1, "only the one that never produced a chain");
		assert_eq!(stuck[0].id, never.id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stopping_an_order_takes_it_out_of_the_work_list() {
	TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "central").await;
		let order =
			ApplicationCertificate::request(&mut conn, server, "a.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		ApplicationCertificate::stop(&mut conn, order.id, "the group released the domain")
			.await
			.expect("stop");

		assert!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.is_empty()
		);
		let after = ApplicationCertificate::get(&mut conn, order.id)
			.await
			.expect("get");
		assert_eq!(after.order_state(), OrderState::Failed);
		assert_eq!(
			after.last_error.as_deref(),
			Some("the group released the domain")
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_stops_renewal_and_unholds_the_certificate() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			Some("classic"),
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("issued");
		// Due for renewal, until it isn't.
		assert_eq!(
			ApplicationCertificate::start_renewals(&mut conn)
				.await
				.expect("renewals")
				.len(),
			1
		);
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			Some("classic"),
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("reissued");

		ApplicationCertificate::record_revoked(
			&mut conn,
			cert.id,
			RevocationReason::Superseded,
			Some("op@example.test"),
		)
		.await
		.expect("revoke");

		let after = ApplicationCertificate::get(&mut conn, cert.id)
			.await
			.expect("get");
		assert_eq!(after.order_state(), OrderState::Revoked);
		assert!(!after.is_current(), "a revoked certificate is not held");
		assert!(after.revoked_at.is_some());
		assert_eq!(after.revoked_by.as_deref(), Some("op@example.test"));
		assert_eq!(after.revocation_reason.as_deref(), Some("superseded"));
		assert!(after.renew_after.is_none(), "nothing left to renew");
		assert!(after.is_revoked());
		assert!(
			!after.requires_new_key(),
			"superseded condemns the certificate, not the key"
		);

		assert!(
			ApplicationCertificate::start_renewals(&mut conn)
				.await
				.expect("renewals")
				.is_empty(),
			"a revoked certificate is not renewed"
		);
		assert!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.is_empty(),
			"a revoked certificate is not an expiry to chase"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_revoked_as_compromised_is_never_certified_again() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			None,
			None,
		)
		.await
		.expect("issued");

		ApplicationCertificate::record_revoked(
			&mut conn,
			cert.id,
			RevocationReason::KeyCompromise,
			Some("op@example.test"),
		)
		.await
		.expect("revoke");

		// The same key, for the same name: refused — and refused with its own
		// problem type, so an agent can rotate the key on the type alone rather
		// than parsing the message.
		let err =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect_err("a compromised key must not be certified again");
		assert!(
			matches!(err, commons_errors::AppError::CertificateKeyCompromised(_)),
			"an agent has to be able to act on this without reading prose; got {err:?}"
		);

		// And for any other name, by anyone: a leaked key is leaked whoever asks.
		let other = insert_server(&mut conn, "other").await;
		ApplicationCertificate::request(&mut conn, other, "b.fiji.tamanu.app", KEY_A, b"c")
			.await
			.expect_err("still barred for another name and server");

		// A fresh key is fine.
		ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_B, b"c2")
			.await
			.expect("a new key is certifiable");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn revoking_for_another_reason_leaves_the_key_usable() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			None,
			None,
		)
		.await
		.expect("issued");
		ApplicationCertificate::record_revoked(
			&mut conn,
			cert.id,
			RevocationReason::CessationOfOperation,
			None,
		)
		.await
		.expect("revoke");

		// A certificate retired says nothing about the key, so asking again
		// re-opens the order rather than being refused.
		let again =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("the key is still usable");
		assert_eq!(again.id, cert.id);
		assert_eq!(again.order_state(), OrderState::Pending);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_compromise_revocation_tells_the_server_to_replace_the_key() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			None,
			None,
		)
		.await
		.expect("issued");
		ApplicationCertificate::record_revoked(
			&mut conn,
			cert.id,
			RevocationReason::KeyCompromise,
			Some("op@example.test"),
		)
		.await
		.expect("revoke");

		// What a collecting server reads: stop serving this, and the key itself
		// is condemned — not just the certificate.
		let after = ApplicationCertificate::get(&mut conn, cert.id)
			.await
			.expect("get");
		assert!(after.is_revoked());
		assert!(
			after.requires_new_key(),
			"a compromised key has to be discarded, not reused"
		);
		assert!(!after.is_current(), "it must not be served");
	})
	.await;
}

// ── Pausing ─────────────────────────────────────────────────────────────────

use database::applications::Application;

#[tokio::test(flavor = "multi_thread")]
async fn revoking_pauses_the_server_so_reissuance_cannot_chase_it() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			None,
			None,
		)
		.await
		.expect("issued");

		assert!(
			!Application::get_by_id(&mut conn, server)
				.await
				.expect("get")
				.name_management_paused(),
			"not paused to start with"
		);

		ApplicationCertificate::record_revoked(
			&mut conn,
			cert.id,
			RevocationReason::KeyCompromise,
			Some("op@example.test"),
		)
		.await
		.expect("revoke");

		let after = Application::get_by_id(&mut conn, server)
			.await
			.expect("get");
		assert!(after.name_management_paused(), "revoking pauses the server");
		assert_eq!(
			after.name_management_paused_by.as_deref(),
			Some("op@example.test")
		);
		assert!(
			after
				.name_management_pause_reason
				.as_deref()
				.is_some_and(|r| r.contains("revoked")),
			"the reason should say what happened: {:?}",
			after.name_management_pause_reason
		);

		// The whole point: a fresh key on a paused server gets no new order
		// worked, so the attacker who took the old key gets nothing.
		ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_B, b"c2")
			.await
			.expect("recording the request is fine");
		assert!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.is_empty(),
			"no order is worked while the server is paused"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pause_stops_every_kind_of_work_and_withdraws_nothing() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;

		// An address change waiting to be published, and a renewal falling due.
		let name =
			ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[addr("192.0.2.1")])
				.await
				.expect("register");
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			None,
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("issued");

		// Unpaused: all three queues have work.
		assert_eq!(
			ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("names")
				.len(),
			1
		);
		assert_eq!(
			ApplicationCertificate::start_renewals(&mut conn)
				.await
				.expect("renew")
				.len(),
			1
		);
		assert_eq!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.len(),
			1
		);

		Application::pause_name_management(
			&mut conn,
			server,
			Some("op@example.test"),
			"investigating",
		)
		.await
		.expect("pause");

		assert!(
			ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("names")
				.is_empty(),
			"no record changes while paused"
		);
		assert!(
			ApplicationCertificate::start_renewals(&mut conn)
				.await
				.expect("renew")
				.is_empty(),
			"no renewals while paused"
		);
		assert!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.is_empty(),
			"no orders worked while paused"
		);

		// But nothing was withdrawn: the registration and the certificate stand.
		let held = ApplicationCertificate::get(&mut conn, cert.id)
			.await
			.expect("get");
		// A renewal is in flight, so the row is pending again — but the chain it
		// holds is untouched and the server must still be served it, or an agent
		// polling mid-renewal would stop serving TLS on a name with weeks left.
		assert!(
			held.is_collectable(),
			"the chain stands and stays collectable while a renewal is under way"
		);
		assert!(held.chain.is_some());
		assert!(
			ApplicationName::for_name(&mut conn, "a.fiji.tamanu.app")
				.await
				.expect("read")
				.is_some(),
			"the registration stands"
		);
		assert_eq!(name.wanted(), vec![addr("192.0.2.1")]);

		// Resuming picks the work back up where it left off.
		Application::resume_name_management(&mut conn, server)
			.await
			.expect("resume");
		assert_eq!(
			ApplicationName::needing_publish(&mut conn, 10)
				.await
				.expect("names")
				.len(),
			1
		);
		assert_eq!(
			ApplicationCertificate::claim_due(&mut conn, 10)
				.await
				.expect("due")
				.len(),
			1
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_paused_server_raises_no_expiry_alert_but_the_pause_is_reportable() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		sql_query(
			"UPDATE application_certificates SET state = 'issued', chain = 'x', \
			 issued_at = now() - interval '91 days', not_after = now() - interval '1 day' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(cert.id)
		.execute(&mut conn)
		.await
		.expect("expire it");

		assert_eq!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.len(),
			1,
			"expired and entitled: reported"
		);

		Application::pause_name_management(&mut conn, server, None, "investigating a leak")
			.await
			.expect("pause");
		assert!(
			ApplicationCertificate::at_risk(&mut conn)
				.await
				.expect("at risk")
				.is_empty(),
			"a paused server's expiry is the expected consequence, not a failure"
		);

		// The pause itself is what becomes reportable, because something has lapsed
		// under it — otherwise a forgotten pause is how certificates quietly
		// expire. The alerting reads this list; what it does with it is covered in
		// `certificate_alerts`.
		let lapsing = ApplicationCertificate::lapsing_under_pause(&mut conn)
			.await
			.expect("lapsing");
		assert_eq!(lapsing.len(), 1);
		assert_eq!(lapsing[0].application_id, server);
		assert!(lapsing[0].expired);
		assert_eq!(
			lapsing[0].pause_reason.as_deref(),
			Some("investigating a leak")
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn pausing_again_keeps_the_original_pause() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		Application::pause_name_management(
			&mut conn,
			server,
			Some("first@example.test"),
			"the real reason",
		)
		.await
		.expect("pause");
		Application::pause_name_management(
			&mut conn,
			server,
			Some("second@example.test"),
			"something else",
		)
		.await
		.expect("pause again");

		let after = Application::get_by_id(&mut conn, server)
			.await
			.expect("get");
		assert_eq!(
			after.name_management_paused_by.as_deref(),
			Some("first@example.test"),
			"the pause being investigated is the one that stopped the work"
		);
		assert_eq!(
			after.name_management_pause_reason.as_deref(),
			Some("the real reason")
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_renewal_in_flight_does_not_stop_the_old_chain_being_collectable() {
	TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"c")
				.await
				.expect("request");
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"chain",
			Timestamp::now() + SignedDuration::from_hours(48),
			None,
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("issued");

		ApplicationCertificate::start_renewals(&mut conn)
			.await
			.expect("renew");

		let during = ApplicationCertificate::get(&mut conn, cert.id)
			.await
			.expect("get");
		assert_eq!(
			during.order_state(),
			OrderState::Pending,
			"renewal under way"
		);
		assert!(!during.is_current(), "not `issued` while the renewal runs");
		assert!(
			during.is_collectable(),
			"but the chain is still valid, so the server must still be served it"
		);

		// Revoked or expired are the things that actually disqualify a chain.
		ApplicationCertificate::record_revoked(
			&mut conn,
			cert.id,
			RevocationReason::Superseded,
			None,
		)
		.await
		.expect("revoke");
		assert!(
			!ApplicationCertificate::get(&mut conn, cert.id)
				.await
				.expect("get")
				.is_collectable(),
			"a revoked chain must not be served"
		);
	})
	.await;
}

/// An operator ties a name to the software answering on it, so a box running
/// several workloads has its later requests routed to the right one.
// spec: NAM#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn declaring_a_name_ties_it_to_one_application() {
	TestDb::run(|mut conn, _url| async move {
		let app = insert_server(&mut conn, "declare-1").await;

		let row = ApplicationName::declare(&mut conn, app, "Front.Fiji.Tamanu.App.")
			.await
			.expect("declare");
		assert_eq!(
			row.name, "front.fiji.tamanu.app",
			"normalised on the way in"
		);
		assert!(
			row.wanted().is_empty(),
			"a declaration carries no addresses; the application registers those itself"
		);

		let again = ApplicationName::declare(&mut conn, app, "front.fiji.tamanu.app")
			.await
			.expect("declaring the same name again changes nothing");
		assert_eq!(again.id, row.id);
	})
	.await
}

/// Safe to name the holder here, and only here: an operator already sees the
/// whole fleet, and needs to know what to release first.
// spec: NAM#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn declaring_a_name_another_application_holds_names_the_holder() {
	TestDb::run(|mut conn, _url| async move {
		let holder = insert_server(&mut conn, "the-holder").await;
		let other = insert_server(&mut conn, "the-other").await;

		ApplicationName::declare(&mut conn, holder, "shared.fiji.tamanu.app")
			.await
			.expect("first declaration");

		let refusal = ApplicationName::declare(&mut conn, other, "shared.fiji.tamanu.app")
			.await
			.expect_err("the second is refused");
		let message = refusal.to_string();
		assert!(
			message.contains("the-holder") && message.contains(&holder.to_string()),
			"the refusal names the holder so an operator can see what to release \
			 first, but said: {message}"
		);
	})
	.await
}

/// Releasing withdraws nothing already in place, exactly as revoking a grant
/// leaves it. What ends is Canopy treating the name as this application's.
// spec: NAM#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn releasing_a_name_leaves_its_certificates_in_place_and_frees_it() {
	TestDb::run(|mut conn, _url| async move {
		let first = insert_server(&mut conn, "release-1").await;
		let second = insert_server(&mut conn, "release-2").await;

		ApplicationName::register(
			&mut conn,
			first,
			"moving.fiji.tamanu.app",
			&[addr("192.0.2.9")],
		)
		.await
		.expect("register");
		let cert = ApplicationCertificate::request(
			&mut conn,
			first,
			"moving.fiji.tamanu.app",
			"a-certified-key",
			b"a-csr",
		)
		.await
		.expect("request a certificate");

		// Issued, granted, and overdue for renewal, so it is genuinely picked up
		// while the declaration stands and the release is what stops it.
		conn.batch_execute(&format!(
			"UPDATE applications SET may_manage_tls = true WHERE id = '{first}'"
		))
		.await
		.expect("grant TLS");
		let expiring = Timestamp::now() + SignedDuration::from_hours(24);
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"a-chain",
			expiring,
			None,
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("record issued");

		assert!(
			ApplicationCertificate::start_renewals(&mut conn)
				.await
				.expect("renewals")
				.iter()
				.any(|c| c.id == cert.id),
			"it is due for renewal while the declaration stands"
		);
		// start_renewals marks what it returns pending, so put it back to issued
		// and overdue for the check after the release.
		ApplicationCertificate::record_issued(
			&mut conn,
			cert.id,
			"a-chain",
			expiring,
			None,
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("still issued and overdue");

		ApplicationName::release(&mut conn, first, "moving.fiji.tamanu.app")
			.await
			.expect("release");

		assert!(
			ApplicationCertificate::get(&mut conn, cert.id)
				.await
				.is_ok(),
			"the certificate held stays held"
		);
		assert!(
			ApplicationName::for_name(&mut conn, "moving.fiji.tamanu.app")
				.await
				.expect("look up")
				.is_none(),
			"and the name is free"
		);

		ApplicationName::declare(&mut conn, second, "moving.fiji.tamanu.app")
			.await
			.expect("free to be declared elsewhere");

		let missing = ApplicationName::release(&mut conn, first, "moving.fiji.tamanu.app")
			.await
			.expect_err("releasing what this application does not hold");
		assert!(
			missing
				.to_string()
				.contains("not declared by this application")
		);

		// Renewing past a release would order for a name the other application
		// now serves, so it stops.
		let due = ApplicationCertificate::start_renewals(&mut conn)
			.await
			.expect("renewals");
		assert!(
			!due.iter().any(|c| c.id == cert.id),
			"a released name's certificate is not renewed"
		);

		// And a deliberate release is not reported as a fault.
		let at_risk = ApplicationCertificate::at_risk(&mut conn)
			.await
			.expect("at risk");
		assert!(
			!at_risk.iter().any(|(c, _)| c.id == cert.id),
			"nor raised as running out"
		);
		assert!(
			ApplicationCertificate::get(&mut conn, cert.id)
				.await
				.expect("still there")
				.chain
				.is_some(),
			"and what was issued stays collectable until it expires"
		);
	})
	.await
}

/// The certificate path declares the name it orders for, and does so without
/// telling a device who else holds it — the same refusal a name nobody declares
/// gets, so the endpoint is not a directory of what other machines serve.
// spec: NAM#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn ordering_declares_the_name_without_naming_another_holder() {
	TestDb::run(|mut conn, _url| async move {
		let mine = insert_server(&mut conn, "orders").await;
		let theirs = insert_server(&mut conn, "the-holder").await;

		ApplicationCertificate::request(&mut conn, mine, "own.fiji.tamanu.app", "key-a", b"csr")
			.await
			.expect("order");
		let declared = ApplicationName::for_name(&mut conn, "own.fiji.tamanu.app")
			.await
			.expect("look up")
			.expect("ordering declared the name");
		assert_eq!(declared.application_id, mine);

		ApplicationName::declare(&mut conn, theirs, "elsewhere.fiji.tamanu.app")
			.await
			.expect("declare");
		let refusal = ApplicationCertificate::request(
			&mut conn,
			mine,
			"elsewhere.fiji.tamanu.app",
			"key-b",
			b"csr",
		)
		.await
		.expect_err("ordering for a name held elsewhere");
		assert!(
			matches!(
				&refusal,
				commons_errors::AppError::DnsNameUndeclared(m) if m == "elsewhere.fiji.tamanu.app"
			),
			"got {refusal:?}"
		);
		assert!(
			!refusal.to_string().contains("the-holder"),
			"and never names the holder"
		);
	})
	.await
}

/// A machine's undeclared records are bounded: past the bound a new DNS name is
/// not recorded, and one already held is still refreshed.
// spec: NAM#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn undeclared_records_are_bounded_per_machine() {
	use database::dns_name_dispositions::{AskedFor, UNDECLARED_PER_MACHINE, UndeclaredDnsName};
	TestDb::run(|mut conn, _url| async move {
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name) VALUES ('{machine}', 'box')"
		))
		.await
		.expect("machine");

		for i in 0..UNDECLARED_PER_MACHINE {
			UndeclaredDnsName::record(
				&mut conn,
				machine,
				&format!("n{i}.example.org"),
				AskedFor::Certificate,
			)
			.await
			.expect("record");
		}
		UndeclaredDnsName::record(
			&mut conn,
			machine,
			"overflow.example.org",
			AskedFor::Certificate,
		)
		.await
		.expect("past the bound is not an error");
		UndeclaredDnsName::record(&mut conn, machine, "n0.example.org", AskedFor::Addresses)
			.await
			.expect("refresh");

		let held = UndeclaredDnsName::for_machine(&mut conn, machine)
			.await
			.expect("list");
		assert_eq!(held.len() as i64, UNDECLARED_PER_MACHINE);
		assert!(held.iter().all(|r| r.dns_name != "overflow.example.org"));
		assert_eq!(
			held.iter()
				.find(|r| r.dns_name == "n0.example.org")
				.map(|r| r.asked_for),
			Some(AskedFor::Addresses)
		);
	})
	.await;
}

/// The monitor's prune drops records not asked about within the lifetime and
/// keeps the rest, whichever machine they belong to.
// spec: NAM#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn lapsed_undeclared_records_are_pruned() {
	use database::dns_name_dispositions::UndeclaredDnsName;
	TestDb::run(|mut conn, _url| async move {
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name) VALUES ('{machine}', 'box'); \
			 INSERT INTO undeclared_dns_names (machine_id, dns_name, asked_for, first_asked_at, last_asked_at) VALUES \
			   ('{machine}', 'lapsed.example.org', 'certificate', now() - interval '3 days', now() - interval '25 hours'), \
			   ('{machine}', 'fresh.example.org', 'addresses', now() - interval '3 days', now() - interval '1 hour')"
		))
		.await
		.expect("seed");

		assert_eq!(UndeclaredDnsName::prune(&mut conn).await.expect("prune"), 1);
		let held: Vec<String> = UndeclaredDnsName::for_machine(&mut conn, machine)
			.await
			.expect("list")
			.into_iter()
			.map(|r| r.dns_name)
			.collect();
		assert_eq!(held, vec!["fresh.example.org".to_string()]);
	})
	.await;
}
