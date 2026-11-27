//! The permissions page's lists and what the policy grants each level.
//!
//! Spec: `.workhorse/specs/private-server/access-levels.md` (id `ADM`), "The
//! permissions page".

use commons_tests::axum_test::TestServer;
use database::{
	access_lists::{AccessListEntry, ListLevel},
	audit::{self, Actor, Entry, Filter},
	diesel_async::AsyncPgConnection,
};
use serde_json::{Value, json};

use crate::access_levels::{level_of, run_with_policy, trust_headers};

const ADMIN: &str = "boss@example.com";
const SESSION_HEADER: &str = "x-canopy-session";

async fn post(private: &TestServer, path: &str, login: Option<&str>, body: Value) -> Value {
	post_in(private, path, login, None, body).await
}

/// Post as a login, in a session where one is given, returning the status and
/// body as one value so a refusal can be checked by both.
async fn post_in(
	private: &TestServer,
	path: &str,
	login: Option<&str>,
	session: Option<&str>,
	body: Value,
) -> Value {
	let mut request = private.post(path);
	if let Some(login) = login {
		request = request
			.add_header("Tailscale-User-Login", login)
			.add_header("Tailscale-User-Name", "Someone");
	}
	if let Some(session) = session {
		request = request.add_header(SESSION_HEADER, session);
	}
	let response = request.json(&body).await;
	json!({
		"status": response.status_code().as_u16(),
		"body": response.text().parse::<Value>().unwrap_or(Value::Null),
	})
}

/// A session for the login, raised to the mode.
async fn raised(private: &TestServer, login: &str, mode: &str) -> String {
	let raised = post(
		private,
		"/api/safety/raise",
		Some(login),
		json!({ "mode": mode }),
	)
	.await;
	assert_eq!(raised["status"], 200, "{raised}");
	raised["body"]["id"]
		.as_str()
		.expect("session id")
		.to_owned()
}

fn problem(response: &Value) -> &str {
	response["body"]["type"].as_str().unwrap_or_default()
}

