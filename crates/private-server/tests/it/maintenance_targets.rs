//! A maintenance window is declared over a machine, so what reads as suspended
//! is the applications on that machine.
//!
//! Every machine that predates the split took its application's id, so a reader
//! comparing a window's machine against an application's own id agreed with a
//! correct one on all existing data. It parts company the moment a machine is
//! created with an id of its own, which is what these seed.
//!
//! spec: MNT

use commons_tests::diesel_async::SimpleAsyncConnection;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
struct GroupDetail {
	applications: Vec<ApplicationRow>,
}

#[derive(Debug, Deserialize)]
struct ApplicationRow {
	id: String,
	maintained: Option<bool>,
}

/// A machine and one application on it, with deliberately unequal ids.
async fn seed(conn: &mut impl SimpleAsyncConnection, group: Uuid, name: &str) -> (Uuid, Uuid) {
	let machine = Uuid::new_v4();
	let application = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, group_id) VALUES ('{machine}', '{group}'); \
		 INSERT INTO applications (id, name, host, type, group_id, machine_id) \
		 VALUES ('{application}', '{name}', 'https://{application}.example.com', \
		         'tamanu-central', '{group}', '{machine}')"
	))
	.await
	.expect("seed machine and application");
	(machine, application)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_over_a_machine_suspends_the_applications_on_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		let group = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'mnt-group')"
		))
		.await
		.unwrap();

		let (under, under_app) = seed(&mut conn, group, "under-maintenance").await;
		let (_, beside_app) = seed(&mut conn, group, "beside-it").await;

		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (machine_id, expected_end) \
			 VALUES ('{under}', NOW() + INTERVAL '1 hour')"
		))
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/groups/get")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		response.assert_status_ok();
		let detail: GroupDetail = response.json();

		let find = |id: Uuid| {
			detail
				.applications
				.iter()
				.find(|a| a.id == id.to_string())
				.unwrap_or_else(|| panic!("{id} missing from the group"))
		};

		assert_eq!(
			find(under_app).maintained,
			Some(true),
			"the workload on the box under maintenance is suspended"
		);
		assert_eq!(
			find(beside_app).maintained,
			Some(false),
			"a workload on another box is not"
		);
	})
	.await
}

/// A group-wide window suspends every application in the group, whichever box
/// each runs on.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_over_a_group_suspends_all_of_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		let group = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'whole-group')"
		))
		.await
		.unwrap();

		let (_, first) = seed(&mut conn, group, "first").await;
		let (_, second) = seed(&mut conn, group, "second").await;

		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (server_group_id, expected_end) \
			 VALUES ('{group}', NOW() + INTERVAL '1 hour')"
		))
		.await
		.unwrap();

		let response = private
			.post("/api/fleet/groups/get")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await;
		response.assert_status_ok();
		let detail: GroupDetail = response.json();

		for id in [first, second] {
			let row = detail
				.applications
				.iter()
				.find(|a| a.id == id.to_string())
				.unwrap_or_else(|| panic!("{id} missing from the group"));
			assert_eq!(row.maintained, Some(true));
		}
	})
	.await
}

#[derive(Debug, Deserialize)]
struct GroupSettling {
	maintained: bool,
	maintenance_settling: bool,
	machines: Vec<MachineRow>,
}

#[derive(Debug, Deserialize)]
struct MachineRow {
	id: String,
	maintenance_settling: bool,
}

