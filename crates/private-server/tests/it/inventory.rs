//! Endpoint tests for `/api/inventory/for_group` — what a configuration run
//! reads for one environment, and the refusals it has to tell apart from
//! Canopy being unreachable.
//!
//! spec: INV

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use serde_json::{Value, json};
use uuid::Uuid;

pub(crate) async fn insert_group(conn: &mut AsyncPgConnection, name: &str, tags: Value) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name, tags) VALUES ('{id}', '{name}', '{tags}')"
	))
	.await
	.expect("insert group");
	id
}

pub(crate) async fn insert_application(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	name: &str,
	r#type: &str,
	host: Option<&str>,
	tags: Value,
) -> Uuid {
	insert_ranked_application(conn, group, name, r#type, None, host, tags).await
}

/// An application on a machine of its own, in a group. Seeded directly: a type
/// is reported rather than entered, so no operator flow creates one.
pub(crate) async fn insert_ranked_application(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	name: &str,
	r#type: &str,
	rank: Option<&str>,
	host: Option<&str>,
	tags: Value,
) -> Uuid {
	let id = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let ty = r#type;
	let host = host.map_or("NULL".to_string(), |h| format!("'{h}'"));
	let rank = rank.map_or("NULL".to_string(), |r| format!("'{r}'"));
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, name, group_id) VALUES ('{machine}', '{name}', '{group}');
		 INSERT INTO applications (id, name, type, rank, host, group_id, machine_id, tags)
		 VALUES ('{id}', '{name}', '{ty}', {rank}, {host}, '{group}', '{machine}', '{tags}')"
	))
	.await
	.expect("insert application");
	id
}

/// The identity speaks for the box, so the device binds to the machine the
/// application runs on.
pub(crate) async fn bind_device(
	conn: &mut AsyncPgConnection,
	application: Uuid,
	tailscale_name: &str,
) {
	let device = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role, tailscale_node_name)
		 VALUES ('{device}', 'server', '{tailscale_name}');
		 UPDATE machines SET device_id = '{device}'
		 WHERE id = (SELECT machine_id FROM applications WHERE id = '{application}')"
	))
	.await
	.expect("bind device");
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_an_environments_applications_and_variables() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(
			&mut conn,
			"kamaka-prod",
			json!({
				"timezone": "Pacific/Auckland",
				"tamanu_version": "v2.54.8",
				"elastic_agent_enabled": "false",
			}),
		)
		.await;
		let central = insert_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			None,
			json!({
				"public_hostname": "central.kamaka.example",
				"elastic_agent_enabled": "true",
				"tamanu_caddy_extra_hostnames": "[\"sync.kamaka.example\"]",
				"ansible_port": "2222",
			}),
		)
		.await;
		bind_device(&mut conn, central, "kamaka-prod-central").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-prod-facility",
			"tamanu-facility",
			Some("https://facility.kamaka.example/"),
			json!({ "canopy:type": "spoofed" }),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-prod" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();

		assert_eq!(body["group"], "kamaka-prod");
		// Nothing here carries a rank, so the whole group is one environment at
		// the default rank.
		assert_eq!(body["rank"], "dev");
		assert_eq!(body["vars"]["timezone"], "Pacific/Auckland");
		assert_eq!(body["vars"]["elastic_agent_enabled"], json!(false));

		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 2);

		// A bound device's tailnet name is the address, and the group's values
		// carry down except where the application sets its own.
		assert_eq!(hosts[0]["name"], "kamaka-prod-central");
		assert_eq!(hosts[0]["type"], "tamanu-central");
		assert_eq!(hosts[0]["address"], "kamaka-prod-central");
		assert_eq!(hosts[0]["vars"]["timezone"], "Pacific/Auckland");
		assert_eq!(hosts[0]["vars"]["elastic_agent_enabled"], json!(true));
		assert_eq!(
			hosts[0]["vars"]["tamanu_caddy_extra_hostnames"],
			json!(["sync.kamaka.example"])
		);
		assert_eq!(hosts[0]["vars"]["ansible_port"], "2222");

		// What the application sets itself is served apart from what it
		// inherits, so a value can be traced to where it is set.
		assert_eq!(hosts[0]["own_vars"]["elastic_agent_enabled"], json!(true));
		assert!(hosts[0]["own_vars"].get("timezone").is_none());

		// No device on the box, so the address is the recorded host as a bare
		// name, and a stored tag in the reserved namespace is not served as a
		// variable.
		assert_eq!(hosts[1]["address"], "facility.kamaka.example");
		assert!(hosts[1]["vars"].get("canopy:type").is_none());
		assert_eq!(hosts[1]["vars"]["timezone"], "Pacific/Auckland");
	})
	.await
}

