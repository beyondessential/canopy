//! DNS addresses and TLS certificates are separate features that share
//! infrastructure (DNS): each has its own declarations, undeclared records and
//! denials, and a DNS name is held by one application across both.

use commons_tests::db::TestDb;
use database::diesel_async::AsyncPgConnection;
use database::{
	ApplicationCertificateName, ApplicationName, DeniedDnsName, DnsNameKind, UndeclaredDnsName,
};
use diesel::{sql_query, sql_types};
use diesel_async::{
	AsyncConnection as _, AsyncMigrationHarness, RunQueryDsl, SimpleAsyncConnection as _,
};
use diesel_migrations::MigrationHarness as _;
use uuid::Uuid;

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_machine(conn: &mut AsyncPgConnection) -> Uuid {
	sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result::<RowId>(conn)
		.await
		.expect("insert machine")
		.id
}

async fn insert_application(conn: &mut AsyncPgConnection, machine: Uuid, name: &str) -> Uuid {
	let host = format!("https://{}.example.invalid", Uuid::new_v4());
	sql_query(
		"INSERT INTO applications (name, host, type, machine_id) \
		 VALUES ($1, $2, 'tamanu-central', $3) RETURNING id",
	)
	.bind::<sql_types::Text, _>(name)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result::<RowId>(conn)
	.await
	.expect("insert application")
	.id
}

const NAME: &str = "shared.fiji.tamanu.app";

// ── Declarations ────────────────────────────────────────────────────────────

/// Declaring for certificates carries no addresses and no order, and is
/// independent of declaring for addresses.
// spec: DNS#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn a_name_is_declared_for_certificates_apart_from_addresses() {
	TestDb::run(async |mut conn, _url| {
		let machine = insert_machine(&mut conn).await;
		let app = insert_application(&mut conn, machine, "central").await;

		let row = ApplicationCertificateName::declare(&mut conn, app, "Front.Fiji.Tamanu.App.")
			.await
			.expect("declare");
		assert_eq!(
			row.name, "front.fiji.tamanu.app",
			"normalised on the way in"
		);
		assert!(
			ApplicationName::for_name(&mut conn, "front.fiji.tamanu.app")
				.await
				.expect("look up")
				.is_none(),
			"declaring for certificates does not declare for addresses"
		);

		let again = ApplicationCertificateName::declare(&mut conn, app, "front.fiji.tamanu.app")
			.await
			.expect("declaring again changes nothing");
		assert_eq!(again.id, row.id);

		ApplicationName::declare(&mut conn, app, "front.fiji.tamanu.app")
			.await
			.expect("the same application may hold the name for both kinds");
		assert_eq!(
			ApplicationCertificateName::for_application(&mut conn, app)
				.await
				.expect("list")
				.len(),
			1
		);
		assert_eq!(
			ApplicationName::for_server(&mut conn, app)
				.await
				.expect("list")
				.len(),
			1
		);
	})
	.await
}

/// One application holds a name across the fleet whichever kinds it is declared
/// for, in either direction, and the refusal names who holds it.
// spec: DNS#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn a_name_has_one_holder_across_both_kinds() {
	TestDb::run(async |mut conn, _url| {
		let machine = insert_machine(&mut conn).await;
		let holder = insert_application(&mut conn, machine, "the-holder").await;
		let other = insert_application(&mut conn, machine, "the-other").await;

		ApplicationName::declare(&mut conn, holder, NAME)
			.await
			.expect("held for addresses");
		let refusal = ApplicationCertificateName::declare(&mut conn, other, NAME)
			.await
			.expect_err("another application cannot hold it for certificates");
		assert!(
			refusal.to_string().contains("the-holder"),
			"the refusal names the holder, but said: {refusal}"
		);

		ApplicationName::release(&mut conn, holder, NAME)
			.await
			.expect("release");
		ApplicationCertificateName::declare(&mut conn, holder, NAME)
			.await
			.expect("held for certificates");
		let refusal = ApplicationName::declare(&mut conn, other, NAME)
			.await
			.expect_err("another application cannot hold it for addresses");
		assert!(refusal.to_string().contains("the-holder"));
		let refusal = ApplicationName::register(&mut conn, other, NAME, &[])
			.await
			.expect_err("nor register addresses for it");
		assert!(
			matches!(&refusal, commons_errors::AppError::DnsNameUndeclared(n) if n == NAME),
			"the device-facing path reads as undeclared, got {refusal:?}"
		);

		ApplicationCertificateName::release(&mut conn, holder, NAME)
			.await
			.expect("release for its last kind");
		ApplicationName::declare(&mut conn, other, NAME)
			.await
			.expect("free for another application once released for every kind");
	})
	.await
}

