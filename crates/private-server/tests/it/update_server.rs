use axum::http::StatusCode;
use commons_tests::diesel_async::SimpleAsyncConnection;
use database::{applications::Application, machines::Machine};
use serde_json::json;

#[tokio::test(flavor = "multi_thread")]
async fn update_server_basic_fields() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '22222222-2222-2222-2222-222222222222') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('22222222-2222-2222-2222-222222222222', 'Original Application', 'https://original.example.com', 'test', 'tamanu-central', '22222222-2222-2222-2222-222222222222')"
		)
		.await
		.unwrap();

		conn.batch_execute("INSERT INTO admins (email) VALUES ('admin@example.com')")
			.await
			.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "22222222-2222-2222-2222-222222222222",
				"data": {
					"name": "Updated Application",
					"host": "https://updated.example.com",
					"rank": "production"
				}
			}))
			.await;
		response.assert_status_ok();
		// update returns Result<()>, no response body
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_partial_update() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '33333333-3333-3333-3333-333333333333') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('33333333-3333-3333-3333-333333333333', 'Partial Application', 'https://partial.example.com', 'demo', 'tamanu-central', '33333333-3333-3333-3333-333333333333')"
		)
		.await
		.unwrap();

		conn.batch_execute("INSERT INTO admins (email) VALUES ('admin@example.com')")
			.await
			.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "33333333-3333-3333-3333-333333333333",
				"data": {
					"rank": "production"
				}
			}))
			.await;
		response.assert_status_ok();
		// update returns Result<()>, no response body
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_device_id() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO devices (id, role) VALUES
			('44444444-4444-4444-4444-444444444444', 'server')"
		)
		.await
		.unwrap();

		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '55555555-5555-5555-5555-555555555555') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('55555555-5555-5555-5555-555555555555', 'Device Application', 'https://device.example.com', 'production', 'tamanu-central', '55555555-5555-5555-5555-555555555555')"
		)
		.await
		.unwrap();

		conn.batch_execute("INSERT INTO admins (email) VALUES ('admin@example.com')")
			.await
			.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "55555555-5555-5555-5555-555555555555",
				"data": {
					"device_id": "44444444-4444-4444-4444-444444444444"
				}
			}))
			.await;
		response.assert_status_ok();
		// update returns Result<()>, no response body
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_invalid_rank() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '66666666-6666-6666-6666-666666666666') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('66666666-6666-6666-6666-666666666666', 'Rank Application', 'https://rank.example.com', 'test', 'tamanu-central', '66666666-6666-6666-6666-666666666666')"
		)
		.await
		.unwrap();

		conn.batch_execute("INSERT INTO admins (email) VALUES ('admin@example.com')")
			.await
			.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "22222222-2222-2222-2222-222222222222",
				"data": {
					"rank": "invalid"
				}
			}))
			.await;
		// axum's Json extractor rejects unknown enum variants with 422
		response.assert_status(StatusCode::UNPROCESSABLE_ENTITY);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_not_found() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute("INSERT INTO admins (email) VALUES ('admin@example.com')")
			.await
			.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "77777777-7777-7777-7777-777777777777",
				"data": {}
			}))
			.await;
		// A missing server is a 404. It used to be a 500 only because diesel
		// refused to build the empty changeset before anything looked the
		// server up — the endpoint never actually checked that it existed.
		response.assert_status(StatusCode::NOT_FOUND);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_group_id() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO server_groups (id, name) VALUES
			('88888888-8888-8888-8888-888888888888', 'Group A');
			WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '99999999-9999-9999-9999-999999999999') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('99999999-9999-9999-9999-999999999999', 'Member', 'https://member.example.com', 'production', 'tamanu-facility', '99999999-9999-9999-9999-999999999999');
			INSERT INTO admins (email) VALUES ('admin@example.com')",
		)
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "99999999-9999-9999-9999-999999999999",
				"data": {
					"group_id": "88888888-8888-8888-8888-888888888888"
				}
			}))
			.await;
		response.assert_status_ok();

		let server_info =
			Application::get_by_id(&mut conn, "99999999-9999-9999-9999-999999999999".parse().unwrap())
				.await
				.unwrap();

		assert_eq!(
			server_info.group_id,
			Some("88888888-8888-8888-8888-888888888888".parse().unwrap())
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_clear_group_id() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO server_groups (id, name) VALUES
			('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'Group');
			WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa') RETURNING id) INSERT INTO applications (id, name, host, rank, type, group_id, machine_id) VALUES
			('bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'Member', 'https://m2.example.com', 'production', 'tamanu-facility', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb');
			INSERT INTO admins (email) VALUES ('admin@example.com')",
		)
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
				"data": {
					"group_id": null
				}
			}))
			.await;
		response.assert_status_ok();

		let server_info =
			Application::get_by_id(&mut conn, "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".parse().unwrap())
				.await
				.unwrap();

		assert_eq!(server_info.group_id, None);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_notes_and_tags() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', 'cccccccc-cccc-cccc-cccc-cccccccccccc') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('cccccccc-cccc-cccc-cccc-cccccccccccc', 'Tagged Application', 'https://tagged.example.com', 'production', 'tamanu-central', 'cccccccc-cccc-cccc-cccc-cccccccccccc');
			INSERT INTO admins (email) VALUES ('admin@example.com')",
		)
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "cccccccc-cccc-cccc-cccc-cccccccccccc",
				"data": {
					"notes": "ops handover note",
					"tags": { "env": "prod", "tier": "1" }
				}
			}))
			.await;
		response.assert_status_ok();

		let server_info =
			Application::get_by_id(&mut conn, "cccccccc-cccc-cccc-cccc-cccccccccccc".parse().unwrap())
				.await
				.unwrap();
		assert_eq!(server_info.notes, "ops handover note");
		assert_eq!(server_info.tags.0.get("env"), Some(&"prod".to_string()));
		assert_eq!(server_info.tags.0.get("tier"), Some(&"1".to_string()));
	})
	.await
}

