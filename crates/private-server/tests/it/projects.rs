//! The projects listing: which groups it lists and the production Tamanu it
//! shows for each.
//!
//! Spec: `.workhorse/specs/servers/projects.md` (id `PRJ`).

use commons_tests::diesel_async::SimpleAsyncConnection;
use database::diesel_async::AsyncPgConnection;
use serde_json::{Value, json};

const ALPHA: &str = "a0000000-0000-0000-0000-000000000000";
const BETA: &str = "b0000000-0000-0000-0000-000000000000";
const GAMMA: &str = "c0000000-0000-0000-0000-000000000000";
const DELTA: &str = "d0000000-0000-0000-0000-000000000000";
const EPSILON: &str = "e0000000-0000-0000-0000-000000000000";

/// The central shares its machine's id, as a migration left many.
const CENTRAL: &str = "a1000000-0000-0000-0000-000000000000";
const CENTRAL_BOX_DEVICE: &str = "a1d00000-0000-0000-0000-000000000000";
const CLINIC_ONE: &str = "a2000000-0000-0000-0000-000000000000";
const CLINIC_ONE_BOX: &str = "a2b00000-0000-0000-0000-000000000000";
const CLINIC_ONE_DEVICE: &str = "a2d00000-0000-0000-0000-000000000000";
const CLINIC_TWO: &str = "a3000000-0000-0000-0000-000000000000";
const CLINIC_TWO_BOX: &str = "a3b00000-0000-0000-0000-000000000000";
const UNNAMED: &str = "a4000000-0000-0000-0000-000000000000";
const UNNAMED_BOX: &str = "a4b00000-0000-0000-0000-000000000000";
const ZETA: &str = "a5000000-0000-0000-0000-000000000000";
const ZETA_BOX: &str = "a5b00000-0000-0000-0000-000000000000";
const CLOUD: &str = "a6000000-0000-0000-0000-000000000000";
const CLUSTER: &str = "a6c00000-0000-0000-0000-000000000000";
const RELAY: &str = "a6d00000-0000-0000-0000-000000000000";

