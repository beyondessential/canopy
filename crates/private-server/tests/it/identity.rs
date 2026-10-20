//! The identity provider's settings and grants (spec `IDP`), and what they
//! record in the audit log.

use axum::http::StatusCode;
use commons_tests::{
	diesel_async::{AsyncPgConnection, SimpleAsyncConnection},
	server::{IDENTITY_ISSUER, identity_key},
};
use database::{
	audit::{self, Entry, Filter, Retention},
	identity::{self, GrantSubject, IdentityKey},
};
use serde_json::{Value, json};

const GROUP: &str = "eeeeeeee-0000-0000-0000-000000000001";
const BOX: &str = "eeeeeeee-0000-0000-0000-0000000000b1";
const APP: &str = "eeeeeeee-0000-0000-0000-0000000000a1";

async fn seed(conn: &mut AsyncPgConnection, rank: Option<&str>) {
	let rank = rank.map(|r| format!("'{r}'")).unwrap_or("NULL".into());
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'Fiji');
		 INSERT INTO machines (name, id, group_id, rank) VALUES ('box', '{BOX}', '{GROUP}', {rank});
		 INSERT INTO applications (id, name, type, group_id, machine_id) VALUES
			('{APP}', 'central', 'tamanu-central', '{GROUP}', '{BOX}');"
	))
	.await
	.unwrap();
}

async fn entries(conn: &mut AsyncPgConnection, action: &str) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some(action.into()),
			limit: 100,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

async fn add(private: &commons_tests::axum_test::TestServer, audience: &str) -> Value {
	let response = private
		.post("/api/identity/add_relying_party")
		.json(&json!({ "name": "Anthropic", "audience": audience }))
		.await;
	response.assert_status_ok();
	response.json()
}

