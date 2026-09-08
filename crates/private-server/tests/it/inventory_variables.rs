//! Endpoint tests for the variables that configure an environment: the three
//! scopes and how they merge, a secret's value reaching a run and never a
//! reader, and a value canopy cannot produce refusing the whole inventory.
//!
//! spec: INV#inventory-variables

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::inventory::{
	insert_application, insert_application_on_its_own_machine, insert_group,
	insert_ranked_application, read_inventory, take_lease,
};

async fn set_var(
	private: &commons_tests::axum_test::TestServer,
	scope: Value,
	name: &str,
	value: Value,
	secret: bool,
) {
	let mut args = scope;
	args["name"] = json!(name);
	args["value"] = value;
	args["secret"] = json!(secret);
	private
		.post("/api/inventory_variables/set")
		.json(&args)
		.await
		.assert_status_ok();
}

/// A name with no value behind it, which only a lost or half-written secret
/// store would produce.
async fn secret_without_value(conn: &mut AsyncPgConnection, group: Uuid, rank: &str, name: &str) {
	conn.batch_execute(&format!(
		"INSERT INTO inventory_variables (server_group_id, rank, name, is_secret)
		 VALUES ('{group}', '{rank}', '{name}', TRUE)"
	))
	.await
	.expect("declare secret");
}

/// The three scopes merge name-wise, a machine's over its environment's over
/// its group's.
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_value_wins_over_its_environments_and_its_groups() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let (_, box_of_central) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		let (_, box_of_facility) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-facility",
			"tamanu-facility",
			Some("production"),
			None,
		)
		.await;

		let group_scope = json!({ "server_group_id": group });
		let environment = json!({ "server_group_id": group, "rank": "production" });
		set_var(
			&private,
			group_scope.clone(),
			"timezone",
			json!("Pacific/Fiji"),
			false,
		)
		.await;
		set_var(
			&private,
			group_scope.clone(),
			"log_level",
			json!("info"),
			false,
		)
		.await;
		set_var(
			&private,
			environment.clone(),
			"log_level",
			json!("debug"),
			false,
		)
		.await;
		set_var(
			&private,
			json!({ "machine_id": box_of_central }),
			"log_level",
			json!("trace"),
			false,
		)
		.await;
		set_var(&private, environment, "replicas", json!(3), false).await;

		let body = read_inventory(
			&private,
			json!({ "server_group_id": group, "rank": "production" }),
		)
		.await;

		// The group's and the environment's are served once, beside the
		// machines that carry them.
		assert_eq!(body["vars"]["timezone"], "Pacific/Fiji");
		assert_eq!(body["vars"]["log_level"], "debug");
		assert_eq!(body["vars"]["replicas"], json!(3));

		let hosts = body["hosts"].as_array().expect("hosts");
		let central_host = hosts
			.iter()
			.find(|host| host["id"] == box_of_central.to_string())
			.expect("central's machine");
		assert_eq!(central_host["vars"]["log_level"], "trace");
		assert_eq!(central_host["vars"]["timezone"], "Pacific/Fiji");
		// What the machine sets itself is served apart from what it inherits,
		// so a value can be traced to where it is set.
		assert_eq!(central_host["own_vars"]["log_level"], "trace");
		assert!(central_host["own_vars"].get("timezone").is_none());

		let facility_host = hosts
			.iter()
			.find(|host| host["id"] == box_of_facility.to_string())
			.expect("facility's machine");
		assert_eq!(facility_host["vars"]["log_level"], "debug");
	})
	.await
}

/// A value is JSON and is served as it was stored, whatever its type.
#[tokio::test(flavor = "multi_thread")]
async fn serves_a_value_as_the_json_it_was_stored_as() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let scope = json!({ "server_group_id": group });

		set_var(
			&private,
			scope.clone(),
			"elastic_agent_enabled",
			json!(false),
			false,
		)
		.await;
		set_var(&private, scope.clone(), "ansible_port", json!(2222), false).await;
		set_var(
			&private,
			scope.clone(),
			"tamanu_version",
			json!("v2.54.8"),
			false,
		)
		.await;
		set_var(
			&private,
			scope,
			"extra_hostnames",
			json!(["sync.kamaka.example"]),
			false,
		)
		.await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["elastic_agent_enabled"], json!(false));
		assert_eq!(body["vars"]["ansible_port"], json!(2222));
		assert_eq!(body["vars"]["tamanu_version"], "v2.54.8");
		assert_eq!(
			body["vars"]["extra_hostnames"],
			json!(["sync.kamaka.example"])
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_reaches_a_run_and_is_marked_as_one() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		set_var(
			&private,
			json!({ "server_group_id": group, "rank": "dev" }),
			"sync_salt",
			json!("pepper"),
			true,
		)
		.await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["sync_salt"], "pepper");
		assert_eq!(body["secret_vars"], json!(["sync_salt"]));

		let host = &body["hosts"][0];
		assert_eq!(host["vars"]["sync_salt"], "pepper");
		assert_eq!(host["secret_vars"], json!(["sync_salt"]));
	})
	.await
}

