//! The alerting for names and certificates: what gets filed against a server,
//! what gets filed against Canopy, and — the part that is easy to get wrong —
//! what gets closed again when the condition clears.

use commons_types::dns::ManagedZone;
use commons_types::status::CheckResult;
use database::application_certificates::ApplicationCertificate;
use database::certificate_alerts::{
	ADDRESS_REF, CANOPY_SOURCE, EXPIRY_REF, ISSUANCE_REF, STUCK_AFTER_ATTEMPTS,
};
use database::diesel_async::AsyncPgConnection;
use database::issues::Issue;
use database::self_alerts::{self, FORGOTTEN_PAUSE_REF};
use database::{ApplicationName, ServerGroupDomain};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
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

/// A server with the grants, in a group that controls `domain`.
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

	let machine =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Uuid, _>(group)
			.get_result::<RowId>(conn)
			.await
			.expect("insert machine")
			.id;

	let host = format!("https://{}.example.invalid", Uuid::new_v4());
	sql_query(
		"INSERT INTO applications \
		 (name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
		 VALUES ($1, $2, 'tamanu-central', $3, true, true, $4) RETURNING id",
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

/// Record a certificate as issued with a chosen lifetime, so a test can hold one
/// at any point in its life without waiting.
async fn issue_aged(
	conn: &mut AsyncPgConnection,
	id: Uuid,
	lifetime: SignedDuration,
	elapsed: SignedDuration,
) {
	let issued_at = Timestamp::now() - elapsed;
	let not_after = issued_at + lifetime;
	ApplicationCertificate::record_issued(
		conn,
		id,
		"-----BEGIN CERTIFICATE-----\n",
		not_after,
		None,
		None,
	)
	.await
	.expect("record issued");
	// `record_issued` stamps issuance as now, which would make every certificate
	// look brand new; the age is the whole point here. The renewal point is
	// derived from issuance, so it moves with it — otherwise a certificate could
	// be five-sixths through its life with a renewal date still in the future,
	// which nothing in production produces.
	sql_query("UPDATE application_certificates SET issued_at = $2, renew_after = $3 WHERE id = $1")
		.bind::<sql_types::Uuid, _>(id)
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(issued_at))
		.bind::<sql_types::Timestamptz, _>(jiff_diesel::Timestamp::from(
			database::application_certificates::default_renew_after(issued_at, not_after),
		))
		.execute(conn)
		.await
		.expect("backdate issuance");
}

async fn server_issue(conn: &mut AsyncPgConnection, server_id: Uuid, check: &str) -> Option<Issue> {
	Issue::list_by_source_ref(conn, CANOPY_SOURCE, check, &[server_id])
		.await
		.expect("read issues")
		.into_iter()
		.next()
}

// ── Certificates running out ────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_past_renewal_warns_and_one_nearly_gone_fails() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		// Ninety days long, seventy gone: past the third-remaining renewal point,
		// with room left.
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(70 * 24),
		)
		.await;
		database::certificate_alerts::sweep_certificate_expiry(&mut conn)
			.await
			.expect("sweep");

		let issue = server_issue(&mut conn, server, EXPIRY_REF)
			.await
			.expect("filed");
		assert!(issue.active);
		assert_eq!(issue.observed_result, Some(CheckResult::Warning));
		assert!(
			issue.message.contains("a.fiji.tamanu.app"),
			"the message names the certificate: {}",
			issue.message
		);

		// Eighty-eight days gone of ninety: past the sixth-remaining line.
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(88 * 24),
		)
		.await;
		database::certificate_alerts::sweep_certificate_expiry(&mut conn)
			.await
			.expect("sweep");

		let issue = server_issue(&mut conn, server, EXPIRY_REF)
			.await
			.expect("still filed");
		assert_eq!(issue.observed_result, Some(CheckResult::Failed));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_renewed_certificate_closes_the_alert_it_raised() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(85 * 24),
		)
		.await;
		database::certificate_alerts::sweep_certificate_expiry(&mut conn)
			.await
			.expect("sweep");
		assert!(
			server_issue(&mut conn, server, EXPIRY_REF)
				.await
				.expect("filed")
				.active
		);

		// The renewal comes through.
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(1),
		)
		.await;
		database::certificate_alerts::sweep_certificate_expiry(&mut conn)
			.await
			.expect("sweep");

		assert!(
			!server_issue(&mut conn, server, EXPIRY_REF)
				.await
				.expect("still present")
				.active,
			"an alert nothing can act on is worse than none"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn one_check_per_server_covers_every_certificate_of_its() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		for (name, key) in [("a.fiji.tamanu.app", KEY_A), ("b.fiji.tamanu.app", KEY_B)] {
			let cert = ApplicationCertificate::request(&mut conn, server, name, key, b"csr")
				.await
				.expect("request");
			issue_aged(
				&mut conn,
				cert.id,
				SignedDuration::from_hours(90 * 24),
				SignedDuration::from_hours(85 * 24),
			)
			.await;
		}

		let filed = database::certificate_alerts::sweep_certificate_expiry(&mut conn)
			.await
			.expect("sweep");
		assert_eq!(filed, 1, "one filing, not one per certificate");

		let issue = server_issue(&mut conn, server, EXPIRY_REF)
			.await
			.expect("filed");
		assert!(issue.message.contains("a.fiji.tamanu.app"));
		assert!(issue.message.contains("b.fiji.tamanu.app"));
		assert!(
			issue.message.starts_with("2 certificate(s)"),
			"got {}",
			issue.message
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_the_server_lost_raises_nothing_however_far_past_expiry() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(120 * 24),
		)
		.await;

		let claim = ServerGroupDomain::controlling(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read")
			.expect("present");
		ServerGroupDomain::release(&mut conn, claim.id)
			.await
			.expect("release");

		assert_eq!(
			database::certificate_alerts::sweep_certificate_expiry(&mut conn)
				.await
				.expect("sweep"),
			0,
			"Canopy stopped renewing it on purpose, so its running out is not a fault"
		);
		assert!(server_issue(&mut conn, server, EXPIRY_REF).await.is_none());
	})
	.await;
}

