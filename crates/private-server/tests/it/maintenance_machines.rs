//! A window over several machines of one environment, and holding the window a
//! run declared, through the operator API: what a declaration offers and
//! accepts, how the window reads back, and what the audit log names.
//!
//! spec: MNT

use commons_tests::diesel_async::SimpleAsyncConnection;
use database::audit::{self, Filter};
use database::maintenance_windows::MaintenanceWindow;
use serde_json::{Value, json};

const GROUP: &str = "cccccccc-0000-0000-0000-000000000001";
const BOX_A: &str = "cccccccc-0000-0000-0000-0000000000b1";
const BOX_B: &str = "cccccccc-0000-0000-0000-0000000000b2";
const BOX_C: &str = "cccccccc-0000-0000-0000-0000000000b3";
const CLONE_BOX: &str = "cccccccc-0000-0000-0000-0000000000b4";

async fn seed(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka');
		 INSERT INTO machines (name, id, group_id) VALUES
			('kamaka-a', '{BOX_A}', '{GROUP}'),
			('kamaka-b', '{BOX_B}', '{GROUP}'),
			('kamaka-c', '{BOX_C}', '{GROUP}'),
			('kamaka-clone', '{CLONE_BOX}', '{GROUP}');
		 INSERT INTO applications (name, host, type, rank, group_id, machine_id) VALUES
			('a', 'https://a.kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{BOX_A}'),
			('b', 'https://b.kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{BOX_B}'),
			('c', 'https://c.kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{BOX_C}'),
			('clone', 'https://clone.kamaka.example', 'tamanu-central', 'clone', '{GROUP}', '{CLONE_BOX}');"
	))
	.await
	.unwrap();
}

fn in_hours(hours: i64) -> String {
	(jiff::Timestamp::now() + jiff::SignedDuration::from_hours(hours)).to_string()
}