#[tokio::test(flavor = "multi_thread")]
async fn registering_editing_and_removing_a_relying_party_are_recorded() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn, Some("production")).await;
		let party = add(&private, "https://api.anthropic.example").await;
		assert_eq!(party["token_lifetime_minutes"], 5);
		let id = party["id"].as_str().unwrap();

		let registered = entries(&mut conn, "relying_party_registered").await;
		assert_eq!(registered.len(), 1);
		assert_eq!(registered[0].retention, Retention::Year);
		assert_eq!(registered[0].records[0].id, id);

		private
			.post("/api/identity/edit_relying_party")
			.json(&json!({ "id": id, "name": "Claude", "token_lifetime_minutes": 10 }))
			.await
			.assert_status_ok();
		let edited = entries(&mut conn, "relying_party_edited").await;
		assert_eq!(edited.len(), 1);
		assert_eq!(edited[0].detail["name_from"], "Anthropic");
		assert_eq!(edited[0].detail["name_to"], "Claude");
		assert_eq!(edited[0].detail["token_lifetime_minutes_from"], 5);
		assert_eq!(edited[0].detail["token_lifetime_minutes_to"], 10);

		// Saved again as it is: succeeds, and records nothing.
		private
			.post("/api/identity/edit_relying_party")
			.json(&json!({ "id": id, "name": "Claude", "token_lifetime_minutes": 10 }))
			.await
			.assert_status_ok();
		assert_eq!(entries(&mut conn, "relying_party_edited").await.len(), 1);

		private
			.post("/api/identity/set_grants")
			.json(
				&json!({ "subject": { "kind": "machine", "id": BOX }, "relying_party_ids": [id] }),
			)
			.await
			.assert_status_ok();
		private
			.post("/api/identity/set_grants")
			.json(
				&json!({ "subject": { "kind": "application", "id": APP }, "relying_party_ids": [id] }),
			)
			.await
			.assert_status_ok();

		private
			.post("/api/identity/remove_relying_party")
			.json(&json!({ "id": id }))
			.await
			.assert_status_ok();
		let removed = entries(&mut conn, "relying_party_removed").await;
		assert_eq!(removed.len(), 1);
		assert_eq!(removed[0].detail["grants_revoked"], 2);
		assert_eq!(entries(&mut conn, "identity_grant_revoked").await.len(), 2);
		assert!(
			identity::grants_of(&mut conn, &[GrantSubject::Machine(BOX.parse().unwrap())])
				.await
				.unwrap()
				.values()
				.all(Vec::is_empty)
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn relying_party_registration_is_checked() {
	commons_tests::server::run(async |_conn, _, private| {
		add(&private, "https://aud.example").await;
		private
			.post("/api/identity/add_relying_party")
			.json(&json!({ "name": "Again", "audience": "https://aud.example" }))
			.await
			.assert_status(StatusCode::CONFLICT);
		for minutes in [0, 61] {
			private
				.post("/api/identity/add_relying_party")
				.json(&json!({
					"name": "Bounds",
					"audience": format!("https://bounds-{minutes}.example"),
					"token_lifetime_minutes": minutes,
				}))
				.await
				.assert_status(StatusCode::BAD_REQUEST);
		}
		for minutes in [1, 60] {
			private
				.post("/api/identity/add_relying_party")
				.json(&json!({
					"name": "Bounds",
					"audience": format!("https://bounds-{minutes}.example"),
					"token_lifetime_minutes": minutes,
				}))
				.await
				.assert_status_ok();
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_audience_is_fixed_once_registered() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let party = identity::RelyingParty::add(&mut conn, "A", "https://a.example", None)
			.await
			.unwrap();
		let changed = conn
			.batch_execute(&format!(
				"UPDATE relying_parties SET audience = 'https://b.example' WHERE id = '{}'",
				party.id
			))
			.await;
		assert!(changed.is_err());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_are_set_per_subject_and_an_unchanged_save_records_nothing() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn, Some("production")).await;
		let first = add(&private, "https://one.example").await;
		let second = add(&private, "https://two.example").await;
		let (first, second) = (
			first["id"].as_str().unwrap(),
			second["id"].as_str().unwrap(),
		);

		let set = async |ids: Vec<&str>| {
			private
				.post("/api/identity/set_grants")
				.json(
					&json!({ "subject": { "kind": "machine", "id": BOX }, "relying_party_ids": ids }),
				)
				.await
				.assert_status_ok();
		};
		set(vec![first, second]).await;
		assert_eq!(entries(&mut conn, "identity_granted").await.len(), 2);
		set(vec![first, second]).await;
		assert_eq!(entries(&mut conn, "identity_granted").await.len(), 2);
		set(vec![second]).await;
		let revoked = entries(&mut conn, "identity_grant_revoked").await;
		assert_eq!(revoked.len(), 1);
		assert_eq!(revoked[0].retention, Retention::Year);
		let acted_on: Vec<&str> = revoked[0].records.iter().map(|r| r.id.as_str()).collect();
		assert!(acted_on.contains(&BOX));
		assert!(acted_on.contains(&first));

		let grants: Value = private
			.post("/api/identity/machine_grants")
			.json(&json!({ "machine_id": BOX }))
			.await
			.json();
		assert_eq!(grants["offered"], true);
		let subjects = grants["subjects"].as_array().unwrap();
		assert_eq!(subjects.len(), 2);
		assert_eq!(
			subjects[0]["subject"],
			json!({ "kind": "machine", "id": BOX })
		);
		assert_eq!(subjects[0]["granted"], json!([second]));
		assert_eq!(
			subjects[0]["sub"],
			format!("group:{GROUP}:rank:production:machine:{BOX}:")
		);
		assert_eq!(subjects[1]["granted"], json!([]));
		assert_eq!(
			subjects[1]["sub"],
			format!("group:{GROUP}:rank:production:application:tamanu-central:{APP}:")
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pending_machine_is_not_offered_the_choice() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn, None).await;
		add(&private, "https://one.example").await;
		let grants: Value = private
			.post("/api/identity/machine_grants")
			.json(&json!({ "machine_id": BOX }))
			.await
			.json();
		assert_eq!(grants["offered"], false);
		assert!(grants["subjects"][0]["sub"].is_null());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ungrouped_machine_is_not_offered_the_choice() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn, Some("production")).await;
		conn.batch_execute(&format!(
			"UPDATE machines SET group_id = NULL WHERE id = '{BOX}'"
		))
		.await
		.unwrap();
		add(&private, "https://one.example").await;
		let grants: Value = private
			.post("/api/identity/machine_grants")
			.json(&json!({ "machine_id": BOX }))
			.await
			.json();
		assert_eq!(grants["offered"], false);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn with_no_relying_party_the_choice_is_not_offered() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn, Some("production")).await;
		let grants: Value = private
			.post("/api/identity/machine_grants")
			.json(&json!({ "machine_id": BOX }))
			.await
			.json();
		assert_eq!(grants["offered"], false);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn status_presents_the_issuer_and_each_key_with_its_usage() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, private| {
			conn.batch_execute(&format!(
				"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'Fiji');
				 INSERT INTO machines (name, id, group_id, rank, device_id)
					VALUES ('box', '{BOX}', '{GROUP}', 'production', '{device_id}');"
			))
			.await
			.unwrap();
			let key = identity_key();
			IdentityKey::observe_served(&mut conn, key.kid(), &key.key_ref, jiff::Timestamp::now())
				.await
				.unwrap();
			let party = add(&private, "https://api.anthropic.example").await;
			identity::set_grants(
				&mut conn,
				GrantSubject::Machine(BOX.parse().unwrap()),
				&[party["id"].as_str().unwrap().parse().unwrap()],
			)
			.await
			.unwrap();
			public
				.post("/identity-tokens")
				.add_header("x-forwarded-client-cert", format!("Cert={cert}"))
				.json(&json!({ "audience": "https://api.anthropic.example" }))
				.await
				.assert_status_ok();

			let status: Value = private
				.post("/api/identity/status")
				.json(&json!({}))
				.await
				.json();
			assert_eq!(status["issuer"], IDENTITY_ISSUER);
			assert_eq!(
				status["discovery_url"],
				format!("{IDENTITY_ISSUER}/.well-known/openid-configuration")
			);
			assert_eq!(status["issues_tokens"], true);
			let keys = status["keys"].as_array().unwrap();
			assert_eq!(keys.len(), 1);
			assert_eq!(keys[0]["kid"], key.kid());
			assert_eq!(keys[0]["state"], "signing");
			assert_eq!(keys[0]["signed_30d"], 1);
			assert!(keys[0]["last_signed_at"].is_string());
			assert!(keys[0]["first_served_at"].is_string());

			let parties: Value = private
				.post("/api/identity/relying_parties")
				.json(&json!({}))
				.await
				.json();
			assert_eq!(parties[0]["machine_grants"], 1);
			assert_eq!(parties[0]["application_grants"], 0);
			assert!(parties[0]["last_issued_at"].is_string());
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_relying_party_with_no_recent_issuance_shows_none() {
	commons_tests::server::run(async |_conn, _, private| {
		add(&private, "https://quiet.example").await;
		let parties: Value = private
			.post("/api/identity/relying_parties")
			.json(&json!({}))
			.await
			.json();
		assert!(parties[0]["last_issued_at"].is_null());
		assert_eq!(parties[0]["machine_grants"], 0);
		assert_eq!(parties[0]["application_grants"], 0);
	})
	.await;
}
