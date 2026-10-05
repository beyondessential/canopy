//! End-to-end tests for the operator-first enrollment handshake on the
//! public server: begin → sign → complete, plus the opaque-error and
//! token-state behaviours.
//!
//! Enrolment admits a box, so every test here enrols a machine and none of
//! them creates an application: what runs on the box only comes into being
//! once the enrolled agent reports it.
//!
//! The channel-binding tests set `CANOPY_ENROLL_EKM_HEADER` via
//! `std::env::set_var`. That's process-global, but safe here because nextest
//! runs each test in its own process — the same reason the ticket test's
//! `PUBLIC_URL` set_var is safe. Do not run these under plain `cargo test`.

use base64::Engine;
use commons_tests::server::{make_signing_certificate, run, run_with_tailnet_device_auth};
use database::{
	diesel_async::AsyncPgConnection,
	machine_enrollment_tokens::MachineEnrollmentToken,
	machines::{Machine, NewMachine},
};
use jiff::SignedDuration;
use serde_json::{Value, json};
use uuid::Uuid;

fn b64() -> base64::engine::general_purpose::GeneralPurpose {
	base64::engine::general_purpose::STANDARD
}

/// The box to enrol. An operator creates it; enrolment is what binds it to
/// the identity that then speaks for it.
async fn machine(conn: &mut AsyncPgConnection) -> Uuid {
	Machine::create(conn, NewMachine::named("box"))
		.await
		.unwrap()
		.id
}

/// Drive begin → sign → complete for a machine with the given cert/key, and
/// return the `complete` response (caller asserts status).
///
/// The wire field stays `server_id`: it is the name fielded agents send, and
/// the value has always identified the box.
async fn run_handshake(
	public: &axum_test::TestServer,
	server_id: Uuid,
	token: &str,
	spki: &[u8],
	cert: &str,
	signing_key: &ring::signature::EcdsaKeyPair,
) -> axum_test::TestResponse {
	let begin = public
		.post("/servers/register/begin")
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
		.json(&json!({"server_id": server_id, "token": token}))
		.await;
	begin.assert_status_ok();
	let nonce_b64 = begin.json::<Value>()["nonce"].as_str().unwrap().to_string();
	let nonce = b64().decode(&nonce_b64).unwrap();

	let mut transcript = nonce.clone();
	transcript.extend_from_slice(server_id.as_bytes());
	transcript.extend_from_slice(spki);
	let rng = ring::rand::SystemRandom::new();
	let sig = signing_key.sign(&rng, &transcript).unwrap();

	public
		.post("/servers/register/complete")
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
		.json(&json!({
			"server_id": server_id,
			"nonce": nonce_b64,
			"signature": b64().encode(sig.as_ref()),
		}))
		.await
}

/// Drive begin → sign → complete over the private-server's tailnet `/public`
/// mount, where there is no client cert: the SPKI rides in the body. The
/// `Forwarded` header makes the caller look like the tagged tailnet device the
/// harness primed. `forged_cert`, if set, is sent as a client-certificate
/// header to prove it is *ignored* on this transport. Returns the `complete`
/// response.
async fn run_tailnet_handshake(
	private: &axum_test::TestServer,
	fwd_ip: std::net::IpAddr,
	server_id: Uuid,
	token: &str,
	spki: &[u8],
	signing_key: &ring::signature::EcdsaKeyPair,
	forged_cert: Option<&str>,
) -> axum_test::TestResponse {
	let spki_b64 = b64().encode(spki);
	let fwd = format!("for={fwd_ip}");

	let mut begin_req = private
		.post("/public/servers/register/begin")
		.add_header("Forwarded", &fwd);
	if let Some(cert) = forged_cert {
		begin_req = begin_req.add_header("x-forwarded-client-cert", &format!("Cert={}", cert));
	}
	let begin = begin_req
		.json(&json!({"server_id": server_id, "token": token, "spki": spki_b64}))
		.await;
	begin.assert_status_ok();
	let nonce_b64 = begin.json::<Value>()["nonce"].as_str().unwrap().to_string();
	let nonce = b64().decode(&nonce_b64).unwrap();

	let mut transcript = nonce.clone();
	transcript.extend_from_slice(server_id.as_bytes());
	transcript.extend_from_slice(spki);
	let rng = ring::rand::SystemRandom::new();
	let sig = signing_key.sign(&rng, &transcript).unwrap();

	let mut complete_req = private
		.post("/public/servers/register/complete")
		.add_header("Forwarded", &fwd);
	if let Some(cert) = forged_cert {
		complete_req =
			complete_req.add_header("x-forwarded-client-cert", &format!("Cert={}", cert));
	}
	complete_req
		.json(&json!({
			"server_id": server_id,
			"nonce": nonce_b64,
			"signature": b64().encode(sig.as_ref()),
			"spki": spki_b64,
		}))
		.await
}