/// Rank is an application's, so two workloads on one box sit in two
/// environments and are configured apart.
#[tokio::test(flavor = "multi_thread")]
async fn splits_a_shared_box_by_the_rank_of_each_application() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-shared", json!({})).await;
		let machine = Uuid::new_v4();
		let device = Uuid::new_v4();
		let production = Uuid::new_v4();
		let demo = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role, tailscale_node_name)
			 VALUES ('{device}', 'server', 'kamaka-shared-box');
			 INSERT INTO machines (id, name, group_id, device_id)
			 VALUES ('{machine}', 'kamaka-shared-box', '{group}', '{device}');
			 INSERT INTO applications (id, name, type, rank, group_id, machine_id)
			 VALUES ('{production}', 'kamaka-central', 'tamanu-central', 'production', '{group}', '{machine}'),
			        ('{demo}', 'kamaka-demo-central', 'tamanu-central', 'demo', '{group}', '{machine}')"
		))
		.await
		.expect("seed a shared box");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "server_group_id": group, "rank": "production" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-central");
		// Both workloads are reached at the box's address.
		assert_eq!(hosts[0]["address"], "kamaka-shared-box");
		assert_eq!(hosts[0]["machine_id"], machine.to_string());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_environment_at_the_rank_asked_for() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo-central",
			"tamanu-central",
			Some("demo"),
			None,
			json!({}),
		)
		.await;

		// A group spanning two environments cannot be served as one: a run
		// configuring the demo applications alongside the production ones is
		// never what was meant.
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka" }))
			.await;
		response.assert_status_conflict();

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka", "rank": "production" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		assert_eq!(body["rank"], "production");
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-prod-central");

		// A rank the group holds no live application at is refused, rather than
		// answered with an empty inventory.
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka", "rank": "test" }))
			.await;
		response.assert_status_conflict();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_an_environment_asked_for_by_identifier() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "drifting-demo", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"drifting-demo-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		assert_eq!(body["group"], "drifting-demo");
		assert_eq!(body["hosts"].as_array().expect("hosts").len(), 1);
	})
	.await
}

/// The address falls back to the recorded host when the device bound to the box
/// has no tailnet name of its own.
#[tokio::test(flavor = "multi_thread")]
async fn falls_back_to_the_recorded_host_for_a_nameless_device() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-nameless", json!({})).await;
		let application = insert_application(
			&mut conn,
			group,
			"kamaka-nameless-central",
			"tamanu-central",
			Some("https://central.kamaka.example/"),
			json!({}),
		)
		.await;
		let device = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{device}', 'server');
			 UPDATE machines SET device_id = '{device}'
			 WHERE id = (SELECT machine_id FROM applications WHERE id = '{application}')"
		))
		.await
		.expect("bind device");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-nameless" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		assert_eq!(body["hosts"][0]["address"], "central.kamaka.example");
	})
	.await
}

/// An archived application is not something to configure, so it leaves the
/// inventory without the group leaving it.
#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_an_archived_application() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-archived", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-archived-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		let gone = insert_application(
			&mut conn,
			group,
			"kamaka-archived-facility",
			"tamanu-facility",
			None,
			json!({}),
		)
		.await;
		conn.batch_execute(&format!(
			"UPDATE applications SET deleted_at = now() WHERE id = '{gone}'"
		))
		.await
		.expect("archive application");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-archived" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-archived-central");
	})
	.await
}

/// An application in no group is in no inventory: it has no group whose tags
/// would configure it and no environment to belong to.
#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_an_application_belonging_to_no_group() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-lonely", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-lonely-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		let loose = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id) VALUES ('{loose}');
			 INSERT INTO applications (id, name, type, machine_id)
			 VALUES ('{loose}', 'kamaka-loose', 'tamanu-central', '{loose}')"
		))
		.await
		.expect("insert ungrouped application");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-lonely" }))
			.await;
		response.assert_status_ok();
		let hosts = response.json::<Value>()["hosts"]
			.as_array()
			.expect("hosts")
			.clone();
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-lonely-central");
	})
	.await
}

