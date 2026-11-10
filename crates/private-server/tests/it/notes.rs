use commons_tests::diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

async fn seed_issue_and_incident(
	conn: &mut commons_tests::diesel_async::AsyncPgConnection,
	private: &commons_tests::axum_test::TestServer,
) -> (Uuid, Uuid) {
	let server_id = Uuid::new_v4();
	let group_id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g'); \
		 INSERT INTO machines (name, id, group_id) VALUES ('box', '{server_id}', '{group_id}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
			('{server_id}', 'https://example.com', 'tamanu-central', '{group_id}', 'production', '{server_id}');"
	))
	.await
	.expect("seed server");

	let r = private
		.post("/api/issues/submit_manual_event")
		.json(&serde_json::json!({
			"applicationId": server_id,
			"ref": "x",
			"result": "failed",
			"message": "trouble",
		}))
		.await;
	r.assert_status_ok();
	let issue_id = Uuid::parse_str(
		r.json::<serde_json::Value>()
			.get("id")
			.unwrap()
			.as_str()
			.unwrap(),
	)
	.unwrap();

	let resp = private
		.post("/api/incidents/list_for_server")
		.json(&serde_json::json!({ "server_id": server_id }))
		.await;
	let items: Vec<serde_json::Value> = resp.json();
	let incident_id = Uuid::parse_str(items[0].get("id").unwrap().as_str().unwrap()).unwrap();
	(issue_id, incident_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn issue_notes_lifecycle() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (issue_id, _) = seed_issue_and_incident(&mut conn, &private).await;

		let add = private
			.post("/api/issues/add_note")
			.json(&serde_json::json!({ "issue_id": issue_id, "body": "first note" }))
			.await;
		add.assert_status_ok();
		let first_id = Uuid::parse_str(
			add.json::<serde_json::Value>()
				.get("id")
				.unwrap()
				.as_str()
				.unwrap(),
		)
		.unwrap();
		assert_eq!(
			add.json::<serde_json::Value>()
				.get("author")
				.and_then(|v| v.as_str()),
			Some("admin@localhost")
		);

		// Add a second so we test ordering.
		private
			.post("/api/issues/add_note")
			.json(&serde_json::json!({ "issue_id": issue_id, "body": "second note" }))
			.await
			.assert_status_ok();

		let listed = private
			.post("/api/issues/list_notes")
			.json(&serde_json::json!({ "issue_id": issue_id }))
			.await;
		listed.assert_status_ok();
		let items: Vec<serde_json::Value> = listed.json();
		assert_eq!(items.len(), 2);
		assert_eq!(
			items[0].get("body").and_then(|v| v.as_str()),
			Some("second note"),
			"newest first",
		);

		// Delete the first.
		private
			.post("/api/issues/delete_note")
			.json(&serde_json::json!({ "note_id": first_id.to_string() }))
			.await
			.assert_status_ok();
		let listed = private
			.post("/api/issues/list_notes")
			.json(&serde_json::json!({ "issue_id": issue_id }))
			.await;
		let items: Vec<serde_json::Value> = listed.json();
		assert_eq!(items.len(), 1);
		assert_eq!(
			items[0].get("body").and_then(|v| v.as_str()),
			Some("second note")
		);

		// Reject empty body.
		let bad = private
			.post("/api/issues/add_note")
			.json(&serde_json::json!({ "issue_id": issue_id, "body": "   " }))
			.await;
		assert!(!bad.status_code().is_success());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn incident_notes_lifecycle() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (_, incident_id) = seed_issue_and_incident(&mut conn, &private).await;

		private
			.post("/api/incidents/add_note")
			.json(&serde_json::json!({ "incident_id": incident_id, "body": "investigating" }))
			.await
			.assert_status_ok();

		let listed = private
			.post("/api/incidents/list_notes")
			.json(&serde_json::json!({ "incident_id": incident_id }))
			.await;
		let items: Vec<serde_json::Value> = listed.json();
		assert_eq!(items.len(), 1);
		assert_eq!(
			items[0].get("body").and_then(|v| v.as_str()),
			Some("investigating")
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_deleted_with_issue() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (issue_id, _) = seed_issue_and_incident(&mut conn, &private).await;

		private
			.post("/api/issues/add_note")
			.json(&serde_json::json!({ "issue_id": issue_id, "body": "note" }))
			.await
			.assert_status_ok();

		conn.batch_execute(&format!("DELETE FROM issues WHERE id = '{issue_id}'"))
			.await
			.expect("delete issue");

		// FK ON DELETE CASCADE: notes should be gone.
		let listed = private
			.post("/api/issues/list_notes")
			.json(&serde_json::json!({ "issue_id": issue_id }))
			.await;
		let items: Vec<serde_json::Value> = listed.json();
		assert!(items.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_note_is_kept_audited_and_shown_to_administrators_who_ask() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (issue_id, incident_id) = seed_issue_and_incident(&mut conn, &private).await;

		let note: serde_json::Value = private
			.post("/api/incidents/add_note")
			.json(&serde_json::json!({ "incident_id": incident_id, "body": "paged the DBA" }))
			.await
			.json();
		assert_eq!(note["author_kind"], "operator");
		assert_eq!(note["revision"], 1);
		let note_id = note["id"].as_str().unwrap().to_owned();
		private
			.post("/api/issues/add_note")
			.json(&serde_json::json!({ "issue_id": issue_id, "body": "seen" }))
			.await
			.assert_status_ok();

		private
			.post("/api/incidents/delete_note")
			.json(&serde_json::json!({ "note_id": note_id }))
			.await
			.assert_status_ok();
		// Deleting again, or something that is not there, changes nothing.
		private
			.post("/api/incidents/delete_note")
			.json(&serde_json::json!({ "note_id": note_id }))
			.await
			.assert_status_ok();
		private
			.post("/api/incidents/delete_note")
			.json(&serde_json::json!({ "note_id": Uuid::new_v4() }))
			.await
			.assert_status_ok();

		let kept: Vec<serde_json::Value> = private
			.post("/api/incidents/list_notes")
			.json(&serde_json::json!({ "incident_id": incident_id }))
			.await
			.json();
		assert!(kept.is_empty());
		let all: Vec<serde_json::Value> = private
			.post("/api/incidents/list_notes")
			.json(&serde_json::json!({ "incident_id": incident_id, "include_deleted": true }))
			.await
			.json();
		assert_eq!(all.len(), 1);
		assert_eq!(all[0]["deleted_by"], "admin@localhost");
		assert!(!all[0]["deleted_at"].is_null());

		for (action, count) in [
			("incident_note_added", 1),
			("issue_note_added", 1),
			("note_deleted", 1),
		] {
			let entries = database::audit::find(
				&mut conn,
				&database::audit::Filter {
					action: Some(action.into()),
					limit: 10,
					..Default::default()
				},
			)
			.await
			.unwrap();
			assert_eq!(entries.len(), count, "{action}");
			let rendered = serde_json::to_string(&entries[0].detail).unwrap();
			assert!(!rendered.contains("paged"), "no note text in {rendered}");
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn only_administrators_are_shown_deleted_notes() {
	// SAFETY: single-threaded test process (nextest), env read only by auth.
	unsafe { std::env::set_var("CANOPY_TRUST_TAILSCALE_HEADERS", "1") };

	commons_tests::server::run(async |mut conn, _public, private| {
		database::admins::Admin::add(&mut conn, "boss@example.com")
			.await
			.unwrap();
		let server_id = Uuid::new_v4();
		let group_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g'); \
			 INSERT INTO machines (name, id, group_id) VALUES ('box', '{server_id}', '{group_id}'); \
			 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
				('{server_id}', 'https://example.com', 'tamanu-central', '{group_id}', 'production', '{server_id}'); \
			 INSERT INTO incidents (id, server_group_id, rank, opened_at) VALUES ('{server_id}', '{group_id}', 'production', NOW()); \
			 INSERT INTO incident_notes (id, incident_id, author, body, deleted_at, deleted_by) \
				VALUES ('{group_id}', '{server_id}', 'op', 'gone', NOW(), 'op');"
		))
		.await
		.unwrap();

		let ask = |who: &'static str| {
			private
				.post("/api/incidents/list_notes")
				.add_header("Tailscale-User-Login", who)
				.add_header("Tailscale-User-Name", who)
				.json(&serde_json::json!({ "incident_id": server_id, "include_deleted": true }))
		};
		let boss: Vec<serde_json::Value> = ask("boss@example.com").await.json();
		assert_eq!(boss.len(), 1);
		let user: Vec<serde_json::Value> = ask("user@example.com").await.json();
		assert!(user.is_empty());
	})
	.await;
}
