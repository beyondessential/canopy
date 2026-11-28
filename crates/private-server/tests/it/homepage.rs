//! The operator's homepage: the page they chose, and the policy's default.
//!
//! Spec: `.workhorse/specs/private-server/homepage.md` (id `HOME`).
//!
//! Every request here is made through the real identity path and with no
//! session, so each succeeding shows the handlers are reachable read-only.

use commons_tests::axum_test::TestServer;
use serde_json::{Value, json};

use crate::access_levels::{run_with_policy, trust_headers};

async fn post(private: &TestServer, path: &str, login: &str, body: Value) -> (u16, Value) {
	let response = private
		.post(path)
		.add_header("Tailscale-User-Login", login)
		.add_header("Tailscale-User-Name", "Someone")
		.json(&body)
		.await;
	(
		response.status_code().as_u16(),
		response.text().parse().unwrap_or(Value::Null),
	)
}

async fn homepage(private: &TestServer, login: &str) -> Value {
	let (status, body) = post(private, "/api/homepage/get", login, json!({})).await;
	assert_eq!(status, 200, "{body}");
	body
}

// spec: HOME#the-default-homepage
#[tokio::test(flavor = "multi_thread")]
async fn with_nothing_chosen_and_no_home_granted_there_is_neither() {
	trust_headers();

	run_with_policy(None, async |_, private, _| {
		assert_eq!(
			homepage(&private, "pat@bes.au").await,
			json!({ "chosen": null, "default": null })
		);
	})
	.await;
}

// spec: HOME#the-default-homepage
#[tokio::test(flavor = "multi_thread")]
async fn the_default_is_the_home_the_policy_grants_the_caller() {
	trust_headers();

	let policy = json!({
		"groups": { "group:pm": ["Pat@BES.au"] },
		"grants": [
			{
				"app": { "bes.au/cap/canopy": [{ "pm": true, "home": "/fleet/pm" }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:pm"],
			},
			{
				"app": { "bes.au/cap/canopy": [{ "home": "/elsewhere", "priority": 9 }] },
				"dst": ["tag:server-other"],
				"src": ["*"],
			},
		],
	});
	run_with_policy(Some(policy), async |_, private, _| {
		assert_eq!(
			homepage(&private, "pat@bes.au").await,
			json!({ "chosen": null, "default": "/fleet/pm" })
		);
		assert_eq!(
			homepage(&private, "someone@bes.au").await,
			json!({ "chosen": null, "default": null }),
			"a home on a grant to another tag is ignored"
		);
	})
	.await;
}

// spec: HOME#choosing-a-homepage
#[tokio::test(flavor = "multi_thread")]
async fn a_choice_is_set_replaced_and_cleared_under_the_login_whatever_its_case() {
	trust_headers();

	let policy = json!({
		"grants": [{
			"app": { "bes.au/cap/canopy": [{ "home": "/fleet/pm" }] },
			"dst": ["tag:server-canopy"],
			"src": ["*"],
		}],
	});
	run_with_policy(Some(policy), async |_, private, _| {
		let (status, body) = post(
			&private,
			"/api/homepage/set",
			"Pat@BES.au",
			json!({ "path": "/fleet/groups" }),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(
			homepage(&private, "pat@bes.au").await,
			json!({ "chosen": "/fleet/groups", "default": "/fleet/pm" })
		);
		assert_eq!(
			homepage(&private, "other@bes.au").await["chosen"],
			Value::Null,
			"a choice is the login's own"
		);

		let (status, body) = post(
			&private,
			"/api/homepage/set",
			"pat@bes.au",
			json!({ "path": "/status/" }),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(homepage(&private, "PAT@bes.au").await["chosen"], "/status/");

		let (status, body) = post(&private, "/api/homepage/clear", "pat@bes.au", json!({})).await;
		assert_eq!(status, 200, "{body}");
		assert_eq!(
			homepage(&private, "pat@bes.au").await,
			json!({ "chosen": null, "default": "/fleet/pm" })
		);

		// Clearing with nothing chosen is not an error.
		let (status, body) = post(&private, "/api/homepage/clear", "pat@bes.au", json!({})).await;
		assert_eq!(status, 200, "{body}");
	})
	.await;
}

// spec: HOME#the-default-homepage
#[tokio::test(flavor = "multi_thread")]
async fn a_path_that_is_not_a_pages_is_refused() {
	trust_headers();

	run_with_policy(None, async |_, private, _| {
		for path in [
			"",
			"/",
			"fleet",
			"//evil.example",
			"https://evil.example/status",
			"/status?filter=x",
			"/status#top",
		] {
			let (status, body) = post(
				&private,
				"/api/homepage/set",
				"pat@bes.au",
				json!({ "path": path }),
			)
			.await;
			assert_eq!(status, 400, "{path:?}: {body}");
		}
		assert_eq!(
			homepage(&private, "pat@bes.au").await["chosen"],
			Value::Null
		);
	})
	.await;
}
