//! Identity tokens (spec `IDT`): what a machine is issued, what it is refused
//! and in what order, and what the audit log keeps of both.

use axum::http::StatusCode;
use axum_test::TestServer;
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use commons_tests::server::{IDENTITY_ISSUER, identity_key};
use database::{
	audit::{self, Actor, Entry, Filter, Status},
	identity::{self, GrantSubject, IdentityKey, RelyingParty},
};
use diesel::{sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use ring::signature::{ECDSA_P256_SHA256_FIXED, UnparsedPublicKey};
use serde_json::{Value, json};
use uuid::Uuid;

const AUDIENCE: &str = "https://api.anthropic.example";

/// A group, and a production machine in it bound to the authenticated device,
/// with a Tamanu central on it.
struct Placed {
	group: Uuid,
	machine: Uuid,
	application: Uuid,
}

async fn placed(conn: &mut AsyncPgConnection, device_id: Uuid, rank: Option<&str>) -> Placed {
	let group = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'Fiji')")
		.bind::<sql_types::Uuid, _>(group)
		.execute(conn)
		.await
		.expect("insert group");
	let machine = Uuid::new_v4();
	sql_query(
		"INSERT INTO machines (name, id, group_id, device_id, rank) VALUES ('box', $1, $2, $3, $4)",
	)
	.bind::<sql_types::Uuid, _>(machine)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Uuid, _>(device_id)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rank)
	.execute(conn)
	.await
	.expect("insert machine");
	let application = Uuid::new_v4();
	sql_query(
		"INSERT INTO applications (id, name, type, group_id, machine_id) \
		 VALUES ($1, 'central', 'tamanu-central', $2, $3)",
	)
	.bind::<sql_types::Uuid, _>(application)
	.bind::<sql_types::Uuid, _>(group)
	.bind::<sql_types::Uuid, _>(machine)
	.execute(conn)
	.await
	.expect("insert application");
	Placed {
		group,
		machine,
		application,
	}
}

async fn party(conn: &mut AsyncPgConnection, lifetime: Option<i32>) -> RelyingParty {
	RelyingParty::add(conn, "Anthropic", AUDIENCE, lifetime)
		.await
		.expect("register relying party")
}

async fn grant(conn: &mut AsyncPgConnection, subject: GrantSubject, party: &RelyingParty) {
	identity::set_grants(conn, subject, &[party.id])
		.await
		.expect("grant");
}

async fn ask(public: &TestServer, cert: &str, body: Value) -> axum_test::TestResponse {
	public
		.post("/identity-tokens")
		.add_header("x-forwarded-client-cert", format!("Cert={cert}"))
		.json(&body)
		.await
}

fn problem_type(body: &Value) -> String {
	body["type"]
		.as_str()
		.unwrap_or_default()
		.trim_start_matches("/errors/")
		.to_string()
}

async fn issuances(conn: &mut AsyncPgConnection) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some("identity_token_issued".into()),
			limit: 100,
			..Default::default()
		},
	)
	.await
	.expect("read the audit log")
}

