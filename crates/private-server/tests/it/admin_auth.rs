//! Exercises the real administrative-identity path end to end.
//!
//! With `CANOPY_TRUST_TAILSCALE_HEADERS` set, the auth extractors authenticate
//! from the `Tailscale-User-*` request headers instead of the fixed debug
//! identity, so a single stack can act as different logins per request and
//! administrative status resolves through the real allow-list. This is what
//! lets a permission tier be graded rather than stubbed.

use database::admins::Admin;

// The extractors read this env var per request. Nextest runs each test in its
// own process, so setting it here can't leak into another test.
const TRUST_HEADERS: &str = "CANOPY_TRUST_TAILSCALE_HEADERS";

#[tokio::test(flavor = "multi_thread")]
async fn admin_identity_resolves_through_the_real_allowlist() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var(TRUST_HEADERS, "1") };

	commons_tests::server::run(async |mut conn, _public, private| {
		// No identity headers at all: the probe reports a definite non-admin,
		// and the gated endpoint refuses. 401, not 403: nobody authenticated,
		// so there is no identity to deny. The unlisted-login case below is the
		// 403, and keeping the two apart is the point of this test.
		let probe = private
			.post("/api/commons/is_current_user_admin")
			.json(&serde_json::json!({}))
			.await;
		probe.assert_status_ok();
		assert!(!probe.json::<bool>(), "no identity is not an admin");

		private
			.post("/api/admins/list")
			.json(&serde_json::json!({}))
			.await
			.assert_status_unauthorized();

		// A login that isn't on the allow-list authenticates, then is denied
		// admin by the real allow-list check — a 403 with a real identity
		// behind it, which is the grading decision this card makes testable.
		let not_admin = private
			.post("/api/commons/is_current_user_admin")
			.add_header("Tailscale-User-Login", "stranger@example.com")
			.add_header("Tailscale-User-Name", "Stranger")
			.json(&serde_json::json!({}))
			.await;
		not_admin.assert_status_ok();
		assert!(!not_admin.json::<bool>(), "unlisted login is not an admin");

		private
			.post("/api/admins/list")
			.add_header("Tailscale-User-Login", "stranger@example.com")
			.add_header("Tailscale-User-Name", "Stranger")
			.json(&serde_json::json!({}))
			.await
			.assert_status_forbidden();

		// Put that same login on the allow-list; now the real path grants it.
		Admin::add(&mut conn, "stranger@example.com")
			.await
			.expect("add admin");

		let now_admin = private
			.post("/api/commons/is_current_user_admin")
			.add_header("Tailscale-User-Login", "stranger@example.com")
			.add_header("Tailscale-User-Name", "Stranger")
			.json(&serde_json::json!({}))
			.await;
		now_admin.assert_status_ok();
		assert!(now_admin.json::<bool>(), "allow-listed login is an admin");

		private
			.post("/api/admins/list")
			.add_header("Tailscale-User-Login", "stranger@example.com")
			.add_header("Tailscale-User-Name", "Stranger")
			.json(&serde_json::json!({}))
			.await
			.assert_status_ok();

		// A different login on the same stack is denied, proving identity
		// varies per request rather than being fixed for the process.
		let other = private
			.post("/api/commons/is_current_user_admin")
			.add_header("Tailscale-User-Login", "someone-else@example.com")
			.add_header("Tailscale-User-Name", "Someone Else")
			.json(&serde_json::json!({}))
			.await;
		other.assert_status_ok();
		assert!(
			!other.json::<bool>(),
			"a different login is still not an admin"
		);
	})
	.await;

	// SAFETY: as above.
	unsafe { std::env::remove_var(TRUST_HEADERS) };
}
