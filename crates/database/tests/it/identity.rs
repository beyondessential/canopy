//! The identity provider's tables (spec `IDP`): a grant names exactly one
//! subject, and goes with its relying party or its subject.

use commons_tests::diesel_async::SimpleAsyncConnection;
use database::identity::{self, GrantSubject, RelyingParty};
use database::machines::{Machine, NewMachine};

#[tokio::test(flavor = "multi_thread")]
async fn a_grant_names_exactly_one_subject() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let party = RelyingParty::add(&mut conn, "A", "https://a.example", None)
			.await
			.unwrap();
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		assert!(
			conn.batch_execute(&format!(
				"INSERT INTO identity_grants (relying_party_id) VALUES ('{}')",
				party.id
			))
			.await
			.is_err()
		);
		identity::set_grants(
			&mut conn,
			GrantSubject::Machine(machine.id),
			&[party.id, party.id],
		)
		.await
		.unwrap();
		assert!(
			conn.batch_execute(&format!(
				"INSERT INTO identity_grants (relying_party_id, machine_id) VALUES ('{}', '{}')",
				party.id, machine.id
			))
			.await
			.is_err(),
			"a subject holds one grant per relying party"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_go_with_their_relying_party() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let party = RelyingParty::add(&mut conn, "A", "https://a.example", None)
			.await
			.unwrap();
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let subject = GrantSubject::Machine(machine.id);
		identity::set_grants(&mut conn, subject, &[party.id])
			.await
			.unwrap();

		let (_, revoked) = RelyingParty::remove(&mut conn, party.id).await.unwrap();
		assert_eq!(revoked, [subject]);
		assert!(identity::grants_of(&mut conn, &[subject]).await.unwrap()[&subject].is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_key_s_first_moments_are_written_once() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let early = jiff::Timestamp::from_second(1_800_000_000).unwrap();
		identity::IdentityKey::observe_served(&mut conn, "k", "ref", early)
			.await
			.unwrap();
		identity::IdentityKey::observe_served(&mut conn, "k", "ref", jiff::Timestamp::now())
			.await
			.unwrap();
		let keys = identity::IdentityKey::all(&mut conn).await.unwrap();
		assert_eq!(keys["k"].first_served_at, Some(early));
		assert!(
			conn.batch_execute("UPDATE identity_keys SET first_served_at = now() WHERE kid = 'k'")
				.await
				.is_err()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_discovery_alert_raises_and_clears() {
	use database::self_alerts::{
		IDENTITY_DISCOVERY_REF, IssuerDocuments, current, sweep_identity_discovery,
	};

	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let configured = vec!["a".to_owned()];
		sweep_identity_discovery(
			&mut conn,
			"https://i.example",
			&IssuerDocuments::Unreachable("refused".into()),
			&configured,
			Some("a"),
		)
		.await
		.unwrap();
		assert!(
			current(&mut conn, IDENTITY_DISCOVERY_REF)
				.await
				.unwrap()
				.unwrap()
				.active
		);

		sweep_identity_discovery(
			&mut conn,
			"https://i.example",
			&IssuerDocuments::Served(["a".to_owned()].into()),
			&configured,
			Some("a"),
		)
		.await
		.unwrap();
		assert!(
			!current(&mut conn, IDENTITY_DISCOVERY_REF)
				.await
				.unwrap()
				.unwrap()
				.active
		);
	})
	.await;
}