/// Canopy's own placeholder application is not something to configure, and its
/// recorded host is a loopback address a run would take literally.
#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_the_meta_application() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-meta", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-meta-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		conn.batch_execute(&format!(
			"UPDATE applications SET group_id = '{group}' WHERE id = '{}'",
			Uuid::nil()
		))
		.await
		.expect("group the meta application");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-meta" }))
			.await;
		response.assert_status_ok();
		let hosts = response.json::<Value>()["hosts"]
			.as_array()
			.expect("hosts")
			.clone();
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-meta-central");
	})
	.await
}

/// A tag that opens like JSON and isn't stays the text it was stored as,
/// rather than becoming an error or a half-parsed value.
#[tokio::test(flavor = "multi_thread")]
async fn serves_a_malformed_json_tag_as_text() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-malformed", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-malformed-central",
			"tamanu-central",
			None,
			json!({ "tamanu_caddy_extra_hostnames": "[\"sync.kamaka.example\"" }),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-malformed" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		assert_eq!(
			body["hosts"][0]["vars"]["tamanu_caddy_extra_hostnames"],
			"[\"sync.kamaka.example\""
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_group_canopy_does_not_have() {
	commons_tests::server::run(async move |_conn, _public, private| {
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "nowhere-prod" }))
			.await;
		response.assert_status_not_found();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_archived_group() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-clone", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-clone-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		conn.batch_execute(&format!(
			"UPDATE server_groups SET deleted_at = now() WHERE id = '{group}'"
		))
		.await
		.expect("archive group");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-clone" }))
			.await;
		response.assert_status_conflict();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_group_with_nothing_to_configure() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		insert_group(&mut conn, "kamaka-demo", json!({})).await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-demo" }))
			.await;
		response.assert_status_conflict();
	})
	.await
}

/// A box added and not yet reporting carries no application, so there is
/// nothing at that rank to configure and the request is refused rather than
/// answered with an empty inventory.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_group_holding_only_a_machine_awaiting_check_in() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-bare", json!({})).await;
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id)
			 VALUES ('{machine}', 'kamaka-bare-box', '{group}')"
		))
		.await
		.expect("insert machine");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status_conflict();
	})
	.await
}

/// Names aren't unique, and serving one of two groups that answer to the same
/// name would configure the wrong fleet.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_name_that_answers_for_two_groups() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		for _ in 0..2 {
			let group = insert_group(&mut conn, "twice-prod", json!({})).await;
			insert_application(
				&mut conn,
				group,
				"twice-prod-central",
				"tamanu-central",
				None,
				json!({}),
			)
			.await;
		}

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "twice-prod" }))
			.await;
		response.assert_status_conflict();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_request_naming_neither_a_group_nor_an_identifier() {
	commons_tests::server::run(async move |_conn, _public, private| {
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({}))
			.await;
		response.assert_status_bad_request();
	})
	.await
}

/// Both together are as ambiguous as neither: canopy picks nothing.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_request_naming_both_a_group_and_an_identifier() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-both", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-both-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-both", "server_group_id": group }))
			.await;
		response.assert_status_bad_request();
	})
	.await
}

async fn declare_group_window(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	declared_by: &str,
	expected_end: &str,
) {
	conn.batch_execute(&format!(
		"INSERT INTO maintenance_windows (server_group_id, expected_end, declared_by)
		 VALUES ('{group}', {expected_end}, '{declared_by}')"
	))
	.await
	.expect("declare window");
}

async fn declare_machine_window(
	conn: &mut AsyncPgConnection,
	application: Uuid,
	declared_by: &str,
	expected_end: &str,
) {
	conn.batch_execute(&format!(
		"INSERT INTO maintenance_windows (machine_id, expected_end, declared_by)
		 SELECT machine_id, {expected_end}, '{declared_by}'
		 FROM applications WHERE id = '{application}'"
	))
	.await
	.expect("declare window");
}

/// A window someone else declared over the group is their work under way: the
/// run is refused and told who holds it.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_under_a_window_someone_else_declared() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-busy", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-busy-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		declare_group_window(
			&mut conn,
			group,
			"someone@else",
			"now() + interval '1 hour'",
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-busy" }))
			.await;
		response.assert_status_conflict();
		let detail = response.text();
		assert!(detail.contains("someone@else"), "{detail}");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_under_the_readers_own_window() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-mine", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-mine-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		declare_group_window(
			&mut conn,
			group,
			"admin@localhost",
			"now() + interval '1 hour'",
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-mine" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