/// The token's claims, having checked its signature against the key the test
/// server signs with.
fn verified_claims(token: &str) -> Value {
	let key = identity_key();
	let (signed, signature) = token.rsplit_once('.').expect("a signature");
	let mut point = vec![0x04];
	point.extend(URL_SAFE_NO_PAD.decode(&key.jwk.x).unwrap());
	point.extend(URL_SAFE_NO_PAD.decode(&key.jwk.y).unwrap());
	UnparsedPublicKey::new(&ECDSA_P256_SHA256_FIXED, point)
		.verify(
			signed.as_bytes(),
			&URL_SAFE_NO_PAD.decode(signature).unwrap(),
		)
		.expect("the token verifies against the signing key");

	let mut parts = signed.split('.');
	let header: Value =
		serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts.next().unwrap()).unwrap()).unwrap();
	assert_eq!(header["alg"], "ES256");
	assert_eq!(header["kid"], key.kid());
	serde_json::from_slice(&URL_SAFE_NO_PAD.decode(parts.next().unwrap()).unwrap()).unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_granted_machine_is_issued_a_token_naming_itself() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			let party = party(&mut conn, None).await;
			grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;

			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status_ok();
			let body: Value = response.json();
			let claims = verified_claims(body["token"].as_str().unwrap());

			let sub = format!("group:{}:rank:production:machine:{}:", at.group, at.machine);
			assert_eq!(body["subject"], sub);
			assert_eq!(claims["sub"], sub);
			assert_eq!(claims["iss"], IDENTITY_ISSUER);
			assert_eq!(claims["aud"], AUDIENCE);
			assert_eq!(claims["group_id"], at.group.to_string());
			assert_eq!(claims["group_name"], "Fiji");
			assert_eq!(claims["rank"], "production");
			assert_eq!(claims["machine_id"], at.machine.to_string());
			assert!(claims.get("application_id").is_none());
			assert!(claims.get("application_type").is_none());
			assert_eq!(claims["iat"], claims["nbf"]);
			assert_eq!(
				claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
				5 * 60,
				"the default lifetime is five minutes"
			);

			let entries = issuances(&mut conn).await;
			assert_eq!(entries.len(), 1);
			let entry = &entries[0];
			assert_eq!(entry.status, Status::Done);
			assert!(matches!(
				entry.actor,
				Actor::Device { id, application: None, .. } if id == device_id
			));
			assert_eq!(entry.detail["jti"], claims["jti"]);
			assert_eq!(entry.detail["kid"], identity_key().kid());
			assert_eq!(entry.detail["sub"], sub);
			assert!(entry.detail["expires_at"].is_string());
			let acted_on: Vec<String> = entry.records.iter().map(|r| r.id.clone()).collect();
			assert!(acted_on.contains(&at.machine.to_string()));
			assert!(acted_on.contains(&party.id.to_string()));
			assert!(
				!entry
					.detail
					.to_string()
					.contains(body["token"].as_str().unwrap()),
				"the token itself is never recorded"
			);

			let signed = IdentityKey::all(&mut conn).await.unwrap();
			assert!(
				signed[identity_key().kid()].first_signed_at.is_some(),
				"the key's first signature is recorded"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_application_token_names_the_application() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			let party = party(&mut conn, Some(10)).await;
			grant(&mut conn, GrantSubject::Application(at.application), &party).await;

			let response = ask(
				&public,
				&cert,
				json!({ "audience": AUDIENCE, "application_type": "tamanu-central" }),
			)
			.await;
			response.assert_status_ok();
			let body: Value = response.json();
			let claims = verified_claims(body["token"].as_str().unwrap());
			let sub = format!(
				"group:{}:rank:production:application:tamanu-central:{}:",
				at.group, at.application
			);
			assert_eq!(claims["sub"], sub);
			assert_eq!(claims["application_id"], at.application.to_string());
			assert_eq!(claims["application_type"], "tamanu-central");
			assert_eq!(claims["machine_id"], at.machine.to_string());
			assert_eq!(
				claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap(),
				10 * 60,
				"the relying party's lifetime applies"
			);

			let entry = &issuances(&mut conn).await[0];
			assert!(matches!(
				entry.actor,
				Actor::Device { application: Some(app), .. } if app == at.application
			));
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn grants_on_machine_and_application_do_not_cover_each_other() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			let party = party(&mut conn, None).await;

			grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;
			let response = ask(
				&public,
				&cert,
				json!({ "audience": AUDIENCE, "application_type": "tamanu-central" }),
			)
			.await;
			response.assert_status(StatusCode::FORBIDDEN);
			assert_eq!(problem_type(&response.json()), "identity-token-not-granted");

			identity::set_grants(&mut conn, GrantSubject::Machine(at.machine), &[])
				.await
				.unwrap();
			grant(&mut conn, GrantSubject::Application(at.application), &party).await;
			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status(StatusCode::FORBIDDEN);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_request_mints_a_new_token() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("test")).await;
			let party = party(&mut conn, None).await;
			grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;

			let first: Value = ask(&public, &cert, json!({ "audience": AUDIENCE }))
				.await
				.json();
			let second: Value = ask(&public, &cert, json!({ "audience": AUDIENCE }))
				.await
				.json();
			let (first, second) = (
				verified_claims(first["token"].as_str().unwrap()),
				verified_claims(second["token"].as_str().unwrap()),
			);
			assert_ne!(first["jti"], second["jti"]);
			assert_eq!(issuances(&mut conn).await.len(), 2);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_refusal_as_not_granted_is_recorded_as_a_failed_issuance() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			let party = party(&mut conn, None).await;

			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status(StatusCode::FORBIDDEN);
			assert_eq!(problem_type(&response.json()), "identity-token-not-granted");

			let entries = issuances(&mut conn).await;
			assert_eq!(entries.len(), 1, "presented at once, not held as pending");
			let entry = &entries[0];
			assert_eq!(entry.status, Status::Failed);
			assert_eq!(entry.detail["reason"], "not_granted");
			assert!(entry.detail["jti"].is_null());
			assert!(entry.detail["kid"].is_null());
			let acted_on: Vec<String> = entry.records.iter().map(|r| r.id.clone()).collect();
			assert!(acted_on.contains(&at.machine.to_string()));
			assert!(acted_on.contains(&party.id.to_string()));
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn other_refusals_leave_nothing_in_the_log() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, None).await;
			let party = party(&mut conn, None).await;

			let unknown = ask(
				&public,
				&cert,
				json!({ "audience": "https://typo.example" }),
			)
			.await;
			unknown.assert_status(StatusCode::NOT_FOUND);
			assert_eq!(problem_type(&unknown.json()), "identity-audience-unknown");

			let no_such = ask(
				&public,
				&cert,
				json!({ "audience": AUDIENCE, "application_type": "senaite" }),
			)
			.await;
			no_such.assert_status(StatusCode::NOT_FOUND);
			assert_eq!(problem_type(&no_such.json()), "identity-subject-not-found");

			grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;
			let pending = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			pending.assert_status(StatusCode::CONFLICT);
			assert_eq!(problem_type(&pending.json()), "identity-subject-not-placed");

			assert!(issuances(&mut conn).await.is_empty());
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn not_granted_comes_before_not_placed() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			placed(&mut conn, device_id, None).await;
			party(&mut conn, None).await;
			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status(StatusCode::FORBIDDEN);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_ungrouped_machine_is_not_placed() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			sql_query("UPDATE machines SET group_id = NULL WHERE id = $1")
				.bind::<sql_types::Uuid, _>(at.machine)
				.execute(&mut conn)
				.await
				.unwrap();
			let party = party(&mut conn, None).await;
			grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;

			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status(StatusCode::CONFLICT);
			let body: Value = response.json();
			assert_eq!(problem_type(&body), "identity-subject-not-placed");
			assert!(
				body["detail"]
					.as_str()
					.unwrap_or_default()
					.contains("no group")
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_archived_application_is_no_such_subject() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			let party = party(&mut conn, None).await;
			grant(&mut conn, GrantSubject::Application(at.application), &party).await;
			sql_query("UPDATE applications SET deleted_at = now() WHERE id = $1")
				.bind::<sql_types::Uuid, _>(at.application)
				.execute(&mut conn)
				.await
				.unwrap();

			let response = ask(
				&public,
				&cert,
				json!({ "audience": AUDIENCE, "application_type": "tamanu-central" }),
			)
			.await;
			response.assert_status(StatusCode::NOT_FOUND);
			assert_eq!(problem_type(&response.json()), "identity-subject-not-found");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_without_a_machine_is_refused() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, _device_id, public, _private| {
			party(&mut conn, None).await;
			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status(StatusCode::PRECONDITION_FAILED);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_caller_asking_too_fast_is_held_back_first() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |_conn, cert, _device_id, public, _private| {
			// No machine, no relying party: every request is refused, and the
			// limit still holds them back before that refusal.
			for _ in 0..60 {
				ask(&public, &cert, json!({ "audience": AUDIENCE }))
					.await
					.assert_status(StatusCode::PRECONDITION_FAILED);
			}
			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			response.assert_status(StatusCode::TOO_MANY_REQUESTS);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn without_an_issuer_nothing_is_signed() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		let device_id: Uuid = {
			#[derive(diesel::QueryableByName)]
			struct Id {
				#[diesel(sql_type = sql_types::Uuid)]
				id: Uuid,
			}
			sql_query("INSERT INTO devices (role) VALUES ('server') RETURNING id")
				.get_result::<Id>(&mut conn)
				.await
				.unwrap()
				.id
		};
		let (key_data, cert) = commons_tests::server::make_certificate();
		sql_query(
			"INSERT INTO device_keys (device_id, key_data, name, is_active) VALUES ($1, $2, 'k', true)",
		)
		.bind::<sql_types::Uuid, _>(device_id)
		.bind::<sql_types::Binary, _>(key_data)
		.execute(&mut conn)
		.await
		.unwrap();

		let at = placed(&mut conn, device_id, Some("production")).await;
		let party = party(&mut conn, None).await;
		grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;

		let db = database::init_to(&url);
		let state = public_server::state::AppState {
			client_cert_header: commons_servers::device_auth::mtls::ClientCertHeader::Xfcc,
			identity: None,
			..public_server::state::AppState::from_db(db).unwrap()
		};
		let mut public = TestServer::new(commons_servers::router(
			public_server::audited(public_server::routes(), state.db.clone())
				.0
				.with_state(state),
			axum_client_ip::ClientIpSource::RightmostForwarded,
		));
		public.add_header("Forwarded", "for=192.0.1.60");
		public.add_header("X-Version", "3.4.5");

		let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
		response.assert_status(StatusCode::BAD_GATEWAY);
		assert_eq!(problem_type(&response.json()), "upstream");
		assert!(issuances(&mut conn).await.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn no_token_leaves_when_its_entry_cannot_be_written() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let at = placed(&mut conn, device_id, Some("production")).await;
			let party = party(&mut conn, None).await;
			grant(&mut conn, GrantSubject::Machine(at.machine), &party).await;
			sql_query(
				"ALTER TABLE audit_entries ADD CONSTRAINT no_issuance \
				 CHECK (action <> 'identity_token_issued')",
			)
			.execute(&mut conn)
			.await
			.unwrap();

			let response = ask(&public, &cert, json!({ "audience": AUDIENCE })).await;
			assert!(response.status_code().is_server_error());
			let body = response.text();
			assert!(!body.contains("\"token\""), "no token is returned: {body}");
		},
	)
	.await;
}