/// A machine's secret overlays the environment's, and the merge stays marked
/// secret.
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_secret_overlays_the_environments() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let (_, machine) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			None,
			None,
		)
		.await;

		set_var(
			&private,
			json!({ "server_group_id": group, "rank": "dev" }),
			"api_key",
			json!("shared"),
			true,
		)
		.await;
		set_var(
			&private,
			json!({ "machine_id": machine }),
			"api_key",
			json!("mine"),
			true,
		)
		.await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["api_key"], "shared");
		assert_eq!(body["hosts"][0]["vars"]["api_key"], "mine");
		assert_eq!(body["hosts"][0]["secret_vars"], json!(["api_key"]));
	})
	.await
}

/// A run receiving a machine that looks configured and is missing a value is
/// worse than one that does not run.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_with_no_value_refuses_the_inventory() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		secret_without_value(&mut conn, group, "dev", "sync_salt").await;

		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": lease }))
			.await;
		response.assert_status(axum::http::StatusCode::BAD_GATEWAY);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("sync_salt"),
		);
	})
	.await
}

/// An `ansible_host` variable overrides the address canopy holds.
#[tokio::test(flavor = "multi_thread")]
async fn ansible_host_overrides_the_address() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let (_, machine) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			None,
			Some("https://central.kamaka.example/"),
		)
		.await;
		set_var(
			&private,
			json!({ "machine_id": machine }),
			"ansible_host",
			json!("10.0.0.4"),
			false,
		)
		.await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["hosts"][0]["address"], "10.0.0.4");
	})
	.await
}

/// A secret is listed by name, with the scope it is set at and when it last
/// changed, and never its value.
#[tokio::test(flavor = "multi_thread")]
async fn the_group_page_reads_names_without_secret_values() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let (_, machine) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			None,
			None,
		)
		.await;

		set_var(
			&private,
			json!({ "server_group_id": group }),
			"timezone",
			json!("Pacific/Fiji"),
			false,
		)
		.await;
		set_var(
			&private,
			json!({ "server_group_id": group, "rank": "dev" }),
			"sync_salt",
			json!("pepper"),
			true,
		)
		.await;
		set_var(
			&private,
			json!({ "machine_id": machine }),
			"log_level",
			json!("trace"),
			false,
		)
		.await;

		let response = private
			.post("/api/inventory_variables/for_group")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status_ok();
		let listed = response.json::<Vec<Value>>();
		assert_eq!(listed.len(), 3);

		let secret = listed
			.iter()
			.find(|variable| variable["name"] == "sync_salt")
			.expect("the secret");
		assert_eq!(secret["is_secret"], json!(true));
		assert_eq!(secret["value"], Value::Null);
		assert_eq!(secret["rank"], "dev");
		assert_eq!(secret["set_by"], crate::inventory::ME);

		let plain = listed
			.iter()
			.find(|variable| variable["name"] == "timezone")
			.expect("the group's");
		assert_eq!(plain["value"], "Pacific/Fiji");
		assert_eq!(plain["rank"], Value::Null);

		let on_machine = listed
			.iter()
			.find(|variable| variable["name"] == "log_level")
			.expect("the machine's");
		assert_eq!(on_machine["machine_id"], machine.to_string());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_forgets_the_value() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let scope = json!({ "server_group_id": group, "rank": "dev" });
		set_var(&private, scope.clone(), "sync_salt", json!("pepper"), true).await;

		let mut args = scope.clone();
		args["name"] = json!("sync_salt");
		private
			.post("/api/inventory_variables/remove")
			.json(&args)
			.await
			.assert_status_ok();
		// Gone, so removing it again finds nothing.
		private
			.post("/api/inventory_variables/remove")
			.json(&args)
			.await
			.assert_status(axum::http::StatusCode::NOT_FOUND);

		// And the name is free to come back as a plain variable, carrying no
		// trace of the value it held.
		set_var(&private, scope, "sync_salt", json!("visible"), false).await;
		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["sync_salt"], "visible");
		assert_eq!(body["secret_vars"], json!([]));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_name_the_secret_store_cannot_key_a_value_under() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		for name in ["not a name", "..", "n".repeat(254).as_str()] {
			private
				.post("/api/inventory_variables/set")
				.json(&json!({
					"server_group_id": group,
					"name": name,
					"value": "x",
				}))
				.await
				.assert_status(axum::http::StatusCode::BAD_REQUEST);
		}
	})
	.await
}

/// A group's value reaches every one of its environments, which is the cascade
/// a per-environment table would throw away.
#[tokio::test(flavor = "multi_thread")]
async fn a_groups_value_reaches_every_environment_in_it() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;
		set_var(
			&private,
			json!({ "server_group_id": group }),
			"timezone",
			json!("Pacific/Fiji"),
			false,
		)
		.await;

		for rank in ["production", "demo"] {
			let body =
				read_inventory(&private, json!({ "server_group_id": group, "rank": rank })).await;
			assert_eq!(body["vars"]["timezone"], "Pacific/Fiji", "{rank}");
			assert_eq!(
				body["hosts"][0]["vars"]["timezone"], "Pacific/Fiji",
				"{rank}"
			);
		}
	})
	.await
}