async fn log(conn: &mut AsyncPgConnection, action: &str) -> Vec<Entry> {
	audit::find(
		conn,
		&Filter {
			action: Some(action.to_owned()),
			limit: 100,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

// spec: ADM#the-permissions-page
#[tokio::test(flavor = "multi_thread")]
async fn the_list_names_each_levels_entries_and_the_sources_the_policy_grants_it_to() {
	trust_headers();

	let policy = json!({
		"groups": { "group:pm": ["a@bes.au", "b@bes.au", "c@bes.au"] },
		"grants": [
			{
				"app": { "bes.au/cap/canopy": [{ "pm": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["group:pm", "Dana@BES.au", "*", "autogroup:member", "group:missing", "autogroup:admin"],
			},
			{
				"app": { "bes.au/cap/canopy": [{ "admin": true }] },
				"dst": ["tag:server-canopy"],
				"src": ["ops@bes.au"],
			},
		],
	});
	run_with_policy(Some(policy), async |mut conn, private, _| {
		AccessListEntry::add(&mut conn, ListLevel::Admin, ADMIN)
			.await
			.unwrap();
		AccessListEntry::add(&mut conn, ListLevel::Pm, "Pat@Example.com")
			.await
			.unwrap();

		let listed = post(&private, "/api/access/list", Some(ADMIN), json!({})).await;
		assert_eq!(listed["status"], 200, "{listed}");
		let overview = &listed["body"];

		let levels = overview["levels"].as_array().unwrap();
		let shape: Vec<_> = levels
			.iter()
			.map(|level| {
				(
					level["level"].clone(),
					level["key"].clone(),
					level["list"]
						.as_array()
						.unwrap()
						.iter()
						.map(|entry| entry["login"].clone())
						.collect::<Vec<_>>(),
				)
			})
			.collect();
		assert_eq!(
			shape,
			vec![
				(json!("admin"), json!("admin"), vec![json!(ADMIN)]),
				(json!("pm"), json!("pm"), vec![json!("pat@example.com")]),
			],
			"admin then PM, each with its key and its list"
		);

		assert_eq!(
			levels[0]["granted_to"],
			json!([{ "source": "ops@bes.au", "covers": { "kind": "person" } }]),
		);
		assert_eq!(
			levels[1]["granted_to"],
			json!([
				{ "source": "group:pm", "covers": { "kind": "group", "members": 3 } },
				{ "source": "Dana@BES.au", "covers": { "kind": "person" } },
				{ "source": "*", "covers": { "kind": "everyone" } },
				{ "source": "autogroup:member", "covers": { "kind": "everyone" } },
				{ "source": "group:missing", "covers": { "kind": "no_one" } },
				{ "source": "autogroup:admin", "covers": { "kind": "no_one" } },
			]),
		);

		let policy = &overview["policy"];
		assert_eq!(policy["configured"], true);
		assert!(policy["read_at"].is_string(), "{policy}");
		assert_eq!(policy["capability"], "bes.au/cap/canopy");
		assert_eq!(policy["service_tag"], "tag:server-canopy");
	})
	.await;
}

// spec: ADM#the-permissions-page
#[tokio::test(flavor = "multi_thread")]
async fn without_a_tailnet_api_the_list_says_canopy_is_not_configured_to_read_the_policy() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		AccessListEntry::add(&mut conn, ListLevel::Admin, ADMIN)
			.await
			.unwrap();

		let listed = post(&private, "/api/access/list", Some(ADMIN), json!({})).await;
		assert_eq!(listed["status"], 200, "{listed}");
		let overview = &listed["body"];
		assert_eq!(overview["policy"]["configured"], false);
		assert_eq!(overview["policy"]["read_at"], Value::Null);
		for level in overview["levels"].as_array().unwrap() {
			assert_eq!(level["granted_to"], json!([]), "{level}");
		}
	})
	.await;
}

// spec: ADM#the-permissions-page
#[tokio::test(flavor = "multi_thread")]
async fn an_admin_adds_and_removes_a_login_in_danger_mode_and_each_is_audited() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		AccessListEntry::add(&mut conn, ListLevel::Admin, ADMIN)
			.await
			.unwrap();
		let entry = json!({ "level": "pm", "login": "Pat@Example.com" });

		// Write mode is not enough for either.
		let write = raised(&private, ADMIN, "write").await;
		for path in ["/api/access/add", "/api/access/remove"] {
			let refused = post_in(&private, path, Some(ADMIN), Some(&write), entry.clone()).await;
			assert_eq!(refused["status"], 403, "{path}: {refused}");
			assert!(
				problem(&refused).ends_with("safety-mode-too-low"),
				"{path}: {refused}"
			);
		}

		let danger = raised(&private, ADMIN, "danger").await;
		let added = post_in(
			&private,
			"/api/access/add",
			Some(ADMIN),
			Some(&danger),
			entry.clone(),
		)
		.await;
		assert_eq!(added["status"], 200, "{added}");
		assert_eq!(level_of(&private, Some("pat@example.com")).await, "pm");

		// Adding a login already on the list changes nothing and records nothing.
		let again = post_in(
			&private,
			"/api/access/add",
			Some(ADMIN),
			Some(&danger),
			json!({ "level": "pm", "login": "pat@example.com" }),
		)
		.await;
		assert_eq!(again["status"], 200, "{again}");

		let entries = log(&mut conn, "access_list_entry_added").await;
		assert_eq!(entries.len(), 1, "one entry, none for the repeat");
		assert_eq!(
			entries[0].detail,
			json!({ "level": "pm", "login": "pat@example.com" })
		);
		assert!(
			matches!(&entries[0].actor, Actor::Operator { login, .. } if login == ADMIN),
			"{:?}",
			entries[0].actor
		);

		let removed = post_in(
			&private,
			"/api/access/remove",
			Some(ADMIN),
			Some(&danger),
			entry.clone(),
		)
		.await;
		assert_eq!(removed["status"], 200, "{removed}");
		assert_eq!(level_of(&private, Some("pat@example.com")).await, "all");

		let entries = log(&mut conn, "access_list_entry_removed").await;
		assert_eq!(entries.len(), 1);
		assert_eq!(
			entries[0].detail,
			json!({ "level": "pm", "login": "pat@example.com" })
		);
	})
	.await;
}

// spec: ADM#the-permissions-page
#[tokio::test(flavor = "multi_thread")]
async fn only_an_administrator_reads_or_edits_the_lists() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		AccessListEntry::add(&mut conn, ListLevel::Pm, "pat@example.com")
			.await
			.unwrap();
		let entry = json!({ "level": "pm", "login": "someone@example.com" });

		let unidentified = post(&private, "/api/access/list", None, json!({})).await;
		assert_eq!(unidentified["status"], 401, "{unidentified}");

		for login in ["pat@example.com", "stranger@example.com"] {
			let listed = post(&private, "/api/access/list", Some(login), json!({})).await;
			assert_eq!(listed["status"], 403, "{login}: {listed}");
			assert!(
				problem(&listed).ends_with("auth-insufficient-permissions"),
				"{login}: {listed}"
			);

			// Raised as far as a session goes, so the refusal is the level's.
			let danger = raised(&private, login, "danger").await;
			for path in ["/api/access/add", "/api/access/remove"] {
				let refused =
					post_in(&private, path, Some(login), Some(&danger), entry.clone()).await;
				assert_eq!(refused["status"], 403, "{login} {path}: {refused}");
				assert!(
					problem(&refused).ends_with("auth-insufficient-permissions"),
					"{login} {path}: {refused}"
				);
			}
		}
		assert_eq!(level_of(&private, Some("someone@example.com")).await, "all");
	})
	.await;
}