// ── Issuance that never came up ─────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_first_issuance_that_keeps_failing_is_reported_separately() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");

		for _ in 0..STUCK_AFTER_ATTEMPTS {
			ApplicationCertificate::record_failure(&mut conn, cert.id, "the zone would not answer")
				.await
				.expect("record failure");
		}

		database::certificate_alerts::sweep_stuck_issuance(&mut conn)
			.await
			.expect("sweep");

		let issue = server_issue(&mut conn, server, ISSUANCE_REF)
			.await
			.expect("filed");
		assert!(issue.active);
		assert_eq!(issue.observed_result, Some(CheckResult::Failed));
		assert!(
			issue.message.contains("the zone would not answer"),
			"the recorded reason is carried through: {}",
			issue.message
		);
		// And it is not the expiry check: a server that never came up wants a
		// different response from one about to go dark.
		assert!(server_issue(&mut conn, server, EXPIRY_REF).await.is_none());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_few_failed_attempts_are_waited_out_rather_than_reported() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		ApplicationCertificate::record_failure(&mut conn, cert.id, "not visible yet")
			.await
			.expect("record failure");

		assert_eq!(
			database::certificate_alerts::sweep_stuck_issuance(&mut conn)
				.await
				.expect("sweep"),
			0,
			"a challenge record not yet visible is the normal case, not an alert"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_issuance_that_finally_succeeds_closes_its_alert() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		for _ in 0..STUCK_AFTER_ATTEMPTS {
			ApplicationCertificate::record_failure(&mut conn, cert.id, "no")
				.await
				.expect("record failure");
		}
		database::certificate_alerts::sweep_stuck_issuance(&mut conn)
			.await
			.expect("sweep");
		assert!(
			server_issue(&mut conn, server, ISSUANCE_REF)
				.await
				.expect("filed")
				.active
		);

		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(1),
		)
		.await;
		database::certificate_alerts::sweep_stuck_issuance(&mut conn)
			.await
			.expect("sweep");

		assert!(
			!server_issue(&mut conn, server, ISSUANCE_REF)
				.await
				.expect("present")
				.active
		);
	})
	.await;
}

// ── Address records ─────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn records_that_could_not_be_published_are_reported_and_then_closed() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let addr: IpAddr = "192.0.2.1".parse().expect("address");
		let row = ApplicationName::register(&mut conn, server, "a.fiji.tamanu.app", &[addr])
			.await
			.expect("register");

		// Not yet published and no failure recorded: the reconcile has simply not
		// run, which is not something to alert an operator about.
		assert_eq!(
			database::certificate_alerts::sweep_address_records(&mut conn)
				.await
				.expect("sweep"),
			0
		);

		ApplicationName::record_publish_error(&mut conn, row.id, "route53 refused the change")
			.await
			.expect("record error");
		database::certificate_alerts::sweep_address_records(&mut conn)
			.await
			.expect("sweep");

		let issue = server_issue(&mut conn, server, ADDRESS_REF)
			.await
			.expect("filed");
		assert!(issue.active);
		assert!(
			issue.message.contains("route53 refused the change"),
			"got {}",
			issue.message
		);

		ApplicationName::record_published(&mut conn, row.id, &[addr])
			.await
			.expect("record published");
		database::certificate_alerts::sweep_address_records(&mut conn)
			.await
			.expect("sweep");

		assert!(
			!server_issue(&mut conn, server, ADDRESS_REF)
				.await
				.expect("present")
				.active
		);
	})
	.await;
}