/// The database holds the rule itself, so a write that does not go through the
/// models cannot split a name between two applications either.
// spec: DNS#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn the_database_refuses_a_split_holder() {
	TestDb::run(async |mut conn, _url| {
		let machine = insert_machine(&mut conn).await;
		let one = insert_application(&mut conn, machine, "one").await;
		let two = insert_application(&mut conn, machine, "two").await;

		conn.batch_execute(&format!(
			"INSERT INTO application_certificate_names (application_id, name) VALUES ('{one}', '{NAME}')"
		))
		.await
		.expect("first");
		let split = conn
			.batch_execute(&format!(
				"INSERT INTO application_names (application_id, name) VALUES ('{two}', '{NAME}')"
			))
			.await;
		assert!(
			split.is_err(),
			"a second application holding it for addresses"
		);
		conn.batch_execute(&format!(
			"INSERT INTO application_names (application_id, name) VALUES ('{one}', '{NAME}')"
		))
		.await
		.expect("the holder may hold it for the other kind too");
	})
	.await
}

// ── Undeclared requests ─────────────────────────────────────────────────────

/// A machine asking about one DNS name both ways has two records, each of its
/// own kind, and each is ended on its own.
// spec: DNS#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn undeclared_requests_are_recorded_per_kind() {
	TestDb::run(async |mut conn, _url| {
		let machine = insert_machine(&mut conn).await;
		let app = insert_application(&mut conn, machine, "central").await;

		UndeclaredDnsName::record(&mut conn, machine, NAME, DnsNameKind::Certificate)
			.await
			.expect("record");
		UndeclaredDnsName::record(&mut conn, machine, NAME, DnsNameKind::Addresses)
			.await
			.expect("record");
		UndeclaredDnsName::record(&mut conn, machine, NAME, DnsNameKind::Addresses)
			.await
			.expect("a repeat updates the one record");

		let certificates =
			UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Certificate)
				.await
				.expect("list");
		let addresses = UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Addresses)
			.await
			.expect("list");
		assert_eq!((certificates.len(), addresses.len()), (1, 1));

		let counts = UndeclaredDnsName::counts_by_machine(&mut conn, None)
			.await
			.expect("counts");
		assert_eq!(counts.len(), 1);
		assert_eq!(
			(counts[0].addresses, counts[0].certificates),
			(1, 1),
			"the notice counts each kind"
		);

		ApplicationCertificateName::declare(&mut conn, app, NAME)
			.await
			.expect("declare for certificates");
		assert!(
			UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Certificate)
				.await
				.expect("list")
				.is_empty(),
			"declaring ends the record of its kind"
		);
		assert_eq!(
			UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Addresses)
				.await
				.expect("list")
				.len(),
			1,
			"and leaves the other kind's"
		);

		UndeclaredDnsName::clear(&mut conn, machine, NAME, DnsNameKind::Addresses)
			.await
			.expect("clear");
		assert!(
			UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Addresses)
				.await
				.expect("list")
				.is_empty()
		);
	})
	.await
}

// ── Denials ─────────────────────────────────────────────────────────────────