#[tokio::test(flavor = "multi_thread")]
async fn tailnet_enrollment_happy_path() {
	run_with_tailnet_device_auth(
		"server",
		async |mut conn, fwd_ip, _node, _dev, _public, private| {
			use database::Device;
			let (spki, _cert, key) = make_signing_certificate();
			let machine_id = machine(&mut conn).await;
			let (_t, token) =
				MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
					.await
					.unwrap();

			run_tailnet_handshake(&private, fwd_ip, machine_id, &token, &spki, &key, None)
				.await
				.assert_status_ok();

			// Registered + bound to a device carrying the body-supplied key; token spent.
			let after = Machine::get_by_id(&mut conn, machine_id).await.unwrap();
			assert!(after.registered_at.is_some(), "registered_at set");
			assert!(after.device_id.is_some(), "device bound");
			let bound = Device::from_key(&mut conn, &spki).await.unwrap().unwrap();
			assert_eq!(
				bound.id,
				after.device_id.unwrap(),
				"body SPKI bound the device"
			);
			assert!(
				MachineEnrollmentToken::find_active(&mut conn, machine_id, &token)
					.await
					.is_err(),
				"token consumed",
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn internet_path_rejects_body_spki() {
	run(async |mut conn, public, _private| {
		// On the internet mTLS mount there is no tailnet directory, so a
		// body-supplied SPKI must NOT be accepted — the cert can't be skipped.
		let (spki, _cert, _key) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		let resp = public
			.post("/servers/register/begin")
			.json(&json!({
				"server_id": machine_id,
				"token": token,
				"spki": b64().encode(&spki),
			}))
			.await;
		resp.assert_status_forbidden();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tailnet_ignores_the_mtls_certificate_header() {
	run_with_tailnet_device_auth(
		"server",
		async |mut conn, fwd_ip, _node, _dev, _public, private| {
			use database::Device;
			// Real device key (carried in the body) and a *different*, attacker-style
			// cert sent in the (forgeable) client-certificate header.
			let (spki_real, _c, key_real) = make_signing_certificate();
			let (spki_forged, cert_forged, _k) = make_signing_certificate();
			let machine_id = machine(&mut conn).await;
			let (_t, token) =
				MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
					.await
					.unwrap();

			// Body SPKI = real key; header cert = forged. The tailnet mount must use
			// the body and ignore the header, so the handshake (signed by the real
			// key) succeeds and binds the real key — not the forged one.
			run_tailnet_handshake(
				&private,
				fwd_ip,
				machine_id,
				&token,
				&spki_real,
				&key_real,
				Some(&cert_forged),
			)
			.await
			.assert_status_ok();

			assert!(
				Device::from_key(&mut conn, &spki_real)
					.await
					.unwrap()
					.is_some(),
				"the body-supplied (real) key was bound",
			);
			assert!(
				Device::from_key(&mut conn, &spki_forged)
					.await
					.unwrap()
					.is_none(),
				"the forged cert-header key was ignored, never bound",
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enrollment_adds_key_to_tailscale_precreated_device() {
	run(async |mut conn, public, _private| {
		use database::Device;
		let (spki, cert, key) = make_signing_certificate();

		// A device pre-created from a Tailscale identity (no mTLS key yet),
		// bound to the server at creation time.
		let device = Device::create_with_tailscale(
			&mut conn,
			database::devices::TailscaleIdentity {
				node_id: "nodekey:precreated".into(),
				node_name: None,
				tailnet: None,
			},
			commons_types::device::DeviceRole::Machine,
		)
		.await
		.unwrap();
		let machine_id = machine(&mut conn).await;
		Machine::bind_device(&mut conn, machine_id, device.id)
			.await
			.unwrap();
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		run_handshake(&public, machine_id, &token, &spki, &cert, &key)
			.await
			.assert_status_ok();

		// The key landed on the *existing* device; no second device was made.
		let after = Machine::get_by_id(&mut conn, machine_id).await.unwrap();
		assert_eq!(
			after.device_id,
			Some(device.id),
			"still the precreated device"
		);
		let bound = Device::from_key(&mut conn, &spki).await.unwrap().unwrap();
		assert_eq!(bound.id, device.id, "key attached to the precreated device");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enrollment_rejects_key_bound_to_another_live_machine() {
	run(async |mut conn, public, _private| {
		let (spki, cert, key) = make_signing_certificate();

		// Machine A enrols with this cert.
		let machine_a = machine(&mut conn).await;
		let (_ta, token_a) =
			MachineEnrollmentToken::mint(&mut conn, machine_a, SignedDuration::from_hours(1))
				.await
				.unwrap();
		run_handshake(&public, machine_a, &token_a, &spki, &cert, &key)
			.await
			.assert_status_ok();

		// Machine B tries to enrol with the SAME key while A is live → refused.
		let machine_b = machine(&mut conn).await;
		let (_tb, token_b) =
			MachineEnrollmentToken::mint(&mut conn, machine_b, SignedDuration::from_hours(1))
				.await
				.unwrap();
		run_handshake(&public, machine_b, &token_b, &spki, &cert, &key)
			.await
			.assert_status_forbidden();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enrollment_happy_path() {
	run(async |mut conn, public, _private| {
		let (spki, cert, signing_key) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		// begin
		let resp = public
			.post("/servers/register/begin")
			.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
			.json(&json!({"server_id": machine_id, "token": token}))
			.await;
		resp.assert_status_ok();
		let begin: Value = resp.json();
		let nonce_b64 = begin["nonce"].as_str().unwrap().to_string();
		assert_eq!(begin["channel_binding_required"], json!(false));
		let nonce = b64().decode(&nonce_b64).unwrap();

		// sign transcript: nonce ‖ machine id ‖ spki
		let mut transcript = nonce.clone();
		transcript.extend_from_slice(machine_id.as_bytes());
		transcript.extend_from_slice(&spki);
		let rng = ring::rand::SystemRandom::new();
		let sig = signing_key.sign(&rng, &transcript).unwrap();
		let sig_b64 = b64().encode(sig.as_ref());

		// complete
		let resp = public
			.post("/servers/register/complete")
			.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
			.json(&json!({"server_id": machine_id, "nonce": nonce_b64, "signature": sig_b64}))
			.await;
		resp.assert_status_ok();

		// the machine is now registered + bound; token is spent.
		let after = Machine::get_by_id(&mut conn, machine_id).await.unwrap();
		assert!(after.registered_at.is_some(), "registered_at set");
		assert!(after.device_id.is_some(), "device bound");
		assert!(
			MachineEnrollmentToken::find_active(&mut conn, machine_id, &token)
				.await
				.is_err(),
			"token consumed"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enrollment_bad_signature_is_opaque_and_keeps_token() {
	run(async |mut conn, public, _private| {
		let (_spki, cert, _key) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		let resp = public
			.post("/servers/register/begin")
			.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
			.json(&json!({"server_id": machine_id, "token": token}))
			.await;
		resp.assert_status_ok();
		let nonce_b64 = resp.json::<Value>()["nonce"].as_str().unwrap().to_string();

		// garbage signature
		let resp = public
			.post("/servers/register/complete")
			.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
			.json(&json!({
				"server_id": machine_id,
				"nonce": nonce_b64,
				"signature": b64().encode([0u8; 72]),
			}))
			.await;
		resp.assert_status_forbidden();

		// token must NOT have been burned by a failed complete.
		assert!(
			MachineEnrollmentToken::find_active(&mut conn, machine_id, &token)
				.await
				.is_ok(),
			"token still active after bad signature"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enrollment_rate_limited_per_machine() {
	run(async |_conn, public, _private| {
		let machine_id = Uuid::new_v4();
		// The per-machine budget is 20/min; the rate check runs before any token
		// or machine validation, so even bogus begins count. The 21st trips 429.
		let mut saw_429 = false;
		for _ in 0..25 {
			let resp = public
				.post("/servers/register/begin")
				.json(&json!({"server_id": machine_id, "token": "x"}))
				.await;
			if resp.status_code() == 429 {
				saw_429 = true;
				break;
			}
		}
		assert!(saw_429, "per-machine rate limit should trip with 429");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn enrollment_unknown_machine_and_bad_token_are_opaque() {
	run(async |mut conn, public, _private| {
		let (_spki, cert, _key) = make_signing_certificate();

		// unknown machine id
		let resp = public
			.post("/servers/register/begin")
			.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
			.json(&json!({"server_id": Uuid::new_v4(), "token": "whatever"}))
			.await;
		resp.assert_status_forbidden();

		// known machine, wrong token
		let machine_id = machine(&mut conn).await;
		let resp = public
			.post("/servers/register/begin")
			.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
			.json(&json!({"server_id": machine_id, "token": "not-the-token"}))
			.await;
		resp.assert_status_forbidden();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn re_enrollment_replaces_the_device() {
	run(async |mut conn, public, _private| {
		use database::{Device, DeviceKey};

		// First enrollment with cert A.
		let (spki_a, cert_a, key_a) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_ta, token_a) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();
		run_handshake(&public, machine_id, &token_a, &spki_a, &cert_a, &key_a)
			.await
			.assert_status_ok();
		let device_a = Machine::get_by_id(&mut conn, machine_id)
			.await
			.unwrap()
			.device_id
			.unwrap();

		// Re-enroll with a DIFFERENT box (cert B) — replaces the device.
		let (spki_b, cert_b, key_b) = make_signing_certificate();
		let (_tb, token_b) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();
		run_handshake(&public, machine_id, &token_b, &spki_b, &cert_b, &key_b)
			.await
			.assert_status_ok();

		let device_b = Machine::get_by_id(&mut conn, machine_id)
			.await
			.unwrap()
			.device_id
			.unwrap();
		assert_ne!(device_b, device_a, "re-enroll bound a new device");
		assert_eq!(
			Device::from_key(&mut conn, &spki_b)
				.await
				.unwrap()
				.unwrap()
				.id,
			device_b,
			"new box's key authenticates as the new device",
		);
		assert!(
			DeviceKey::find_by_device(&mut conn, device_a)
				.await
				.unwrap()
				.is_empty(),
			"old device's keys were deactivated on replacement",
		);
	})
	.await;
}

/// Header the test plays the proxy with — must match the value we set
/// `CANOPY_ENROLL_EKM_HEADER` to in each channel-binding test.
const EKM_HEADER: &str = "x-tls-exporter";

/// Internet-path begin → sign → complete with channel-binding knobs. The
/// transcript is signed with `signed_ekm` appended (if `Some`); `header_ekm` is
/// sent in the EKM header (if `Some`), standing in for the terminating proxy.
/// Returns the parsed `begin` body and the `complete` response.
async fn run_cb_handshake(
	public: &axum_test::TestServer,
	server_id: Uuid,
	token: &str,
	spki: &[u8],
	cert: &str,
	signing_key: &ring::signature::EcdsaKeyPair,
	signed_ekm: Option<&[u8]>,
	header_ekm: Option<&[u8]>,
) -> (Value, axum_test::TestResponse) {
	let begin = public
		.post("/servers/register/begin")
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
		.json(&json!({"server_id": server_id, "token": token}))
		.await;
	begin.assert_status_ok();
	let begin_body: Value = begin.json();
	let nonce_b64 = begin_body["nonce"].as_str().unwrap().to_string();
	let nonce = b64().decode(&nonce_b64).unwrap();

	let mut transcript = nonce.clone();
	transcript.extend_from_slice(server_id.as_bytes());
	transcript.extend_from_slice(spki);
	if let Some(ekm) = signed_ekm {
		transcript.extend_from_slice(ekm);
	}
	let rng = ring::rand::SystemRandom::new();
	let sig = signing_key.sign(&rng, &transcript).unwrap();

	let mut complete_req = public
		.post("/servers/register/complete")
		.add_header("x-forwarded-client-cert", &format!("Cert={}", cert));
	if let Some(ekm) = header_ekm {
		complete_req = complete_req.add_header(EKM_HEADER, b64().encode(ekm));
	}
	let complete = complete_req
		.json(&json!({
			"server_id": server_id,
			"nonce": nonce_b64,
			"signature": b64().encode(sig.as_ref()),
		}))
		.await;
	(begin_body, complete)
}

// NOTE: these verify Canopy's channel-binding *logic* — that it advertises the
// requirement, folds the proxy-supplied EKM into the expected transcript, and
// rejects on absence/mismatch. They do not (and an integration test can't)
// prove the EKM corresponds to a real TLS session: there's no mTLS terminator
// here, so the test supplies the EKM on both sides. Real relay-resistance
// depends on the proxy emitting a genuine RFC 9266 exporter.

#[tokio::test(flavor = "multi_thread")]
async fn channel_binding_happy_path() {
	unsafe { std::env::set_var("CANOPY_ENROLL_EKM_HEADER", EKM_HEADER) };
	run(async |mut conn, public, _private| {
		let (spki, cert, key) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		// Same EKM in the signed transcript and the proxy header → bound.
		let ekm = [7u8; 32];
		let (begin, complete) = run_cb_handshake(
			&public,
			machine_id,
			&token,
			&spki,
			&cert,
			&key,
			Some(&ekm),
			Some(&ekm),
		)
		.await;
		assert_eq!(
			begin["channel_binding_required"],
			json!(true),
			"begin advertises the requirement when the EKM header env is set",
		);
		complete.assert_status_ok();
		assert!(
			Machine::get_by_id(&mut conn, machine_id)
				.await
				.unwrap()
				.registered_at
				.is_some(),
			"registered with channel binding",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_binding_missing_header_is_rejected() {
	unsafe { std::env::set_var("CANOPY_ENROLL_EKM_HEADER", EKM_HEADER) };
	run(async |mut conn, public, _private| {
		let (spki, cert, key) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		// The device folded the EKM into its signature, but the proxy header is
		// absent → Canopy has nothing to bind against → rejected, token intact.
		let ekm = [7u8; 32];
		let (_begin, complete) = run_cb_handshake(
			&public,
			machine_id,
			&token,
			&spki,
			&cert,
			&key,
			Some(&ekm),
			None,
		)
		.await;
		complete.assert_status_forbidden();
		assert!(
			MachineEnrollmentToken::find_active(&mut conn, machine_id, &token)
				.await
				.is_ok(),
			"token not burned by a failed complete",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn channel_binding_mismatch_is_rejected() {
	unsafe { std::env::set_var("CANOPY_ENROLL_EKM_HEADER", EKM_HEADER) };
	run(async |mut conn, public, _private| {
		let (spki, cert, key) = make_signing_certificate();
		let machine_id = machine(&mut conn).await;
		let (_t, token) =
			MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
				.await
				.unwrap();

		// Header EKM differs from the one signed → expected transcript differs
		// from the signed one → PoP fails. This is the case that exercises the
		// binding itself.
		let (_begin, complete) = run_cb_handshake(
			&public,
			machine_id,
			&token,
			&spki,
			&cert,
			&key,
			Some(&[1u8; 32]),
			Some(&[2u8; 32]),
		)
		.await;
		complete.assert_status_forbidden();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn tailnet_path_skips_channel_binding() {
	unsafe { std::env::set_var("CANOPY_ENROLL_EKM_HEADER", EKM_HEADER) };
	run_with_tailnet_device_auth(
		"server",
		async |mut conn, fwd_ip, _node, _dev, _public, private| {
			let (spki, _cert, key) = make_signing_certificate();
			let machine_id = machine(&mut conn).await;
			let (_t, token) =
				MachineEnrollmentToken::mint(&mut conn, machine_id, SignedDuration::from_hours(1))
					.await
					.unwrap();

			// Even with channel binding enabled globally, the tailnet mount has no
			// TLS exporter, so it neither advertises nor requires it: begin reports
			// false and complete succeeds with no EKM header and none in the
			// signature.
			let fwd = format!("for={fwd_ip}");
			let begin = private
				.post("/public/servers/register/begin")
				.add_header("Forwarded", &fwd)
				.json(
					&json!({"server_id": machine_id, "token": token, "spki": b64().encode(&spki)}),
				)
				.await;
			begin.assert_status_ok();
			assert_eq!(
				begin.json::<Value>()["channel_binding_required"],
				json!(false),
				"tailnet mount never requires channel binding",
			);

			run_tailnet_handshake(&private, fwd_ip, machine_id, &token, &spki, &key, None)
				.await
				.assert_status_ok();
		},
	)
	.await;
}