// ── The forgotten pause ─────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_lapsing_under_a_pause_is_canopys_to_report() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(85 * 24),
		)
		.await;
		database::applications::Application::pause_name_management(
			&mut conn,
			server,
			Some("operator@example.test"),
			"looking into a suspected key leak",
		)
		.await
		.expect("pause");

		// The per-server alerting goes quiet, which is the point of a pause…
		assert_eq!(
			database::certificate_alerts::sweep_certificate_expiry(&mut conn)
				.await
				.expect("sweep"),
			0
		);

		// …so the pause is what becomes visible, against Canopy.
		let raised = self_alerts::sweep_forgotten_pauses(&mut conn)
			.await
			.expect("sweep")
			.expect("raised");
		assert_eq!(raised.observed_result, Some(CheckResult::Warning));
		assert_eq!(raised.r#ref, FORGOTTEN_PAUSE_REF);
		assert!(
			raised.application_id.is_none(),
			"canopy-wide, not per-server"
		);
		assert!(
			raised.message.contains("a.fiji.tamanu.app")
				&& raised.message.contains("suspected key leak"),
			"the message says what is lapsing and why it was paused: {}",
			raised.message
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_expired_under_a_pause_is_a_fault_rather_than_a_nudge() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(100 * 24),
		)
		.await;
		database::applications::Application::pause_name_management(
			&mut conn,
			server,
			None,
			"investigating",
		)
		.await
		.expect("pause");

		let raised = self_alerts::sweep_forgotten_pauses(&mut conn)
			.await
			.expect("sweep")
			.expect("raised");
		assert_eq!(raised.observed_result, Some(CheckResult::Failed));
		assert!(raised.message.contains("expired"), "got {}", raised.message);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn lifting_the_pause_recovers_the_self_alert() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(85 * 24),
		)
		.await;
		database::applications::Application::pause_name_management(
			&mut conn,
			server,
			None,
			"investigating",
		)
		.await
		.expect("pause");
		self_alerts::sweep_forgotten_pauses(&mut conn)
			.await
			.expect("sweep")
			.expect("raised");

		database::applications::Application::resume_name_management(&mut conn, server)
			.await
			.expect("resume");
		self_alerts::sweep_forgotten_pauses(&mut conn)
			.await
			.expect("sweep");

		let current = self_alerts::current(&mut conn, FORGOTTEN_PAUSE_REF)
			.await
			.expect("read")
			.expect("present");
		assert!(!current.active);

		// And the per-server alerting takes over again, since the certificate is
		// still running out — the pause was suppressing it, not fixing it.
		database::certificate_alerts::sweep_certificate_expiry(&mut conn)
			.await
			.expect("sweep");
		assert!(
			server_issue(&mut conn, server, EXPIRY_REF)
				.await
				.expect("filed")
				.active
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pause_over_a_healthy_certificate_raises_nothing() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(1),
		)
		.await;
		database::applications::Application::pause_name_management(
			&mut conn,
			server,
			None,
			"brief look",
		)
		.await
		.expect("pause");

		assert!(
			self_alerts::sweep_forgotten_pauses(&mut conn)
				.await
				.expect("sweep")
				.is_none(),
			"a pause is not itself a problem; forgetting one is"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_the_server_lost_does_not_count_against_a_pause() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = entitled_server(&mut conn, "fiji.tamanu.app").await;
		let cert =
			ApplicationCertificate::request(&mut conn, server, "a.fiji.tamanu.app", KEY_A, b"csr")
				.await
				.expect("request");
		issue_aged(
			&mut conn,
			cert.id,
			SignedDuration::from_hours(90 * 24),
			SignedDuration::from_hours(120 * 24),
		)
		.await;
		database::applications::Application::pause_name_management(
			&mut conn,
			server,
			None,
			"investigating",
		)
		.await
		.expect("pause");

		let claim = ServerGroupDomain::controlling(&mut conn, "a.fiji.tamanu.app")
			.await
			.expect("read")
			.expect("present");
		ServerGroupDomain::release(&mut conn, claim.id)
			.await
			.expect("release");

		assert!(
			self_alerts::sweep_forgotten_pauses(&mut conn)
				.await
				.expect("sweep")
				.is_none(),
			"the pause is not why this certificate is running out"
		);
	})
	.await;
}