/// Editing a workload says nothing about the box it runs on: the identity is
/// the machine's, and the application update path never touches it.
#[tokio::test(flavor = "multi_thread")]
async fn update_server_leaves_the_machine_identity_alone() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO devices (id, role) VALUES
			('aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa', 'server')"
		)
		.await
		.unwrap();

		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id, device_id) VALUES ('box', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb', 'Device Application', 'https://device.example.com', 'production', 'tamanu-central', 'bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb')"
		)
		.await
		.unwrap();

		conn.batch_execute("INSERT INTO admins (email) VALUES ('admin@example.com')")
			.await
			.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb",
				"data": {
					"name": "Updated Name",
					"host": "https://updated.example.com"
				}
			}))
			.await;
		response.assert_status_ok();

		let server_info = Application::get_by_id(&mut conn, "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".parse().unwrap())
			.await
			.unwrap();
		assert_eq!(server_info.name, Some("Updated Name".to_string()));
		assert_eq!(
			server_info.host.as_ref().unwrap().0.to_string(),
			"https://updated.example.com/"
		);

		let machine = Machine::get_by_id(&mut conn, "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb".parse().unwrap())
			.await
			.unwrap();
		assert_eq!(
			machine.device_id,
			Some("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa".parse().unwrap()),
			"the box keeps its identity across an application update"
		);
	})
	.await
}

/// The per-server name-management grants (DOM) are withheld at creation and
/// carried by the ordinary update path, one independently of the other.
#[tokio::test(flavor = "multi_thread")]
async fn update_server_name_management_grants() {
	commons_tests::server::run(async |mut conn, _, private| {
		let id = "44444444-4444-4444-4444-444444444444";
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '{id}') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('{id}', 'DNS Application', 'https://dns.example.com', 'production', 'tamanu-central', '{id}')"
		))
		.await
		.unwrap();

		let server = Application::get_by_id(&mut conn, id.parse().unwrap())
			.await
			.unwrap();
		assert!(!server.may_manage_dns, "withheld until granted");
		assert!(!server.may_manage_tls, "withheld until granted");

		private
			.post("/api/fleet/applications/update")
			.json(&json!({"server_id": id, "data": {"may_manage_dns": true}}))
			.await
			.assert_status_ok();

		let server = Application::get_by_id(&mut conn, id.parse().unwrap())
			.await
			.unwrap();
		assert!(server.may_manage_dns);
		assert!(!server.may_manage_tls, "granting DNS must not grant TLS");

		// An update touching neither leaves both alone.
		private
			.post("/api/fleet/applications/update")
			.json(&json!({"server_id": id, "data": {"name": "Renamed"}}))
			.await
			.assert_status_ok();
		let server = Application::get_by_id(&mut conn, id.parse().unwrap())
			.await
			.unwrap();
		assert!(
			server.may_manage_dns,
			"an unrelated update must not revoke it"
		);

		// Revoked again.
		private
			.post("/api/fleet/applications/update")
			.json(
				&json!({"server_id": id, "data": {"may_manage_dns": false, "may_manage_tls": true}}),
			)
			.await
			.assert_status_ok();
		let server = Application::get_by_id(&mut conn, id.parse().unwrap())
			.await
			.unwrap();
		assert!(!server.may_manage_dns);
		assert!(server.may_manage_tls);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_server_blank_name_clears_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '77777777-7777-7777-7777-777777777777') RETURNING id) INSERT INTO applications (id, name, type, machine_id) VALUES
			('77777777-7777-7777-7777-777777777777', 'Named', 'postgres', '77777777-7777-7777-7777-777777777777')"
		)
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": "77777777-7777-7777-7777-777777777777",
				"data": { "name": "  " }
			}))
			.await;
		response.assert_status_ok();

		let application = Application::get_by_id(
			&mut conn,
			"77777777-7777-7777-7777-777777777777".parse().unwrap(),
		)
		.await
		.unwrap();
		assert_eq!(application.name, None);
		assert_eq!(application.display_name(), "Postgres");
	})
	.await
}

const BOX: &str = "66666666-6666-6666-6666-666666666666";
const CENTRAL: &str = "66666666-6666-6666-6666-666666666601";
const DATABASE: &str = "66666666-6666-6666-6666-666666666602";

