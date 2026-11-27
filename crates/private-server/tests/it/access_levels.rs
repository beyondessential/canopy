//! A caller's access level, resolved through the real identity path end to end.
//!
//! Spec: `.workhorse/specs/private-server/access-levels.md` (id `ADM`).
//!
//! With `CANOPY_TRUST_TAILSCALE_HEADERS` set, the auth extractors authenticate
//! from the `Tailscale-User-*` request headers instead of the fixed debug
//! identity, so a single stack can act as different logins per request and a
//! level resolves through the real access lists and tailnet policy.

use std::future::Future;

use axum_client_ip::ClientIpSource;
use commons_servers::{router, tailnet_directory::TailnetDirectory};
use commons_tests::{axum_test::TestServer, db::TestDb};
use database::{
	access_lists::{AccessListEntry, ListLevel},
	diesel_async::AsyncPgConnection,
};
use serde_json::{Value, json};

// The extractors read this env var per request. Nextest runs each test in its
// own process, so setting it here can't leak into another test.
const TRUST_HEADERS: &str = "CANOPY_TRUST_TAILSCALE_HEADERS";

/// Opt out of the debug identity for this test process.
pub fn trust_headers() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var(TRUST_HEADERS, "1") };
}

/// Run a test against a private server reading the given tailnet policy, as if
/// it had been read successfully, or against one with no tailnet API
/// configured where there is none. The directory is handed to the test so it
/// can make a later read fail.
pub async fn run_with_policy<F, Fut>(policy: Option<Value>, test: F)
where
	F: FnOnce(AsyncPgConnection, TestServer, Option<TailnetDirectory>) -> Fut,
	Fut: Future<Output = ()>,
{
	TestDb::run(async |conn, url| {
		let directory = policy.map(|policy| TailnetDirectory::for_test_with_policy([], policy));
		let private = router(
			private_server::routes(private_server::state::AppState {
				tailnet_directory: directory.clone(),
				..private_server::state::AppState::from_db_url(&url)
					.await
					.unwrap()
			})
			.unwrap(),
			ClientIpSource::RightmostForwarded,
		);
		let mut private = TestServer::new(private);
		// An operator's laptop, not a tagged tailnet device.
		private.add_header("Forwarded", "for=192.0.2.60");
		test(conn, private, directory).await
	})
	.await
}

/// The level the probe reports for a login, or for no identity at all.
pub async fn level_of(private: &TestServer, login: Option<&str>) -> Value {
	let mut request = private.post("/api/commons/current_user_level");
	if let Some(login) = login {
		request = request
			.add_header("Tailscale-User-Login", login)
			.add_header("Tailscale-User-Name", "Someone");
	}
	let response = request.json(&json!({})).await;
	response.assert_status_ok();
	response.json()
}

async fn add(conn: &mut AsyncPgConnection, level: ListLevel, login: &str) {
	AccessListEntry::add(conn, level, login)
		.await
		.expect("add to list");
}

// spec: ADM#how-a-level-is-held
// spec: ADM#reporting-a-level-to-a-caller
#[tokio::test(flavor = "multi_thread")]
async fn a_listed_login_holds_its_lists_level_and_anyone_else_holds_all() {
	trust_headers();

	// No tailnet API configured: the lists alone confer levels.
	run_with_policy(None, async |mut conn, private, _| {
		add(&mut conn, ListLevel::Pm, "Pat@Example.com").await;
		add(&mut conn, ListLevel::Admin, "boss@example.com").await;

		assert_eq!(level_of(&private, Some("pat@example.com")).await, "pm");
		assert_eq!(
			level_of(&private, Some("PAT@EXAMPLE.COM")).await,
			"pm",
			"a list matches a login whatever its case"
		);
		assert_eq!(level_of(&private, Some("boss@example.com")).await, "admin");
		assert_eq!(
			level_of(&private, Some("stranger@example.com")).await,
			"all"
		);
		assert_eq!(
			level_of(&private, None).await,
			Value::Null,
			"no tailnet identity holds no level"
		);
	})
	.await;
}

// spec: ADM#the-canopy-grant
// spec: ADM#resolving-grant-sources
#[tokio::test(flavor = "multi_thread")]
async fn the_policy_confers_levels_alongside_the_lists() {
	trust_headers();

	let policy = json!({
		"groups": {
			"group:pm": ["Pat@BES.au"],
			"group:ops": ["lee@bes.au"],
			"group:elsewhere": ["sam@bes.au"],
		},
		"grants": [
			{
				"app": { "bes.au/cap/canopy": [{ "pm": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:pm"],
			},
			{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:ops"],
			},
			{
				"app": { "bes.au/cap/canopy": [{ "pm": true }] },
				"dst": ["tag:server-other"],
				"src": ["group:elsewhere"],
			},
		],
	});
	run_with_policy(Some(policy), async |mut conn, private, _| {
		// Listed PM, granted admin by the policy: the higher wins.
		add(&mut conn, ListLevel::Pm, "lee@bes.au").await;

		assert_eq!(
			level_of(&private, Some("pat@bes.au")).await,
			"pm",
			"a policy login matches whatever its case"
		);
		assert_eq!(level_of(&private, Some("lee@bes.au")).await, "admin");
		assert_eq!(
			level_of(&private, Some("sam@bes.au")).await,
			"all",
			"a grant to another tag confers nothing"
		);
	})
	.await;
}

// spec: ADM#resolving-grant-sources
#[tokio::test(flavor = "multi_thread")]
async fn a_grant_to_star_gives_its_level_to_every_tailnet_user() {
	trust_headers();

	let policy = json!({
		"grants": [{
			"app": { "bes.au/cap/canopy": [{ "pm": true }] },
			"dst": ["tag:server-canopy"],
			"src": ["*"],
		}],
	});
	run_with_policy(Some(policy), async |_, private, _| {
		assert_eq!(level_of(&private, Some("anyone@example.com")).await, "pm");
		assert_eq!(level_of(&private, None).await, Value::Null);
	})
	.await;
}

// spec: ADM#freshness-and-availability
#[tokio::test(flavor = "multi_thread")]
async fn an_unreadable_policy_withdraws_no_level() {
	trust_headers();

	let policy = json!({
		"grants": [{
			"app": { "bes.au/cap/canopy": [{ "pm": true }] },
			"dst": ["tag:server-canopy"],
			"src": ["pat@bes.au"],
		}],
	});
	run_with_policy(Some(policy), async |mut conn, private, directory| {
		add(&mut conn, ListLevel::Pm, "lee@bes.au").await;

		directory
			.expect("a directory")
			.refresh_policy()
			.await
			.expect_err("the test directory's API is unreachable");

		assert_eq!(level_of(&private, Some("pat@bes.au")).await, "pm");
		assert_eq!(level_of(&private, Some("lee@bes.au")).await, "pm");
	})
	.await;
}

// spec: ADM#freshness-and-availability
#[tokio::test(flavor = "multi_thread")]
async fn taking_a_level_takes_effect_on_the_next_request() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		add(&mut conn, ListLevel::Admin, "boss@example.com").await;
		assert_eq!(level_of(&private, Some("boss@example.com")).await, "admin");

		AccessListEntry::remove(&mut conn, ListLevel::Admin, "boss@example.com")
			.await
			.expect("remove");
		assert_eq!(level_of(&private, Some("boss@example.com")).await, "all");
	})
	.await;
}