async fn seed(conn: &mut AsyncPgConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name, project, pm_in_charge, ground_contact, deleted_at) VALUES \
			('{ALPHA}', 'Alpha', 'Alpha rollout', 'pat@example.com', 'Sam', NULL), \
			('{BETA}', 'Beta', NULL, NULL, NULL, NULL), \
			('{GAMMA}', 'Gamma', 'Not live yet', NULL, NULL, NULL), \
			('{DELTA}', 'Delta', NULL, NULL, NULL, NOW()), \
			('{EPSILON}', 'Epsilon', NULL, NULL, NULL, NULL); \
		 INSERT INTO tailscale_users (login, name) VALUES ('Pat@Example.com', 'Pat Smith'); \
		 INSERT INTO devices (id, role, tailscale_node_name) VALUES \
			('{CENTRAL_BOX_DEVICE}', 'server', 'alpha-central'), \
			('{CLINIC_ONE_DEVICE}', 'server', 'alpha-clinic-one'), \
			('{RELAY}', 'relay', NULL); \
		 INSERT INTO kubernetes_clusters (id, name, relay_identity_id) VALUES ('{CLUSTER}', 'shared', '{RELAY}'); \
		 INSERT INTO machines (id, name, group_id, rank, device_id, deleted_at) VALUES \
			('{CENTRAL}', 'alpha-central-box', '{ALPHA}', 'production', '{CENTRAL_BOX_DEVICE}', NULL), \
			('{CLINIC_ONE_BOX}', 'clinic-north', '{ALPHA}', 'production', '{CLINIC_ONE_DEVICE}', NULL), \
			('{CLINIC_TWO_BOX}', 'clinic-south', '{ALPHA}', 'production', NULL, NULL), \
			('{UNNAMED_BOX}', 'unnamed-box', '{ALPHA}', 'production', NULL, NULL), \
			('{ZETA_BOX}', 'zeta-box', '{ALPHA}', 'production', NULL, NULL), \
			('a7b00000-0000-0000-0000-000000000000', 'alpha-test-box', '{ALPHA}', 'test', NULL, NULL), \
			('a8b00000-0000-0000-0000-000000000000', 'alpha-pending-box', '{ALPHA}', NULL, NULL, NULL), \
			('a9b00000-0000-0000-0000-000000000000', 'alpha-archived-box', '{ALPHA}', 'production', NULL, NOW()), \
			('b1b00000-0000-0000-0000-000000000000', 'beta-lab', '{BETA}', 'production', NULL, NULL), \
			('c1b00000-0000-0000-0000-000000000000', 'gamma-test', '{GAMMA}', 'test', NULL, NULL), \
			('c2b00000-0000-0000-0000-000000000000', 'gamma-demo', '{GAMMA}', 'demo', NULL, NULL), \
			('d1b00000-0000-0000-0000-000000000000', 'delta-central', '{DELTA}', 'production', NULL, NULL), \
			('e1b00000-0000-0000-0000-000000000000', 'epsilon-box', '{EPSILON}', 'production', NULL, NULL); \
		 INSERT INTO applications (id, name, type, rank, group_id, machine_id, kubernetes_cluster_id, host, deleted_at) VALUES \
			('{CENTRAL}', 'Alpha central', 'tamanu-central', 'production', '{ALPHA}', '{CENTRAL}', NULL, 'https://alpha.example.com', NULL), \
			('{CLINIC_ONE}', 'Clinic', 'tamanu-facility', 'production', '{ALPHA}', '{CLINIC_ONE_BOX}', NULL, 'https://clinic.example.com', NULL), \
			('{CLINIC_TWO}', 'Clinic', 'tamanu-facility', 'production', '{ALPHA}', '{CLINIC_TWO_BOX}', NULL, NULL, NULL), \
			('{UNNAMED}', NULL, 'tamanu-facility', 'production', '{ALPHA}', '{UNNAMED_BOX}', NULL, NULL, NULL), \
			('{ZETA}', 'Zeta clinic', 'tamanu-facility', 'production', '{ALPHA}', '{ZETA_BOX}', NULL, NULL, NULL), \
			('{CLOUD}', 'Cloud clinic', 'tamanu-facility', 'production', '{ALPHA}', NULL, '{CLUSTER}', 'https://cloud.example.com', NULL), \
			('a7000000-0000-0000-0000-000000000000', 'Test central', 'tamanu-central', 'test', '{ALPHA}', 'a7b00000-0000-0000-0000-000000000000', NULL, NULL, NULL), \
			('a8000000-0000-0000-0000-000000000000', 'Pending clinic', 'tamanu-facility', NULL, '{ALPHA}', 'a8b00000-0000-0000-0000-000000000000', NULL, NULL, NULL), \
			('a9000000-0000-0000-0000-000000000000', 'Archived clinic', 'tamanu-facility', 'production', '{ALPHA}', 'a9b00000-0000-0000-0000-000000000000', NULL, NULL, NOW()), \
			('aa000000-0000-0000-0000-000000000000', 'Retired clinic', 'tamanu-facility', 'production', '{ALPHA}', '{ZETA_BOX}', NULL, NULL, NOW()), \
			('b1000000-0000-0000-0000-000000000000', 'Beta lab', 'senaite', 'production', '{BETA}', 'b1b00000-0000-0000-0000-000000000000', NULL, NULL, NULL), \
			('c1000000-0000-0000-0000-000000000000', 'Gamma central', 'tamanu-central', 'test', '{GAMMA}', 'c1b00000-0000-0000-0000-000000000000', NULL, NULL, NULL), \
			('c2000000-0000-0000-0000-000000000000', 'Gamma demo', 'tamanu-central', 'demo', '{GAMMA}', 'c2b00000-0000-0000-0000-000000000000', NULL, NULL, NULL), \
			('d1000000-0000-0000-0000-000000000000', 'Delta central', 'tamanu-central', 'production', '{DELTA}', 'd1b00000-0000-0000-0000-000000000000', NULL, NULL, NULL);"
	))
	.await
	.expect("seed");
}