/// Turning a secret into a plain variable forgets the stored value, so a name
/// that comes back as a secret cannot resurrect the one it used to hold.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_turned_plain_forgets_the_value_it_held() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let scope = json!({ "server_group_id": group, "rank": "dev" });

		set_var(&private, scope.clone(), "salt", json!("pepper"), true).await;
		set_var(&private, scope.clone(), "salt", json!("visible"), false).await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["salt"], "visible");
		assert_eq!(body["secret_vars"], json!([]));

		// The name coming back as a secret finds nothing behind it, which
		// refuses the read rather than serving what it held before.
		let mut args = scope;
		args["name"] = json!("salt");
		private
			.post("/api/inventory_variables/remove")
			.json(&args)
			.await
			.assert_status_ok();
		secret_without_value(&mut conn, group, "dev", "salt").await;

		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": lease }))
			.await;
		response.assert_status(axum::http::StatusCode::BAD_GATEWAY);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("salt"),
		);
	})
	.await
}

/// A rank the deployment does not have is a mistake, and answering it at the
/// whole group's scope would put a value meant for one environment in front of
/// every one of them, production included.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_rank_that_is_not_one() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		let response = private
			.post("/api/inventory_variables/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "prod",
				"name": "salt",
				"value": "pepper",
				"secret": true,
			}))
			.await;
		assert_ne!(response.status_code(), axum::http::StatusCode::OK);

		let listed = private
			.post("/api/inventory_variables/for_group")
			.json(&json!({ "server_group_id": group }))
			.await;
		listed.assert_status_ok();
		assert_eq!(listed.json::<Value>(), json!([]));
	})
	.await
}

/// A name a group holds as a secret and an environment sets in the open is
/// served as the plain value it is, so nothing tells a run to withhold it.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_set_plain_at_the_environment_is_no_longer_one() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		set_var(
			&private,
			json!({ "server_group_id": group }),
			"salt",
			json!("pepper"),
			true,
		)
		.await;
		set_var(
			&private,
			json!({ "server_group_id": group, "rank": "dev" }),
			"salt",
			json!("visible"),
			false,
		)
		.await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["salt"], "visible");
		assert_eq!(body["secret_vars"], json!([]));
		assert_eq!(body["hosts"][0]["vars"]["salt"], "visible");
		assert_eq!(body["hosts"][0]["secret_vars"], json!([]));
	})
	.await
}

/// A machine's variables are its own, and reach no other box in the
/// environment.
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_value_reaches_only_that_machine() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let (_, central) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			None,
			None,
		)
		.await;
		let (_, facility) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-facility",
			"tamanu-facility",
			None,
			None,
		)
		.await;
		set_var(
			&private,
			json!({ "machine_id": central }),
			"tamanu_facility_id",
			json!("kamaka-central"),
			false,
		)
		.await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		let hosts = body["hosts"].as_array().expect("hosts");
		let mine = hosts
			.iter()
			.find(|host| host["id"] == central.to_string())
			.expect("central's machine");
		let theirs = hosts
			.iter()
			.find(|host| host["id"] == facility.to_string())
			.expect("facility's machine");
		assert_eq!(mine["vars"]["tamanu_facility_id"], "kamaka-central");
		assert!(theirs["vars"].get("tamanu_facility_id").is_none());
		assert!(body["vars"].get("tamanu_facility_id").is_none());
	})
	.await
}

/// `ansible_host` at a wider scope would give every machine in the environment
/// one address, so it is set on a machine.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_ansible_host_outside_machine_scope() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		for scope in [
			json!({ "server_group_id": group }),
			json!({ "server_group_id": group, "rank": "dev" }),
		] {
			let mut args = scope;
			args["name"] = json!("ansible_host");
			args["value"] = json!("10.0.0.4");
			let response = private
				.post("/api/inventory_variables/set")
				.json(&args)
				.await;
			response.assert_status(axum::http::StatusCode::BAD_REQUEST);
			assert!(
				response.json::<Value>()["detail"]
					.as_str()
					.expect("detail")
					.contains("rather than on a group or an environment"),
			);
		}

		// `ansible_user` is not so restricted: an environment's machines share
		// the account a run connects as.
		set_var(
			&private,
			json!({ "server_group_id": group }),
			"ansible_user",
			json!("ubuntu"),
			false,
		)
		.await;
	})
	.await
}

/// A run receiving two machines at one address would configure one box twice
/// and leave the other untouched.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_environment_whose_machines_share_an_address() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let (_, central) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			None,
			Some("https://shared.kamaka.example/"),
		)
		.await;
		insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-facility",
			"tamanu-facility",
			None,
			Some("https://shared.kamaka.example/"),
		)
		.await;

		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": lease }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("both reached at"),
		);

		// Naming one box explicitly is what resolves it.
		set_var(
			&private,
			json!({ "machine_id": central }),
			"ansible_host",
			json!("central.kamaka.example"),
			false,
		)
		.await;
		private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status_ok();
	})
	.await
}
