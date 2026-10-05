//! Round-trips a minted enrollment ticket: the private API returns an
//! age/scrypt-encrypted ticket plus the 4-word passphrase; decrypting the
//! ticket with the passphrase must yield the real, active enrollment token.

use algae_cli::{
	passphrases::{Passphrase, SecretString},
	streams::decrypt_stream,
};
use base64::Engine;
use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use database::machine_enrollment_tokens::MachineEnrollmentToken;
use serde_json::{Value, json};
use uuid::Uuid;

/// A machine to enrol. It carries no application, and is not supposed to:
/// enrolment is what admits the box, and what runs on the box only exists once
/// the enrolled agent reports it.
async fn seed_machine(conn: &mut AsyncPgConnection) -> String {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (name, id) VALUES ('box', '{id}')"
	))
	.await
	.expect("seed machine");
	id.to_string()
}

#[tokio::test(flavor = "multi_thread")]
async fn mint_enrollment_ticket_round_trips_to_active_token() {
	commons_tests::server::run(async |mut conn, _public, private| {
		// PUBLIC_URL is read by the handler to build the payload.
		unsafe { std::env::set_var("PUBLIC_URL", "https://api.example.test") };

		let machine_id = seed_machine(&mut conn).await;

		// Mint an encrypted enrollment ticket.
		let response = private
			.post("/api/fleet/machines/mint_enrollment")
			.json(&json!({ "machine_id": machine_id }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		let ticket = body["ticket"].as_str().expect("ticket present");
		let passphrase = body["passphrase"].as_str().expect("passphrase present");

		// The passphrase is 4 lowercase hyphen-separated words.
		let words: Vec<&str> = passphrase.split('-').collect();
		assert_eq!(words.len(), 4, "passphrase is four words: {passphrase}");
		assert!(
			words
				.iter()
				.all(|w| !w.is_empty() && w == &w.to_lowercase()),
			"passphrase words are non-empty and lowercase: {passphrase}"
		);

		// Decrypt the ticket with the passphrase.
		let encrypted = base64::engine::general_purpose::STANDARD
			.decode(ticket)
			.expect("ticket is valid base64");
		let key = Passphrase::new(SecretString::from(passphrase.to_string()));
		let mut decrypted = Vec::new();
		decrypt_stream(
			futures::io::Cursor::new(&encrypted[..]),
			&mut decrypted,
			Box::new(key),
		)
		.await
		.expect("ticket decrypts with the returned passphrase");

		// (a) The decrypted payload carries a token.
		let payload: Value = serde_json::from_slice(&decrypted).expect("payload is JSON");
		assert_eq!(payload["v"], "enroll-1");
		// The ticket key stays `server_id`: it is what a fielded bestool reads,
		// and what it sends back when it registers.
		assert_eq!(payload["server_id"], machine_id);
		let token = payload["token"].as_str().expect("token in payload");

		// (b) That token is the real, active enrollment token for the machine.
		MachineEnrollmentToken::find_active(&mut conn, machine_id.parse().unwrap(), token)
			.await
			.expect("decrypted token is active for the machine");
	})
	.await
}

/// The plaintext enrollment token is delivered *only* inside the encrypted
/// ticket — it must never appear in the clear in any response or error body.
#[tokio::test(flavor = "multi_thread")]
async fn enrollment_token_never_leaks_in_the_clear() {
	commons_tests::server::run(async |mut conn, public, private| {
		unsafe { std::env::set_var("PUBLIC_URL", "https://api.example.test") };

		let machine_id = seed_machine(&mut conn).await;

		let mint = private
			.post("/api/fleet/machines/mint_enrollment")
			.json(&json!({ "machine_id": machine_id }))
			.await;
		mint.assert_status_ok();
		let mint_body = mint.text();
		let body: Value = serde_json::from_str(&mint_body).expect("mint body is JSON");
		let ticket = body["ticket"].as_str().expect("ticket present");
		let passphrase = body["passphrase"].as_str().expect("passphrase present");

		// Recover the plaintext token by decrypting the ticket — the only place
		// it should be obtainable.
		let encrypted = base64::engine::general_purpose::STANDARD
			.decode(ticket)
			.expect("ticket is valid base64");
		let key = Passphrase::new(SecretString::from(passphrase.to_string()));
		let mut decrypted = Vec::new();
		decrypt_stream(
			futures::io::Cursor::new(&encrypted[..]),
			&mut decrypted,
			Box::new(key),
		)
		.await
		.expect("ticket decrypts");
		let payload: Value = serde_json::from_slice(&decrypted).expect("payload is JSON");
		let token = payload["token"].as_str().expect("token in payload");
		assert!(!token.is_empty(), "token is non-empty");

		// (a) The mint response carries the token only as ciphertext, never in
		// the clear.
		assert!(
			!mint_body.contains(token),
			"mint response must not contain the plaintext token"
		);

		// (b) enrollment_status returns only timestamps — never the token.
		let status = private
			.post("/api/fleet/machines/enrollment_status")
			.json(&json!({ "machine_id": machine_id }))
			.await;
		status.assert_status_ok();
		assert!(
			!status.text().contains(token),
			"enrollment_status must not contain the token"
		);

		// (c) A failed register (no client cert) returns an opaque error that
		// does not echo the token back, even though we sent it.
		let begin = public
			.post("/servers/register/begin")
			.json(&json!({ "server_id": machine_id, "token": token }))
			.await;
		begin.assert_status_forbidden();
		assert!(
			!begin.text().contains(token),
			"a register error must not echo the token"
		);
	})
	.await
}