/// Denying addresses leaves certificates allowed and the reverse, and denying
/// both is two denials lifted separately.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn a_denial_is_of_one_kind() {
	TestDb::run(async |mut conn, _url| {
		let machine = insert_machine(&mut conn).await;

		UndeclaredDnsName::record(&mut conn, machine, NAME, DnsNameKind::Certificate)
			.await
			.expect("record");
		UndeclaredDnsName::record(&mut conn, machine, NAME, DnsNameKind::Addresses)
			.await
			.expect("record");

		DeniedDnsName::deny(
			&mut conn,
			machine,
			NAME,
			DnsNameKind::Certificate,
			"admin@localhost",
			Some("retired"),
		)
		.await
		.expect("deny certificates");

		assert!(
			DeniedDnsName::get(&mut conn, machine, NAME, DnsNameKind::Certificate)
				.await
				.expect("get")
				.is_some()
		);
		assert!(
			DeniedDnsName::get(&mut conn, machine, NAME, DnsNameKind::Addresses)
				.await
				.expect("get")
				.is_none(),
			"address requests are unaffected"
		);
		assert!(
			UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Certificate)
				.await
				.expect("list")
				.is_empty(),
			"denying ends the record of its kind"
		);
		assert_eq!(
			UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Addresses)
				.await
				.expect("list")
				.len(),
			1,
			"and leaves the other's"
		);

		DeniedDnsName::deny(
			&mut conn,
			machine,
			NAME,
			DnsNameKind::Addresses,
			"admin@localhost",
			None,
		)
		.await
		.expect("deny addresses");
		DeniedDnsName::lift(&mut conn, machine, NAME, DnsNameKind::Addresses)
			.await
			.expect("lift addresses");
		assert!(
			DeniedDnsName::get(&mut conn, machine, NAME, DnsNameKind::Certificate)
				.await
				.expect("get")
				.is_some(),
			"lifting one kind leaves the other denied"
		);
		let missing = DeniedDnsName::lift(&mut conn, machine, NAME, DnsNameKind::Addresses)
			.await
			.expect_err("nothing left to lift");
		assert!(missing.to_string().contains("not denied"));
	})
	.await
}

/// Declaring ends the denial of the kind declared, and a denial is refused while
/// the DNS name is declared for that kind but not while it is declared only for
/// the other.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn declaring_ends_only_its_own_kinds_denial() {
	TestDb::run(async |mut conn, _url| {
		let machine = insert_machine(&mut conn).await;
		let app = insert_application(&mut conn, machine, "central").await;

		for kind in [DnsNameKind::Addresses, DnsNameKind::Certificate] {
			DeniedDnsName::deny(&mut conn, machine, NAME, kind, "admin@localhost", None)
				.await
				.expect("deny");
		}

		ApplicationCertificateName::declare(&mut conn, app, NAME)
			.await
			.expect("declare for certificates");
		assert!(
			DeniedDnsName::get(&mut conn, machine, NAME, DnsNameKind::Certificate)
				.await
				.expect("get")
				.is_none(),
			"the certificate denial ends"
		);
		assert!(
			DeniedDnsName::get(&mut conn, machine, NAME, DnsNameKind::Addresses)
				.await
				.expect("get")
				.is_some(),
			"the address denial stands"
		);

		let refused = DeniedDnsName::deny(
			&mut conn,
			machine,
			NAME,
			DnsNameKind::Certificate,
			"admin@localhost",
			None,
		)
		.await
		.expect_err("declared for certificates on this machine");
		assert!(refused.to_string().contains("release it there"));
		DeniedDnsName::deny(
			&mut conn,
			machine,
			NAME,
			DnsNameKind::Addresses,
			"admin@localhost",
			Some("still allowed to deny: it is only declared for certificates"),
		)
		.await
		.expect("denying the other kind is not contradicted");
	})
	.await
}

// ── Migration ───────────────────────────────────────────────────────────────

const THIS_MIGRATION: &str = "202610090328490000";