async fn seed_box(conn: &mut commons_tests::diesel_async::AsyncPgConnection, rank: &str) {
	conn.batch_execute(&format!(
		"INSERT INTO machines (name, id) VALUES ('box', '{BOX}'); \
		 INSERT INTO applications (id, host, type, rank, machine_id) VALUES \
		 ('{CENTRAL}', 'https://central.example.com', 'tamanu-central', '{rank}', '{BOX}'), \
		 ('{DATABASE}', 'https://database.example.com', 'postgres', '{rank}', '{BOX}')"
	))
	.await
	.unwrap();
}

async fn ranks_on_the_box(
	conn: &mut commons_tests::diesel_async::AsyncPgConnection,
) -> Vec<Option<commons_types::server::rank::ServerRank>> {
	let machine = BOX.parse().unwrap();
	Machine::get_by_id(conn, machine)
		.await
		.unwrap()
		.applications(conn)
		.await
		.unwrap()
		.into_iter()
		.map(|application| application.rank)
		.collect()
}

/// A box serves one environment, so ranking one application ranks every
/// application on it.
// spec: GRP#environments
#[tokio::test(flavor = "multi_thread")]
async fn ranking_an_application_ranks_its_whole_box() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed_box(&mut conn, "test").await;

		private
			.post("/api/fleet/applications/update")
			.json(&json!({ "server_id": DATABASE, "data": { "rank": "demo" } }))
			.await
			.assert_status_ok();
		assert!(
			ranks_on_the_box(&mut conn)
				.await
				.iter()
				.all(|rank| *rank == Some(commons_types::server::rank::ServerRank::Demo))
		);

		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "rank": "clone" }))
			.await
			.assert_status_ok();
		assert!(
			ranks_on_the_box(&mut conn)
				.await
				.iter()
				.all(|rank| *rank == Some(commons_types::server::rank::ServerRank::Clone))
		);
	})
	.await
}

/// A rank is changed and never cleared: asking for it is refused and leaves the
/// box as it was.
// spec: GRP#environments
#[tokio::test(flavor = "multi_thread")]
async fn a_rank_cannot_be_cleared() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed_box(&mut conn, "production").await;
		let before = ranks_on_the_box(&mut conn).await;

		private
			.post("/api/fleet/applications/update")
			.json(&json!({ "server_id": CENTRAL, "data": { "rank": null } }))
			.await
			.assert_status_bad_request();
		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "rank": null, "name": "renamed" }))
			.await
			.assert_status_bad_request();

		assert_eq!(ranks_on_the_box(&mut conn).await, before);
		assert_eq!(
			Machine::get_by_id(&mut conn, BOX.parse().unwrap())
				.await
				.unwrap()
				.name,
			"box",
			"a refused edit applies none of it"
		);
	})
	.await
}

/// A machine edit that fails leaves the rank as it was, whichever part of the
/// edit is refused.
// spec: FLT#editing
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_edit_that_fails_does_not_change_its_rank() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed_box(&mut conn, "production").await;
		let before = ranks_on_the_box(&mut conn).await;

		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "rank": "dev", "name": "   " }))
			.await
			.assert_status_bad_request();

		assert_eq!(ranks_on_the_box(&mut conn).await, before);
	})
	.await
}

/// A machine carries its own rank, so one with nothing on it yet is ranked all
/// the same.
// spec: FLT#editing
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_with_no_application_can_be_ranked() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id) VALUES ('empty', '{BOX}')"
		))
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "rank": "test" }))
			.await;
		response.assert_status_ok();
		assert_eq!(response.json::<serde_json::Value>()["rank"], "test");

		let machine = Machine::get_by_id(&mut conn, BOX.parse().unwrap())
			.await
			.unwrap();
		assert_eq!(
			machine.rank,
			Some(commons_types::server::rank::ServerRank::Test)
		);
	})
	.await
}

/// An archived box serves nothing, so it is refused a rank, and the refusal
/// leaves the rest of the edit unapplied.
// spec: FLT#editing
#[tokio::test(flavor = "multi_thread")]
async fn an_archived_machine_cannot_be_ranked() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id, deleted_at) VALUES ('gone', '{BOX}', NOW())"
		))
		.await
		.unwrap();

		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "rank": "test", "name": "renamed" }))
			.await
			.assert_status_bad_request();

		let machine = Machine::get_by_id(&mut conn, BOX.parse().unwrap())
			.await
			.unwrap();
		assert_eq!(machine.name, "gone");
		assert_eq!(machine.rank, None);
	})
	.await
}

/// Naming the rank an archived box already carries changes nothing, so the
/// rest of the edit saves.
// spec: FLT#archival
#[tokio::test(flavor = "multi_thread")]
async fn an_edit_naming_an_archived_machines_own_rank_saves() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id, rank, deleted_at) \
			 VALUES ('gone', '{BOX}', 'test', NOW())"
		))
		.await
		.unwrap();

		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": BOX, "rank": "test", "name": "renamed" }))
			.await
			.assert_status_ok();

		let machine = Machine::get_by_id(&mut conn, BOX.parse().unwrap())
			.await
			.unwrap();
		assert_eq!(machine.name, "renamed");
	})
	.await
}
