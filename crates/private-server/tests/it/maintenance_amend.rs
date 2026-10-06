//! Retargeting a declaration and moving a window through the operator API: the
//! choices a declaration offers, the coverage marks an incident puts on them,
//! and amending a window's end, note, or target.
//!
//! spec: MNT

use commons_tests::diesel_async::SimpleAsyncConnection;
use commons_types::status::CheckResult;
use database::issues::{CheckFiling, Scope, file_check};
use database::statuses::CANOPY_SOURCE;
use serde_json::{Value, json};
use uuid::Uuid;

const GROUP: &str = "eeeeeeee-0000-0000-0000-000000000001";
const OTHER_GROUP: &str = "eeeeeeee-0000-0000-0000-000000000002";
const PRODUCTION_BOX: &str = "eeeeeeee-0000-0000-0000-0000000000b1";
const CLONE_BOX: &str = "eeeeeeee-0000-0000-0000-0000000000b2";
const PRODUCTION: &str = "eeeeeeee-0000-0000-0000-0000000000a1";
const CLONE: &str = "eeeeeeee-0000-0000-0000-0000000000a2";

async fn seed(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka'), ('{OTHER_GROUP}', 'tonga');
		 INSERT INTO machines (name, id, group_id) VALUES
			('kamaka-central', '{PRODUCTION_BOX}', '{GROUP}'),
			('kamaka-clone', '{CLONE_BOX}', '{GROUP}');
		 INSERT INTO applications (id, name, host, type, rank, group_id, machine_id) VALUES
			('{PRODUCTION}', 'central', 'https://kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{PRODUCTION_BOX}'),
			('{CLONE}', 'clone central', 'https://clone.kamaka.example', 'tamanu-central', 'clone', '{GROUP}', '{CLONE_BOX}');"
	))
	.await
	.unwrap();
}

fn in_hours(hours: i64) -> String {
	(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(hours)).to_string()
}

fn group_check() -> CheckFiling<'static> {
	CheckFiling {
		source: CANOPY_SOURCE,
		scope: Scope::Group(GROUP.parse().unwrap()),
		device_id: None,
		check: "backup-staleness",
		observed: CheckResult::Failed,
		title: Some("Backups are late"),
		message: "the group's backups are late",
		detail: None,
		default_ceiling: CheckResult::Failed,
		default_escalates: false,
		documentation: None,
	}
}

fn kinds(choices: &[Value]) -> Vec<(String, u64)> {
	choices
		.iter()
		.map(|choice| {
			(
				choice["grain"]["kind"].as_str().unwrap().to_string(),
				choice["depth"].as_u64().unwrap(),
			)
		})
		.collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn a_groups_choices_nest_everything_in_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let targets: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({ "start": { "kind": "group", "group_id": GROUP } }))
			.await
			.json();
		let choices = targets["choices"].as_array().unwrap();
		assert_eq!(
			kinds(choices),
			vec![
				("group".into(), 0),
				("environment".into(), 1),
				("machine".into(), 2),
				("application".into(), 3),
				("environment".into(), 1),
				("machine".into(), 2),
				("application".into(), 3),
			]
		);
		assert_eq!(choices[1]["label"], "production");
		assert_eq!(choices[2]["label"], "kamaka-central");
		assert!(
			choices
				.iter()
				.all(|choice| choice["covers_failures"].is_null())
		);
		assert!(targets["fixed_because"].is_null());
	})
	.await
}

