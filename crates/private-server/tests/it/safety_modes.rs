//! The safety-mode boundary, exercised through the real tailnet header path.
//!
//! Spec: `.workhorse/specs/private-server/safety-modes.md` (id `SAFE`).
//!
//! The debug identity is an administrator in danger mode by construction, which
//! is what lets the rest of the suite reach graded handlers without driving a
//! session. Every test here opts out of it with `CANOPY_TRUST_TAILSCALE_HEADERS`
//! so the boundary is decided rather than skipped.

use database::admins::Admin;
use serde_json::json;

const TRUST_HEADERS: &str = "CANOPY_TRUST_TAILSCALE_HEADERS";
const SESSION_HEADER: &str = "x-canopy-session";

const OPERATOR: &str = "operator@example.com";
const OTHER: &str = "other@example.com";

/// Opt out of the debug identity for this test process. Nextest runs each test
/// in its own process, so this cannot leak into another test.
fn trust_headers() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var(TRUST_HEADERS, "1") };
}

/// The problem-type slug of a refusal, so the two refusals can be told apart.
fn problem_type(response: &serde_json::Value) -> String {
	response
		.get("type")
		.and_then(|t| t.as_str())
		.unwrap_or_default()
		.to_owned()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_read_only_session_is_refused_a_higher_graded_request() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		Admin::add(&mut conn, OPERATOR).await.expect("add admin");
		Admin::set_danger(&mut conn, OPERATOR, true)
			.await
			.expect("grant danger");

		// A fresh session begins read-only.
		let session = private
			.post("/api/safety/session")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.json(&json!({}))
			.await;
		session.assert_status_ok();
		let session: serde_json::Value = session.json();
		let id = session["id"].as_str().expect("session id").to_owned();
		assert_eq!(session["mode"], "read-only", "a session begins read-only");

		// A read-only-graded request is answered from it.
		private
			.post("/api/admins/list")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({}))
			.await
			.assert_status_ok();

		// A write-graded one is refused, and says so as a mode refusal: the
		// operator can make this request once they raise.
		let refused = private
			.post("/api/inventory_variables/set")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(
				&json!({"group_id": "00000000-0000-0000-0000-000000000000", "name": "x", "value": "y"}),
			)
			.await;
		refused.assert_status_forbidden();
		assert!(
			problem_type(&refused.json()).ends_with("safety-mode-too-low"),
			"a mode refusal, not a permission one"
		);

		// And so is a danger-graded one, even though the operator holds danger:
		// the permission is not the mode.
		private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await
			.assert_status_forbidden();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_ladder_holds_downwards_and_lowering_ends_a_raise() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		Admin::add(&mut conn, OPERATOR).await.expect("add admin");
		Admin::set_danger(&mut conn, OPERATOR, true)
			.await
			.expect("grant danger");

		let id = new_session(&private, OPERATOR).await;

		// Raising to danger reports the mode and a time to count down to.
		let raised = private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "danger"}))
			.await;
		raised.assert_status_ok();
		let raised: serde_json::Value = raised.json();
		assert_eq!(raised["mode"], "danger");
		assert!(
			raised["raise_expires_at"].is_string(),
			"a raise carries the time it lapses"
		);

		// Danger permits write-graded work without dropping back down. The
		// group does not exist, so this fails on its way to the database — what
		// matters is that it is not turned away at the boundary.
		let write_graded = private
			.post("/api/inventory_variables/set")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(
				&json!({"group_id": "00000000-0000-0000-0000-000000000000", "name": "x", "value": "y"}),
			)
			.await;
		assert_ne!(
			write_graded.status_code(),
			403,
			"danger reaches everything write reaches"
		);

		// A danger-graded request is now answered.
		private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await
			.assert_status_ok();

		// Lowering ends the raise at once, without waiting for the countdown.
		let lowered = private
			.post("/api/safety/lower")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({}))
			.await;
		lowered.assert_status_ok();
		let lowered: serde_json::Value = lowered.json();
		assert_eq!(lowered["mode"], "read-only");
		assert!(lowered["raise_expires_at"].is_null());

		private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await
			.assert_status_forbidden();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_danger_permission_is_separate_from_the_mode() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		// An administrator who does not hold danger.
		Admin::add(&mut conn, OPERATOR).await.expect("add admin");

		let id = new_session(&private, OPERATOR).await;

		// They cannot raise to danger, and are told they lack the permission
		// rather than that something went wrong.
		let refused = private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "danger"}))
			.await;
		refused.assert_status_forbidden();
		assert!(
			problem_type(&refused.json()).ends_with("danger-not-permitted"),
			"a permission refusal, not a mode one"
		);

		// Raising to write needs no permission, and takes effect.
		let raised = private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "write"}))
			.await;
		raised.assert_status_ok();
		assert_eq!(raised.json::<serde_json::Value>()["mode"], "write");

		// Write does not reach a danger-graded handler, and the refusal names
		// the permission they lack rather than telling them to raise: raising
		// would not help.
		let refused = private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await;
		refused.assert_status_forbidden();
		assert!(
			problem_type(&refused.json()).ends_with("danger-not-permitted"),
			"an operator without danger can never make this request"
		);

		// Granting danger takes effect at once, on the session they already
		// hold — but the mode still has to be raised to it.
		Admin::set_danger(&mut conn, OPERATOR, true)
			.await
			.expect("grant danger");
		let refused = private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await;
		refused.assert_status_forbidden();
		assert!(
			problem_type(&refused.json()).ends_with("safety-mode-too-low"),
			"now it is the mode that is short, not the permission"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn withdrawing_danger_takes_effect_during_an_existing_raise() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		Admin::add(&mut conn, OPERATOR).await.expect("add admin");
		Admin::set_danger(&mut conn, OPERATOR, true)
			.await
			.expect("grant danger");

		let id = new_session(&private, OPERATOR).await;
		private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "danger"}))
			.await
			.assert_status_ok();

		// Withdrawn mid-raise: the permission is resolved afresh per request, so
		// it does not wait for the raise to lapse.
		Admin::set_danger(&mut conn, OPERATOR, false)
			.await
			.expect("withdraw danger");

		let refused = private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await;
		refused.assert_status_forbidden();
		assert!(
			problem_type(&refused.json()).ends_with("danger-not-permitted"),
			"the withdrawal bites inside the raise"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_session_is_usable_only_by_the_login_it_belongs_to() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		for login in [OPERATOR, OTHER] {
			Admin::add(&mut conn, login).await.expect("add admin");
			Admin::set_danger(&mut conn, login, true)
				.await
				.expect("grant danger");
		}

		let id = new_session(&private, OPERATOR).await;
		private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "danger"}))
			.await
			.assert_status_ok();

		// The other login presenting that identifier reaches no further than a
		// caller with no session at all.
		let refused = private
			.post("/api/mcp_tokens/mint")
			.add_header("Tailscale-User-Login", OTHER)
			.add_header("Tailscale-User-Name", "Other")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"name": "agent"}))
			.await;
		refused.assert_status_forbidden();
		assert!(
			problem_type(&refused.json()).ends_with("safety-mode-too-low"),
			"another login's raise is not theirs to use"
		);

		// Asking for their own session hands them a different one, still
		// read-only, rather than adopting the one they presented.
		let theirs = private
			.post("/api/safety/session")
			.add_header("Tailscale-User-Login", OTHER)
			.add_header("Tailscale-User-Name", "Other")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({}))
			.await;
		theirs.assert_status_ok();
		let theirs: serde_json::Value = theirs.json();
		assert_ne!(theirs["id"].as_str().unwrap(), id);
		assert_eq!(theirs["mode"], "read-only");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unknown_session_reads_as_read_only_rather_than_being_refused() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		Admin::add(&mut conn, OPERATOR).await.expect("add admin");

		let unknown = uuid::Uuid::new_v4().to_string();

		// A read-only-graded request is answered: an unknown identifier is not
		// itself a refusal, and a client can read before it has a session.
		private
			.post("/api/admins/list")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &unknown)
			.json(&json!({}))
			.await
			.assert_status_ok();

		// With no session header at all, likewise.
		private
			.post("/api/admins/list")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.json(&json!({}))
			.await
			.assert_status_ok();

		// A higher-graded request from it reads as read-only, so it is a mode
		// refusal rather than a rejection of the identifier.
		let refused = private
			.post("/api/inventory_variables/set")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &unknown)
			.json(
				&json!({"group_id": "00000000-0000-0000-0000-000000000000", "name": "x", "value": "y"}),
			)
			.await;
		refused.assert_status_forbidden();
		assert!(problem_type(&refused.json()).ends_with("safety-mode-too-low"));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_raise_survives_a_restart_because_it_is_a_row() {
	trust_headers();

	commons_tests::server::run(async |mut conn, _public, private| {
		Admin::add(&mut conn, OPERATOR).await.expect("add admin");
		Admin::set_danger(&mut conn, OPERATOR, true)
			.await
			.expect("grant danger");

		let id = new_session(&private, OPERATOR).await;
		private
			.post("/api/safety/raise")
			.add_header("Tailscale-User-Login", OPERATOR)
			.add_header("Tailscale-User-Name", "Operator")
			.add_header(SESSION_HEADER, &id)
			.json(&json!({"mode": "danger"}))
			.await
			.assert_status_ok();

		// Read straight out of the database, which is where a restart — or
		// another private-server process — would find it.
		let stored = database::operator_sessions::OperatorSession::get(
			&mut conn,
			id.parse().expect("session id is a uuid"),
		)
		.await
		.expect("read session")
		.expect("session exists");
		assert_eq!(
			stored.effective_mode(jiff::Timestamp::now()),
			commons_types::safety::SafetyMode::Danger,
		);
		assert_eq!(stored.login, OPERATOR);
	})
	.await;
}

/// Mint a fresh session for a login and return its identifier.
async fn new_session(private: &commons_tests::axum_test::TestServer, login: &str) -> String {
	let session = private
		.post("/api/safety/session")
		.add_header("Tailscale-User-Login", login)
		.add_header("Tailscale-User-Name", "Operator")
		.json(&json!({}))
		.await;
	session.assert_status_ok();
	session.json::<serde_json::Value>()["id"]
		.as_str()
		.expect("session id")
		.to_owned()
}