/// Lifting a group's window does not end suspension: the settle period does.
/// Until it elapses the group and every box in it read as settling, marked apart
/// from being worked on, so lifting shows on the target rather than only on the
/// window.
///
/// spec: MNT#settling
#[tokio::test(flavor = "multi_thread")]
async fn a_lifted_group_window_reads_as_settling_on_the_group_and_its_boxes() {
	commons_tests::server::run(async |mut conn, _, private| {
		let group = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'settling-group')"
		))
		.await
		.unwrap();
		let (first, _) = seed(&mut conn, group, "box-one").await;
		let (second, _) = seed(&mut conn, group, "box-two").await;

		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (server_group_id, expected_end) \
			 VALUES ('{group}', NOW() + INTERVAL '1 hour')"
		))
		.await
		.unwrap();

		let holding: GroupSettling = private
			.post("/api/fleet/groups/get")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await
			.json();
		assert!(holding.maintained, "the group is suspended");
		assert!(
			!holding.maintenance_settling,
			"and being worked on, not settling"
		);
		for machine in &holding.machines {
			assert!(
				!machine.maintenance_settling,
				"{} is being worked on too",
				machine.id
			);
		}

		conn.batch_execute(&format!(
			"UPDATE maintenance_windows SET ended_at = NOW() - INTERVAL '2 minutes', \
			 expected_end = NOW() - INTERVAL '2 minutes' WHERE server_group_id = '{group}'"
		))
		.await
		.unwrap();

		let settling: GroupSettling = private
			.post("/api/fleet/groups/get")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await
			.json();
		assert!(
			settling.maintained,
			"still suspended, since the settle period has not run out"
		);
		assert!(
			settling.maintenance_settling,
			"and now reading as just handed back"
		);
		let ids: Vec<&str> = settling
			.machines
			.iter()
			.filter(|m| m.maintenance_settling)
			.map(|m| m.id.as_str())
			.collect();
		assert_eq!(
			ids.len(),
			2,
			"both boxes carry the settling mark, not only the group, got {ids:?}"
		);
		assert!(
			ids.contains(&first.to_string().as_str()) && ids.contains(&second.to_string().as_str()),
		);
	})
	.await
}

/// Once the settle period is out, nothing is marked: the group stops reading as
/// suspended rather than staying marked for good.
///
/// spec: MNT#settling
#[tokio::test(flavor = "multi_thread")]
async fn a_group_past_the_settle_period_is_not_marked_at_all() {
	commons_tests::server::run(async |mut conn, _, private| {
		let group = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'settled-group')"
		))
		.await
		.unwrap();
		seed(&mut conn, group, "box-one").await;
		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (server_group_id, expected_end, ended_at) \
			 VALUES ('{group}', NOW() - INTERVAL '1 hour', NOW() - INTERVAL '1 hour')"
		))
		.await
		.unwrap();

		let detail: GroupSettling = private
			.post("/api/fleet/groups/get")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await
			.json();
		assert!(!detail.maintained, "watching has resumed");
		assert!(!detail.maintenance_settling);
		for machine in &detail.machines {
			assert!(!machine.maintenance_settling);
		}
	})
	.await
}

#[derive(Debug, Deserialize)]
struct Card {
	members: Vec<CardMember>,
}

#[derive(Debug, Deserialize)]
struct CardMember {
	id: String,
	health: String,
	machine_maintained: bool,
}

/// A window holds a target's issues out of incidents and leaves their grading
/// alone, so what the card reports is the failure as it stands, marked. This is
/// the field the status grid colours its dots from: flattening it here would
/// paint a failing box healthy under the hatch, which is the behaviour the
/// window used to have.
///
/// spec: MNT#what-a-window-suspends
#[tokio::test(flavor = "multi_thread")]
async fn a_failing_box_under_a_window_still_reports_its_own_health() {
	commons_tests::server::run(async |mut conn, _, private| {
		let group = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'mid-cutover')"
		))
		.await
		.unwrap();
		let (box_id, failing) = seed(&mut conn, group, "failing-on-purpose").await;
		conn.batch_execute(&format!(
			"INSERT INTO statuses (server_id, healthy, extra, health, source) \
			 VALUES ('{failing}', false, '{{}}'::jsonb, \
			         '[{{\"check\": \"database\", \"healthy\": false}}]'::jsonb, 'test'); \
			 INSERT INTO check_policies (source, check_name, ceiling, escalates, subject, application_type) \
			 VALUES ('test', 'database', 'failed', false, 'application', 'tamanu-central'); \
			 INSERT INTO issues (application_id, source, ref, check_name, observed_result, \
			                     effective_result, message, active, first_seen, last_seen) \
			 VALUES ('{failing}', 'test', 'database', 'database', 'failed', 'failed', \
			         'connection refused', true, NOW(), NOW()); \
			 INSERT INTO maintenance_windows (machine_id, expected_end, note) \
			 VALUES ('{box_id}', NOW() + INTERVAL '1 hour', 'Cutting over the database')"
		))
		.await
		.unwrap();

		let card: Card = private
			.post("/api/statuses/group_details")
			.json(&serde_json::json!({ "server_group_id": group }))
			.await
			.json();
		let member = card
			.members
			.iter()
			.find(|m| m.id == failing.to_string())
			.expect("the failing member is on the card");
		assert_eq!(
			member.health, "unhealthy",
			"the check grades as it stands, so the card carries the failure"
		);
		assert!(
			member.machine_maintained,
			"and says the box is being worked on, beside it rather than instead of it"
		);
	})
	.await
}