/// A window over several machines is declared, listed and audited, refuses what
/// it cannot cover, is offered between the environment and its machines, and has
/// machines added and removed by amending it.
#[tokio::test(flavor = "multi_thread")]
async fn a_window_over_machines_is_declared_offered_and_amended() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		let window: Value = private
			.post("/api/maintenance/declare")
			.json(
				&json!({ "machine_ids": [BOX_B, BOX_A], "expected_end": in_hours(1), "note": "patching" }),
			)
			.await
			.json();
		let named: Vec<&str> = window["machine_ids"]
			.as_array()
			.unwrap()
			.iter()
			.map(|id| id.as_str().unwrap())
			.collect();
		assert_eq!(named, [BOX_A, BOX_B], "declaring: the machines it names");
		assert!(
			window["machine_id"].is_null(),
			"declaring: a set has no single machine"
		);
		assert!(
			window["lease_id"].is_null(),
			"declaring: a set has no lease"
		);

		let open: Vec<Value> = private
			.post("/api/maintenance/list_open")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(open.len(), 1, "listing: one open window");
		assert_eq!(open[0]["target"], "kamaka 2 machines");

		for machine in [BOX_A, BOX_B] {
			let history: Vec<Value> = private
				.post("/api/maintenance/for_target")
				.json(&json!({ "machine_id": machine }))
				.await
				.json();
			assert_eq!(history.len(), 1, "{machine}'s history names it");
			assert_eq!(history[0]["window"]["id"], window["id"]);
		}
		let history: Vec<Value> = private
			.post("/api/maintenance/for_target")
			.json(&json!({ "machine_id": BOX_C }))
			.await
			.json();
		assert!(history.is_empty(), "the history of a machine it omits");

		let entries = audit::find(
			&mut conn,
			&Filter {
				limit: 10,
				..Filter::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(entries.len(), 1, "the declaration's audit entries");
		for machine in ["kamaka-a", "kamaka-b"] {
			assert!(
				entries[0]
					.records
					.iter()
					.any(|record| record.name == machine),
				"the entry names {machine}: {:?}",
				entries[0].records
			);
		}
		assert!(
			!entries[0]
				.records
				.iter()
				.any(|record| record.name == "kamaka-c"),
			"the entry does not name kamaka-c"
		);

		private
			.post("/api/maintenance/declare")
			.json(&json!({ "machine_ids": [BOX_B, BOX_C], "expected_end": in_hours(1) }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
		private
			.post("/api/maintenance/declare")
			.json(&json!({ "machine_ids": [BOX_A, CLONE_BOX], "expected_end": in_hours(1) }))
			.await
			.assert_status_bad_request();
		private
			.post("/api/maintenance/declare")
			.json(
				&json!({ "machine_ids": [BOX_C, BOX_B], "machine_id": BOX_C, "expected_end": in_hours(1) }),
			)
			.await
			.assert_status_bad_request();

		let targets: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({ "start": { "server_group_id": GROUP, "rank": "production" } }))
			.await
			.json();
		let choices = targets["choices"].as_array().unwrap();
		let labels: Vec<&str> = choices
			.iter()
			.map(|choice| choice["label"].as_str().unwrap())
			.collect();
		assert_eq!(
			&labels[..4],
			["kamaka", "production", "Several machines", "kamaka-a"],
			"between the environment and its machines"
		);
		assert_eq!(choices[2]["depth"], 2);
		let machines = choices[2]["machines"].as_array().unwrap();
		assert_eq!(
			machines.len(),
			3,
			"the environment's machines, not the clone's"
		);
		let window_of = |name: &str| {
			machines
				.iter()
				.find(|machine| machine["name"] == name)
				.unwrap()["window"]["id"]
				.clone()
		};
		assert_eq!(
			window_of("kamaka-a"),
			window["id"],
			"a machine another window names says so"
		);
		assert_eq!(window_of("kamaka-b"), window["id"]);
		assert!(window_of("kamaka-c").is_null());
		assert!(
			choices[3]["window"]["id"] == window["id"],
			"and so does its own entry"
		);
		assert!(
			choices[2]["window"].is_null(),
			"the environment's start is not an amendment of the set"
		);

		let from_machine: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({ "start": { "machine_id": BOX_A } }))
			.await
			.json();
		assert_eq!(
			from_machine["amends"]["id"], window["id"],
			"a machine in a set starts as an amendment of the set"
		);
		assert!(
			from_machine["choices"]
				.as_array()
				.unwrap()
				.iter()
				.any(|choice| choice["label"] == "Several machines"),
			"and offers the several"
		);

		let from_set: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({ "start": { "machine_ids": [BOX_A, BOX_B] } }))
			.await
			.json();
		assert_eq!(
			from_set["amends"]["id"], window["id"],
			"a set starts as an amendment of itself"
		);
		let own = from_set["choices"]
			.as_array()
			.unwrap()
			.iter()
			.find(|choice| choice["label"] == "Several machines")
			.unwrap();
		assert_eq!(own["target"]["machine_ids"].as_array().unwrap().len(), 2);
		assert_eq!(own["window"]["id"], window["id"]);

		let from_clone: Value = private
			.post("/api/maintenance/targets")
			.json(&json!({ "start": { "machine_id": CLONE_BOX } }))
			.await
			.json();
		assert!(
			!from_clone["choices"]
				.as_array()
				.unwrap()
				.iter()
				.any(|choice| choice["label"] == "Several machines"),
			"an environment of one machine has no several to choose"
		);

		let grown: Value = private
			.post("/api/maintenance/amend")
			.json(&json!({ "id": window["id"], "target": { "machine_ids": [BOX_A, BOX_B, BOX_C] } }))
			.await
			.json();
		assert_eq!(
			grown["machine_ids"].as_array().unwrap().len(),
			3,
			"amending: the set grown"
		);

		let single: Value = private
			.post("/api/maintenance/amend")
			.json(&json!({ "id": window["id"], "target": { "machine_ids": [BOX_C] } }))
			.await
			.json();
		assert_eq!(
			single["machine_id"], BOX_C,
			"left with one it is that machine's window"
		);
		assert_eq!(single["machine_ids"].as_array().unwrap().len(), 0);

		let entries = audit::find(
			&mut conn,
			&Filter {
				limit: 10,
				..Filter::default()
			},
		)
		.await
		.unwrap();
		let amendments: Vec<_> = entries
			.iter()
			.filter(|entry| entry.action == "maintenance_amended")
			.collect();
		assert_eq!(amendments.len(), 2, "amending: two amendments audited");
		let removal = amendments[0];
		assert!(
			removal
				.records
				.iter()
				.any(|record| record.name == "kamaka-a"),
			"the machines it left are named: {:?}",
			removal.records
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_window_a_run_declared_is_held_by_its_operator() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let lease = |held_by: &str| {
			format!(
				"INSERT INTO ansible_leases (server_group_id, rank, intent, held_by, expires_at) \
				 VALUES ('{GROUP}', 'production', 'configure', '{held_by}', NOW() + INTERVAL '30 minutes'); "
			)
		};
		conn.batch_execute(&lease("someone-else")).await.unwrap();
		let theirs = database::ansible_leases::AnsibleLease::open_for(
			&mut conn,
			GROUP.parse().unwrap(),
			commons_types::server::rank::ServerRank::Production,
		)
		.await
		.unwrap()
		.unwrap();
		let theirs = MaintenanceWindow::declare_for_run(&mut conn, &theirs, None, None)
			.await
			.unwrap()
			.declared
			.remove(0);

		let refused = private
			.post("/api/maintenance/hold")
			.json(&json!({ "id": theirs.id, "expected_end": in_hours(2) }))
			.await;
		refused.assert_status(axum::http::StatusCode::CONFLICT);

		conn.batch_execute(
			"UPDATE ansible_leases SET released_at = NOW(); DELETE FROM maintenance_windows;",
		)
		.await
		.unwrap();
		conn.batch_execute(&lease("admin@localhost")).await.unwrap();
		let mine = database::ansible_leases::AnsibleLease::open_for(
			&mut conn,
			GROUP.parse().unwrap(),
			commons_types::server::rank::ServerRank::Production,
		)
		.await
		.unwrap()
		.unwrap();
		let window = MaintenanceWindow::declare_for_run(&mut conn, &mine, None, None)
			.await
			.unwrap()
			.declared
			.remove(0);

		let open: Vec<Value> = private
			.post("/api/maintenance/list_open")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(open[0]["window"]["lease_id"], mine.id.to_string());

		let held: Value = private
			.post("/api/maintenance/hold")
			.json(&json!({ "id": window.id, "expected_end": in_hours(2) }))
			.await
			.json();
		assert!(held["lease_id"].is_null(), "it no longer lasts the run");

		let entries = audit::find(
			&mut conn,
			&Filter {
				limit: 10,
				..Filter::default()
			},
		)
		.await
		.unwrap();
		assert!(
			entries
				.iter()
				.any(|entry| entry.action == "maintenance_amended"),
			"holding is an amendment"
		);
	})
	.await
}