/// The case the card was raised for: a group's backup failure joins its
/// headline environment's incident, and a window over that environment leaves
/// it watched. The dialog says so before anything is declared.
#[tokio::test(flavor = "multi_thread")]
async fn an_incidents_choices_say_which_cover_its_failures() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		file_check(&mut conn, group_check()).await.unwrap();
		let incidents: Vec<Value> = private
			.post("/api/incidents/list_active")
			.json(&json!({}))
			.await
			.json();
		let incident = incidents
			.iter()
			.find(|incident| incident["server_group_id"] == GROUP)
			.expect("the group's check opens the headline environment's incident");

		let targets: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({
				"start": { "kind": "environment", "group_id": GROUP, "rank": "production" },
				"incident_id": incident["id"],
			}))
			.await
			.json();
		let marks: Vec<(String, bool)> = targets["choices"]
			.as_array()
			.unwrap()
			.iter()
			.map(|choice| {
				(
					choice["grain"]["kind"].as_str().unwrap().to_string(),
					choice["covers_failures"].as_bool().unwrap(),
				)
			})
			.collect();
		assert_eq!(
			marks,
			vec![
				("group".into(), true),
				("environment".into(), false),
				("machine".into(), false),
				("application".into(), false),
			],
			"only the group quiets the group's own check"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn amending_changes_only_what_it_names_and_can_move_the_window() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let window: Value = private
			.post("/api/maintenance/declare")
			.json(&json!({
				"server_group_id": GROUP,
				"expected_end": in_hours(1),
				"note": "patching",
			}))
			.await
			.json();
		let id = window["id"].as_str().unwrap();

		let moved: Value = private
			.post("/api/maintenance/amend")
			.json(&json!({
				"id": id,
				"target": { "kind": "environment", "group_id": GROUP, "rank": "clone" },
			}))
			.await
			.json();
		assert_eq!(moved["id"], window["id"], "it stays the same window");
		assert_eq!(moved["rank"], "clone");
		assert_eq!(moved["note"], "patching", "the note was not named");
		assert_eq!(moved["expected_end"], window["expected_end"]);

		let cleared: Value = private
			.post("/api/maintenance/amend")
			.json(&json!({ "id": id, "note": null }))
			.await
			.json();
		assert!(cleared["note"].is_null(), "a null note clears it");

		let history: Vec<Value> = private
			.post("/api/maintenance/for_target")
			.json(&json!({ "server_group_id": GROUP }))
			.await
			.json();
		let left = history
			.iter()
			.find(|row| !row["moved_at"].is_null())
			.expect("the group keeps the span it covered");
		assert!(left["rank"].is_null(), "that span was the group's own");
		assert_eq!(left["moved_to"], "kamaka clone");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_move_is_refused_where_it_cannot_go() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let window: Value = private
			.post("/api/maintenance/declare")
			.json(&json!({ "machine_id": PRODUCTION_BOX, "expected_end": in_hours(1) }))
			.await
			.json();
		let id = window["id"].as_str().unwrap();

		private
			.post("/api/maintenance/amend")
			.json(&json!({ "id": id, "target": { "kind": "group", "group_id": OTHER_GROUP } }))
			.await
			.assert_status_bad_request();

		private
			.post("/api/maintenance/declare")
			.json(&json!({ "server_group_id": GROUP, "expected_end": in_hours(1) }))
			.await
			.assert_status_ok();
		private
			.post("/api/maintenance/amend")
			.json(&json!({ "id": id, "target": { "kind": "group", "group_id": GROUP } }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);

		private
			.post("/api/maintenance/amend")
			.json(&json!({ "id": Uuid::new_v4(), "note": "x" }))
			.await
			.assert_status_not_found();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_leased_windows_choices_say_it_cannot_move() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let window: Value = private
			.post("/api/maintenance/declare")
			.json(&json!({
				"server_group_id": GROUP,
				"rank": "production",
				"expected_end": in_hours(1),
			}))
			.await
			.json();
		conn.batch_execute(&format!(
			"INSERT INTO inventory_leases (server_group_id, rank, intent, held_by, expires_at) \
			 VALUES ('{GROUP}', 'production', 'configure', 'admin@localhost', NOW() + INTERVAL '1 hour')"
		))
		.await
		.unwrap();

		let targets: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({
				"start": { "kind": "environment", "group_id": GROUP, "rank": "production" },
				"window_id": window["id"],
			}))
			.await
			.json();
		assert!(
			targets["fixed_because"].is_string(),
			"the run is acting on the environment the window covers: {targets}"
		);
		private
			.post("/api/maintenance/amend")
			.json(&json!({
				"id": window["id"],
				"target": { "kind": "machine", "machine_id": PRODUCTION_BOX },
			}))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}
