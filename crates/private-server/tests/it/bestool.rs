use uuid::Uuid;

/// `get_snippet` declares 404 for a snippet that isn't there, but built the
/// miss with `AppError::custom`, which maps to 500 `/errors/other`. A client
/// following the checked-in contract saw a server fault where it should have
/// seen an ordinary miss.
#[tokio::test(flavor = "multi_thread")]
async fn get_snippet_is_404_for_an_unknown_id() {
	commons_tests::server::run(async |_conn, _public, private| {
		let resp = private
			.post("/api/bestool/get_snippet")
			.json(&serde_json::json!({ "id": Uuid::new_v4() }))
			.await;

		assert_eq!(resp.status_code().as_u16(), 404, "body: {}", resp.text());
		let body: serde_json::Value = resp.json();
		assert!(
			body["type"]
				.as_str()
				.is_some_and(|t| t.ends_with("resource-not-found")),
			"a miss should carry the resource-not-found problem type, got {body}",
		);
	})
	.await
}

mod levels {
	//! Who may change the snippet library. Reading it is open to every
	//! operator; saving and deleting take an administrator.
	//!
	//! Spec: `.workhorse/specs/private-server/access-levels.md` (id `ADM`).

	use commons_tests::axum_test::TestServer;
	use database::access_lists::{AccessListEntry, ListLevel};
	use serde_json::{Value, json};

	use crate::access_levels::{run_with_policy, trust_headers};

	const ADMIN: &str = "boss@example.com";
	const OPERATOR: &str = "stranger@example.com";

	async fn post(
		private: &TestServer,
		path: &str,
		login: Option<&str>,
		session: Option<&str>,
		body: Value,
	) -> (u16, Value) {
		let mut request = private.post(path);
		if let Some(login) = login {
			request = request
				.add_header("Tailscale-User-Login", login)
				.add_header("Tailscale-User-Name", "Someone");
		}
		if let Some(session) = session {
			request = request.add_header("x-canopy-session", session);
		}
		let response = request.json(&body).await;
		(
			response.status_code().as_u16(),
			response.text().parse().unwrap_or(Value::Null),
		)
	}

	/// A write-mode session for the login, so a refusal is the level's.
	async fn writing(private: &TestServer, login: &str) -> String {
		let (status, body) = post(
			private,
			"/api/safety/raise",
			Some(login),
			None,
			json!({ "mode": "write" }),
		)
		.await;
		assert_eq!(status, 200, "{body}");
		body["id"].as_str().expect("session id").to_owned()
	}

	fn snippet() -> Value {
		json!({ "name": "count-users", "sql": "select count(*) from users" })
	}

	// spec: ADM#the-levels
	#[tokio::test(flavor = "multi_thread")]
	async fn an_operator_reads_snippets_but_only_an_admin_saves_or_deletes_one() {
		trust_headers();

		run_with_policy(None, async |mut conn, private, _| {
			AccessListEntry::add(&mut conn, ListLevel::Admin, ADMIN)
				.await
				.unwrap();
			let admin = writing(&private, ADMIN).await;
			let operator = writing(&private, OPERATOR).await;

			let (status, saved) = post(
				&private,
				"/api/bestool/save_snippet",
				Some(ADMIN),
				Some(&admin),
				snippet(),
			)
			.await;
			assert_eq!(status, 200, "{saved}");
			let id = saved["id"].clone();

			let (status, listed) = post(
				&private,
				"/api/bestool/list_snippets",
				Some(OPERATOR),
				None,
				json!({ "offset": 0 }),
			)
			.await;
			assert_eq!(status, 200, "{listed}");
			assert_eq!(listed["total"], 1);
			let (status, opened) = post(
				&private,
				"/api/bestool/get_snippet",
				Some(OPERATOR),
				None,
				json!({ "id": id }),
			)
			.await;
			assert_eq!(status, 200, "{opened}");
			assert_eq!(opened["name"], "count-users");

			for (path, body) in [
				("/api/bestool/save_snippet", snippet()),
				("/api/bestool/delete_snippet", json!({ "id": id })),
			] {
				let (status, refused) =
					post(&private, path, Some(OPERATOR), Some(&operator), body).await;
				assert_eq!(status, 403, "{path}: {refused}");
				assert!(
					refused["type"]
						.as_str()
						.is_some_and(|t| t.ends_with("auth-insufficient-permissions")),
					"{path}: {refused}"
				);
			}

			let (status, body) = post(
				&private,
				"/api/bestool/delete_snippet",
				Some(ADMIN),
				Some(&admin),
				json!({ "id": id }),
			)
			.await;
			assert_eq!(status, 200, "{body}");
			let (_, listed) = post(
				&private,
				"/api/bestool/list_snippets",
				Some(OPERATOR),
				None,
				json!({ "offset": 0 }),
			)
			.await;
			assert_eq!(listed["total"], 0);
		})
		.await;
	}

	// spec: ADM#the-levels
	#[tokio::test(flavor = "multi_thread")]
	async fn deleting_a_snippet_without_a_tailnet_identity_is_refused() {
		trust_headers();

		run_with_policy(None, async |_, private, _| {
			let (status, body) = post(
				&private,
				"/api/bestool/delete_snippet",
				None,
				None,
				json!({ "id": uuid::Uuid::new_v4() }),
			)
			.await;
			assert_eq!(status, 401, "{body}");
		})
		.await;
	}
}