/// Every declaration made before addresses and certificates were separate was
/// made for a certificate, since Canopy DNS was not in use; a row carrying
/// addresses is also an address declaration, and nothing is lost.
///
/// Replays it for real: reverts this migration, seeds the old shape, applies it
/// again.
// spec: DNS#declared-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn existing_declarations_denials_and_records_migrate() {
	TestDb::run(async |mut conn, url| {
		let machine = insert_machine(&mut conn).await;
		let bare = insert_application(&mut conn, machine, "bare").await;
		let served = insert_application(&mut conn, machine, "served").await;
		let withdrawing = insert_application(&mut conn, machine, "withdrawing").await;

		{
			let second = AsyncPgConnection::establish(&url)
				.await
				.expect("second connection");
			let mut harness = AsyncMigrationHarness::new(second);
			let latest = harness
				.applied_migrations()
				.expect("applied")
				.iter()
				.map(ToString::to_string)
				.max()
				.expect("some migration");
			assert_eq!(latest, THIS_MIGRATION, "this is the newest migration");
			harness
				.revert_last_migration(commons_tests::db::MIGRATIONS)
				.expect("revert");
		}

		conn.batch_execute(&format!(
			"INSERT INTO application_names (application_id, name, addresses, published_addresses) VALUES \
			   ('{bare}', 'bare.fiji.tamanu.app', '{{}}', '{{}}'), \
			   ('{served}', 'served.fiji.tamanu.app', '{{192.0.2.1/32}}', '{{192.0.2.1/32}}'), \
			   ('{withdrawing}', 'going.fiji.tamanu.app', '{{}}', '{{192.0.2.9/32}}'); \
			 INSERT INTO denied_dns_names (machine_id, dns_name, denied_by) VALUES \
			   ('{machine}', 'old.fiji.tamanu.app', 'someone'); \
			 INSERT INTO undeclared_dns_names (machine_id, dns_name, asked_for) VALUES \
			   ('{machine}', 'asked.fiji.tamanu.app', 'addresses'), \
			   ('{machine}', 'wanted.fiji.tamanu.app', 'certificate')"
		))
		.await
		.expect("seed the old shape");

		{
			let second = AsyncPgConnection::establish(&url)
				.await
				.expect("second connection");
			AsyncMigrationHarness::new(second)
				.run_pending_migrations(commons_tests::db::MIGRATIONS)
				.expect("re-apply");
		}

		let certificate_names: Vec<String> =
			ApplicationCertificateName::for_application(&mut conn, bare)
				.await
				.expect("list")
				.into_iter()
				.chain(
					ApplicationCertificateName::for_application(&mut conn, served)
						.await
						.expect("list"),
				)
				.chain(
					ApplicationCertificateName::for_application(&mut conn, withdrawing)
						.await
						.expect("list"),
				)
				.map(|r| r.name)
				.collect();
		assert_eq!(
			certificate_names,
			vec![
				"bare.fiji.tamanu.app",
				"served.fiji.tamanu.app",
				"going.fiji.tamanu.app"
			],
			"every declaration is a certificate declaration for the same application"
		);

		assert!(
			ApplicationName::for_server(&mut conn, bare)
				.await
				.expect("list")
				.is_empty(),
			"a declaration with no addresses leaves the address side"
		);
		assert_eq!(
			ApplicationName::for_server(&mut conn, served)
				.await
				.expect("list")
				.len(),
			1,
			"a row with addresses stays an address declaration"
		);
		assert_eq!(
			ApplicationName::for_server(&mut conn, withdrawing)
				.await
				.expect("list")
				.len(),
			1,
			"as does one still withdrawing records it published, so they are removed"
		);

		let denied = DeniedDnsName::for_machine(&mut conn, machine, DnsNameKind::Certificate)
			.await
			.expect("list");
		assert_eq!(denied.len(), 1, "the denial becomes a certificate one");
		assert!(
			DeniedDnsName::for_machine(&mut conn, machine, DnsNameKind::Addresses)
				.await
				.expect("list")
				.is_empty()
		);

		let asked = UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Addresses)
			.await
			.expect("list");
		let wanted = UndeclaredDnsName::for_machine(&mut conn, machine, DnsNameKind::Certificate)
			.await
			.expect("list");
		assert_eq!(
			(
				asked
					.iter()
					.map(|r| r.dns_name.as_str())
					.collect::<Vec<_>>(),
				wanted
					.iter()
					.map(|r| r.dns_name.as_str())
					.collect::<Vec<_>>()
			),
			(
				vec!["asked.fiji.tamanu.app"],
				vec!["wanted.fiji.tamanu.app"]
			),
			"undeclared records keep the kind they carried"
		);
	})
	.await
}