/// A run acts on the environment as a whole, so a window over one member
/// refuses all of it.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_under_a_window_over_a_member() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-member", json!({})).await;
		let central = insert_application(
			&mut conn,
			group,
			"kamaka-member-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		insert_application(
			&mut conn,
			group,
			"kamaka-member-facility",
			"tamanu-facility",
			None,
			json!({}),
		)
		.await;
		declare_machine_window(
			&mut conn,
			central,
			"someone@else",
			"now() + interval '1 hour'",
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-member" }))
			.await;
		response.assert_status_conflict();
		assert!(response.text().contains("kamaka-member-central"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn ignores_a_window_over_a_machine_at_another_rank() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-ranks", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		let demo = insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo-central",
			"tamanu-central",
			Some("demo"),
			None,
			json!({}),
		)
		.await;
		declare_machine_window(&mut conn, demo, "someone@else", "now() + interval '1 hour'").await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-ranks", "rank": "production" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

/// A window past its expected end has stopped holding whether or not the sweep
/// has stamped it yet.
#[tokio::test(flavor = "multi_thread")]
async fn serves_once_a_window_has_passed_its_end() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-done", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-done-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		declare_group_window(
			&mut conn,
			group,
			"someone@else",
			"now() - interval '1 minute'",
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-done" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

/// A secret variable someone else set moments ago is their change still under
/// way, and the refusal names it before any value is read.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_after_someone_else_set_a_secret_variable() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-fresh", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-fresh-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		conn.batch_execute(&format!(
			"INSERT INTO inventory_secret_variables (server_group_id, rank, name, set_by)
			 VALUES ('{group}', 'dev', 'salt', 'someone@else')"
		))
		.await
		.expect("declare secret");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-fresh" }))
			.await;
		response.assert_status_conflict();
		let detail = response.text();
		assert!(
			detail.contains("salt") && detail.contains("someone@else"),
			"{detail}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_readers_own_fresh_secret_variable() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-own", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-own-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "dev",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_success();

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-own" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_once_someone_elses_secret_change_has_aged() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-aged", json!({})).await;
		insert_application(
			&mut conn,
			group,
			"kamaka-aged-central",
			"tamanu-central",
			None,
			json!({}),
		)
		.await;
		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "dev",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_success();
		conn.batch_execute(&format!(
			"UPDATE inventory_secret_variables
			 SET set_by = 'someone@else', updated_at = now() - interval '1 hour'
			 WHERE server_group_id = '{group}'"
		))
		.await
		.expect("age the change");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-aged" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

async fn plan_upgrade(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	extra_columns: &str,
	extra_values: &str,
) {
	let version = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status)
		 VALUES ('{version}', 2, 63, 0, '', 'published');
		 INSERT INTO upgrade_plans (group_id, target_version_id{extra_columns})
		 VALUES ('{group}', '{version}'{extra_values})"
	))
	.await
	.expect("plan upgrade");
}

/// An upgrade of production is permitted by the group's open plan and refused
/// without one.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_unplanned_upgrade_of_production() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-unplanned", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-unplanned-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-unplanned", "intent": "upgrade" }))
			.await;
		response.assert_status_conflict();
		let detail = response.text();
		assert!(detail.contains("upgrade plan"), "{detail}");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_a_planned_upgrade_of_production() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-planned", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-planned-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		plan_upgrade(&mut conn, group, "", "").await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-planned", "intent": "upgrade" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

/// A withdrawn plan says the deployment is no longer going there, so it
/// permits nothing.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_upgrade_whose_plan_was_withdrawn() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-withdrawn", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-withdrawn-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		plan_upgrade(
			&mut conn,
			group,
			", withdrawn_at, withdrawn_by",
			", now(), 'someone@else'",
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-withdrawn", "intent": "upgrade" }))
			.await;
		response.assert_status_conflict();
	})
	.await
}

/// A configuration run moves nothing, so there is nothing to plan.
#[tokio::test(flavor = "multi_thread")]
async fn serves_an_unplanned_configuration_run_on_production() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-config", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-config-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-config" }))
			.await;
		response.assert_status_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_an_unplanned_upgrade_at_another_rank() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-demo-up", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo-up-central",
			"tamanu-central",
			Some("demo"),
			None,
			json!({}),
		)
		.await;

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "group": "kamaka-demo-up", "intent": "upgrade" }))
			.await;
		response.assert_status_ok();
	})
	.await
}