async fn projects(private: &commons_tests::axum_test::TestServer) -> Vec<Value> {
	let response = private
		.post("/api/fleet/groups/projects")
		.json(&json!({}))
		.await;
	response.assert_status_ok();
	response.json()
}

// spec: PRJ#which-groups-it-lists
// spec: PRJ#what-each-group-shows
#[tokio::test(flavor = "multi_thread")]
async fn lists_live_groups_with_production_and_their_production_tamanu() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let listed = projects(&private).await;

		let names: Vec<&str> = listed.iter().map(|g| g["name"].as_str().unwrap()).collect();
		assert_eq!(
			names,
			["Alpha", "Beta", "Epsilon"],
			"live groups with a production environment, by name: not the test and demo \
			 group, not the archived one"
		);

		let alpha = &listed[0];
		assert_eq!(alpha["id"], ALPHA);
		assert_eq!(
			alpha["project_details"],
			json!({
				"project": "Alpha rollout",
				"pm_in_charge": "pat@example.com",
				"pm_in_charge_name": "Pat Smith",
				"ground_contact": "Sam",
			}),
		);
		assert_eq!(
			alpha["tamanu"],
			json!([
				{
					"application_id": CENTRAL,
					"type": "tamanu-central",
					"name": "Alpha central",
					"machine_id": CENTRAL,
					"machine_name": "alpha-central-box",
					"url": "https://alpha.example.com/",
					"tailnet_name": "alpha-central",
				},
				{
					"application_id": CLINIC_ONE,
					"type": "tamanu-facility",
					"name": "Clinic",
					"machine_id": CLINIC_ONE_BOX,
					"machine_name": "clinic-north",
					"url": "https://clinic.example.com/",
					"tailnet_name": "alpha-clinic-one",
				},
				{
					"application_id": CLINIC_TWO,
					"type": "tamanu-facility",
					"name": "Clinic",
					"machine_id": CLINIC_TWO_BOX,
					"machine_name": "clinic-south",
					"url": null,
					"tailnet_name": null,
				},
				{
					"application_id": CLOUD,
					"type": "tamanu-facility",
					"name": "Cloud clinic",
					"machine_id": null,
					"machine_name": null,
					"url": "https://cloud.example.com/",
					"tailnet_name": null,
				},
				{
					"application_id": UNNAMED,
					"type": "tamanu-facility",
					"name": null,
					"machine_id": UNNAMED_BOX,
					"machine_name": "unnamed-box",
					"url": null,
					"tailnet_name": null,
				},
				{
					"application_id": ZETA,
					"type": "tamanu-facility",
					"name": "Zeta clinic",
					"machine_id": ZETA_BOX,
					"machine_name": "zeta-box",
					"url": null,
					"tailnet_name": null,
				},
			]),
			"the central, then facilities by name (an unnamed one by its type), told apart \
			 by machine; no test, pending or archived Tamanu"
		);

		for group in &listed[1..] {
			assert_eq!(
				group["tamanu"],
				json!([]),
				"{}: a production environment with no Tamanu has none",
				group["name"]
			);
			assert_eq!(
				group["project_details"],
				json!({
					"project": null,
					"pm_in_charge": null,
					"pm_in_charge_name": null,
					"ground_contact": null,
				}),
			);
		}
	})
	.await;
}

// spec: PRJ#which-groups-it-lists
#[tokio::test(flavor = "multi_thread")]
async fn marks_the_groups_the_caller_is_in_charge_of_ignoring_case() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		// The debug identity is admin@localhost.
		conn.batch_execute(&format!(
			"UPDATE server_groups SET pm_in_charge = 'Admin@LocalHost' WHERE id = '{BETA}'"
		))
		.await
		.expect("seed");
		let listed = projects(&private).await;

		let yours: Vec<(&str, bool)> = listed
			.iter()
			.map(|g| (g["name"].as_str().unwrap(), g["yours"].as_bool().unwrap()))
			.collect();
		assert_eq!(
			yours,
			[("Alpha", false), ("Beta", true), ("Epsilon", false)],
			"only the group naming the caller as PM in charge, whatever its case"
		);
	})
	.await;
}
