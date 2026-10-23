//! Endpoint tests for the variables that configure an environment: the three
//! scopes and how they merge, a secret's value reaching a run and never a
//! reader, and a value canopy cannot produce refusing the whole environment.
//!
//! spec: ANSVAR#variables

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ansible::{
	insert_application, insert_application_on_its_own_machine, insert_group,
	insert_ranked_application, read_served, take_lease,
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
		.post("/api/ansible_variables/set")
		.json(&args)
		.await
		.assert_status_ok();
}

/// A name with no value behind it, which only a lost or half-written secret
/// store would produce.
async fn secret_without_value(conn: &mut AsyncPgConnection, group: Uuid, rank: &str, name: &str) {
	conn.batch_execute(&format!(
		"INSERT INTO ansible_variables (server_group_id, rank, name, is_secret)
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

		let body = read_served(
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["api_key"], "shared");
		assert_eq!(body["hosts"][0]["vars"]["api_key"], "mine");
		assert_eq!(body["hosts"][0]["secret_vars"], json!(["api_key"]));
	})
	.await
}

/// A run receiving a machine that looks configured and is missing a value is
/// worse than one that does not run.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_with_no_value_refuses_the_environment() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		secret_without_value(&mut conn, group, "dev", "sync_salt").await;

		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let response = private
			.post("/api/ansible/for_group")
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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
			.post("/api/ansible_variables/for_group")
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
		assert_eq!(secret["set_by"], crate::ansible::ME);

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
			.post("/api/ansible_variables/remove")
			.json(&args)
			.await
			.assert_status_ok();
		// Gone, so removing it again finds nothing.
		private
			.post("/api/ansible_variables/remove")
			.json(&args)
			.await
			.assert_status(axum::http::StatusCode::NOT_FOUND);

		// And the name is free to come back as a plain variable, carrying no
		// trace of the value it held.
		set_var(&private, scope, "sync_salt", json!("visible"), false).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
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
				.post("/api/ansible_variables/set")
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
				read_served(&private, json!({ "server_group_id": group, "rank": rank })).await;
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["salt"], "visible");
		assert_eq!(body["secret_vars"], json!([]));

		// The name coming back as a secret finds nothing behind it, which
		// refuses the read rather than serving what it held before.
		let mut args = scope;
		args["name"] = json!("salt");
		private
			.post("/api/ansible_variables/remove")
			.json(&args)
			.await
			.assert_status_ok();
		secret_without_value(&mut conn, group, "dev", "salt").await;

		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let response = private
			.post("/api/ansible/for_group")
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
			.post("/api/ansible_variables/set")
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
			.post("/api/ansible_variables/for_group")
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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

/// Whether a run acts on a machine is set by a switch on the machine, so a
/// wider value is refused, and `ansible_host` at a wider scope would give every
/// machine in the environment one address, so it is set on a machine too.
// spec: INV#managed-machines
#[tokio::test(flavor = "multi_thread")]
async fn refuses_machine_only_variables_outside_machine_scope() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		for (name, value, reason) in [
			("managed_by_ansible", json!(true), "set on a machine"),
			(
				"ansible_host",
				json!("10.0.0.4"),
				"rather than on a group or an environment",
			),
		] {
			for scope in [
				json!({ "server_group_id": group }),
				json!({ "server_group_id": group, "rank": "dev" }),
			] {
				let mut args = scope;
				args["name"] = json!(name);
				args["value"] = value.clone();
				let response = private.post("/api/ansible_variables/set").json(&args).await;
				response.assert_status(axum::http::StatusCode::BAD_REQUEST);
				assert!(
					response.json::<Value>()["detail"]
						.as_str()
						.expect("detail")
						.contains(reason),
					"{name} at {args}",
				);
			}
		}

		// A machine's address is a string, so a value of another shape is
		// refused rather than served as no override at all.
		let response = private
			.post("/api/ansible_variables/set")
			.json(&json!({
				"machine_id": Uuid::new_v4(),
				"name": "ansible_host",
				"value": 10,
			}))
			.await;
		response.assert_status(axum::http::StatusCode::BAD_REQUEST);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("so it is a string"),
		);

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
			.post("/api/ansible/for_group")
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
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status_ok();
	})
	.await
}

/// A small catalogue: a typed variable, one limited to machines, a secret, and
/// one a run sets for itself.
fn catalogue() -> Value {
	json!({
		"$schema": "https://json-schema.org/draft/2020-12/schema",
		"type": "object",
		"properties": {
			"ansible_port": { "type": "integer", "minimum": 1, "maximum": 65535 },
			"tamanu_facility_id": { "type": "string", "x-canopy": { "scope": ["machine"] } },
			"salt": { "type": "string", "x-canopy": { "secret": true } },
			"canopy_intent": { "type": "string", "x-canopy": { "reserved": true } }
		},
		"x-canopy": { "playbooks": {
			"base-install.yml": { "description": "The host arrangement.", "intent": "configure", "tags": ["base"] }
		} }
	})
}

async fn read_catalogue(conn: &mut AsyncPgConnection) {
	database::ansible_catalogue::CatalogueState::record_read(
		conn,
		&catalogue(),
		Some("abc123"),
		jiff::Timestamp::now(),
	)
	.await
	.expect("read a catalogue");
}

async fn refused(
	private: &commons_tests::axum_test::TestServer,
	scope: Value,
	name: &str,
	value: Value,
	secret: bool,
) -> String {
	let mut args = scope;
	args["name"] = json!(name);
	args["value"] = value;
	args["secret"] = json!(secret);
	let response = private.post("/api/ansible_variables/set").json(&args).await;
	response.assert_status(axum::http::StatusCode::BAD_REQUEST);
	response.json::<Value>()["detail"]
		.as_str()
		.expect("detail")
		.to_owned()
}

/// Until a catalogue is read nothing is held to one and none is offered. Once
/// read, it is offered, a variable it describes must be secret exactly as it
/// says, a violation of it is a 400, and a name it does not describe is free.
// spec: ANSVAR#catalogue
#[tokio::test(flavor = "multi_thread")]
async fn a_variable_the_catalogue_describes_is_held_to_it() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		let environment = json!({ "server_group_id": group, "rank": "production" });

		let none: Value = private
			.post("/api/ansible_variables/catalogue")
			.json(&json!({}))
			.await
			.json();
		assert!(none.is_null(), "{none}");
		set_var(
			&private,
			environment.clone(),
			"ansible_port",
			json!("not a port"),
			false,
		)
		.await;
		read_catalogue(&mut conn).await;

		let held: Value = private
			.post("/api/ansible_variables/catalogue")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(held["source_commit"], "abc123");
		assert_eq!(held["variables"].as_array().unwrap().len(), 4);

		assert!(
			refused(
				&private,
				environment.clone(),
				"tamanu_facility_id",
				json!("a"),
				false
			)
			.await
			.contains("only at machine")
		);

		assert!(
			refused(
				&private,
				environment.clone(),
				"salt",
				json!("pepper"),
				false
			)
			.await
			.contains("is secret in the catalogue")
		);
		set_var(&private, environment.clone(), "salt", json!("pepper"), true).await;
		assert!(
			refused(
				&private,
				environment.clone(),
				"ansible_port",
				json!(22),
				true
			)
			.await
			.contains("is not secret in the catalogue")
		);

		set_var(
			&private,
			environment.clone(),
			"something_new",
			json!({ "any": [1] }),
			false,
		)
		.await;
	})
	.await
}

/// A value set before the catalogue described it stays, marked where it no
/// longer matches.
// spec: ANSVAR#catalogue
#[tokio::test(flavor = "multi_thread")]
async fn marks_a_value_that_no_longer_matches_the_catalogue() {
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
		let environment = json!({ "server_group_id": group, "rank": "production" });
		set_var(
			&private,
			environment.clone(),
			"ansible_port",
			json!("2222"),
			false,
		)
		.await;
		set_var(
			&private,
			environment.clone(),
			"salt",
			json!("pepper"),
			false,
		)
		.await;
		set_var(
			&private,
			environment.clone(),
			"timezone",
			json!("Pacific/Fiji"),
			false,
		)
		.await;
		read_catalogue(&mut conn).await;

		let listed: Value = private
			.post("/api/ansible_variables/for_group")
			.json(&json!({ "server_group_id": group }))
			.await
			.json();
		let problems = |name: &str| -> Vec<String> {
			let variable = listed
				.as_array()
				.unwrap()
				.iter()
				.find(|variable| variable["name"] == name)
				.unwrap_or_else(|| panic!("{name} is listed: {listed}"));
			serde_json::from_value(variable["problems"].clone()).unwrap()
		};

		assert_eq!(problems("ansible_port").len(), 1, "{listed}");
		assert!(problems("ansible_port")[0].contains("ansible_port"));
		assert!(problems("salt")[0].contains("secret in the catalogue"));
		assert!(
			problems("timezone").is_empty(),
			"a name the catalogue lacks is unmarked"
		);
	})
	.await
}

async fn post(private: &commons_tests::axum_test::TestServer, path: &str, args: Value) -> Value {
	let response = private.post(path).json(&args).await;
	response.assert_status_ok();
	response.json()
}

fn changes(history: &Value) -> Vec<(&str, &str)> {
	history
		.as_array()
		.unwrap()
		.iter()
		.map(|change| {
			(
				change["name"].as_str().unwrap(),
				change["change"].as_str().unwrap(),
			)
		})
		.collect()
}

fn named(scope: &Value, name: &str) -> Value {
	let mut args = scope.clone();
	args["name"] = json!(name);
	args
}

/// A variable's history is its audit entries, newest first, with the plain
/// value each set and the one it replaced, and a set can be undone, putting
/// back what it replaced.
// spec: ANSVAR#history
#[tokio::test(flavor = "multi_thread")]
async fn a_plain_change_is_in_the_history_and_can_be_undone() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let scope = json!({ "server_group_id": group, "rank": "dev" });
		set_var(&private, scope.clone(), "log_level", json!("info"), false).await;
		set_var(&private, scope.clone(), "log_level", json!("debug"), false).await;
		// Another variable's changes are not in this one's history.
		set_var(&private, scope.clone(), "log_level_extra", json!(1), false).await;

		let history = post(
			&private,
			"/api/ansible_variables/history",
			named(&scope, "log_level"),
		)
		.await;
		let history = history.as_array().unwrap();
		assert_eq!(history.len(), 2, "{history:?}");
		assert_eq!(history[0]["change"], "set");
		assert_eq!(history[0]["value"], "debug");
		assert_eq!(history[0]["replaced"], "info");
		assert_eq!(history[0]["undoable"], true);
		assert_eq!(history[0]["actor"]["login"], "admin@localhost");
		// The first set replaced nothing, so there is nothing to put back.
		assert_eq!(history[1]["value"], "info");
		assert_eq!(history[1]["undoable"], false);

		let mut restore = named(&scope, "log_level");
		restore["entry_id"] = history[0]["entry_id"].clone();
		post(&private, "/api/ansible_variables/restore", restore).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["log_level"], "info");

		// The restore is a set of its own, naming the change it undid.
		let history = post(
			&private,
			"/api/ansible_variables/history",
			named(&scope, "log_level"),
		)
		.await;
		assert_eq!(history[0]["value"], "info");
		assert_eq!(history[0]["replaced"], "debug");
		assert_eq!(history[0]["restored_from"], history[1]["entry_id"]);

		let mut first = named(&scope, "log_level");
		first["entry_id"] = history[2]["entry_id"].clone();
		private
			.post("/api/ansible_variables/restore")
			.json(&first)
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);

		// A change to another variable is not restored as this one.
		let other = post(
			&private,
			"/api/ansible_variables/history",
			named(&scope, "log_level_extra"),
		)
		.await;
		let mut wrong = named(&scope, "log_level");
		wrong["entry_id"] = other[0]["entry_id"].clone();
		private
			.post("/api/ansible_variables/restore")
			.json(&wrong)
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);
	})
	.await
}

/// A removal is listed at its scope for a week, and undoing it puts back the
/// value it removed. A scope's history holds every change to a variable set
/// there, removed ones among them and however long ago within the log, and
/// nothing of another scope's, though the group's record names its
/// environments' too.
// spec: ANSVAR#presentation
#[tokio::test(flavor = "multi_thread")]
async fn a_removal_is_listed_at_its_scope_and_can_be_undone() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let environment = json!({ "server_group_id": group, "rank": "dev" });
		let group_scope = json!({ "server_group_id": group });
		set_var(&private, environment.clone(), "retries", json!(3), false).await;
		set_var(&private, environment.clone(), "kept", json!(true), false).await;
		set_var(
			&private,
			group_scope.clone(),
			"timezone",
			json!("Pacific/Fiji"),
			false,
		)
		.await;
		let removal = post(
			&private,
			"/api/ansible_variables/remove",
			named(&environment, "retries"),
		)
		.await;
		post(
			&private,
			"/api/ansible_variables/remove",
			named(&group_scope, "timezone"),
		)
		.await;

		// Each scope lists only its own removals.
		let removed = post(
			&private,
			"/api/ansible_variables/removed",
			environment.clone(),
		)
		.await;
		let removed = removed.as_array().unwrap();
		assert_eq!(removed.len(), 1, "{removed:?}");
		assert_eq!(removed[0]["name"], "retries");
		assert_eq!(removed[0]["value"], 3);
		assert_eq!(removed[0]["undoable"], true);
		// The removal answers with the entry that undoes it.
		assert_eq!(removed[0]["entry_id"], removal["entry_id"]);
		let at_group = post(
			&private,
			"/api/ansible_variables/removed",
			group_scope.clone(),
		)
		.await;
		assert_eq!(at_group[0]["name"], "timezone");

		let mut restore = named(&environment, "retries");
		restore["entry_id"] = removed[0]["entry_id"].clone();
		post(&private, "/api/ansible_variables/restore", restore).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["retries"], 3);

		// Set again, it is no longer listed as removed.
		let removed = post(
			&private,
			"/api/ansible_variables/removed",
			environment.clone(),
		)
		.await;
		assert_eq!(removed, json!([]));

		// Removed long enough ago to be off the list of recent removals, a
		// removal is still in its scope's history.
		// A month back is a week that is never provisioned ahead, and an entry
		// is never updated once settled.
		conn.batch_execute(
			"DO $$ DECLARE week_start date := date_trunc('week', now() - interval '30 days')::date; BEGIN \
			   EXECUTE format('CREATE TABLE audit_entries_aged PARTITION OF audit_entries \
			     FOR VALUES FROM (%L) TO (%L) PARTITION BY LIST (retention)', week_start, week_start + 7); \
			   CREATE TABLE audit_entries_aged_standard PARTITION OF audit_entries_aged FOR VALUES IN ('standard'); \
			   CREATE TABLE audit_entries_aged_year PARTITION OF audit_entries_aged FOR VALUES IN ('year'); \
			 END $$; \
			 ALTER TABLE audit_entries DISABLE TRIGGER audit_entries_guard; \
			 UPDATE audit_entries SET recorded_at = recorded_at - interval '30 days' \
			 WHERE action = 'inventory_variable_removed'; \
			 ALTER TABLE audit_entries ENABLE TRIGGER audit_entries_guard",
		)
		.await
		.unwrap();
		let recent = post(
			&private,
			"/api/ansible_variables/removed",
			group_scope.clone(),
		)
		.await;
		assert_eq!(recent, json!([]));

		let history = post(
			&private,
			"/api/ansible_variables/scope_history",
			environment,
		)
		.await;
		assert_eq!(
			changes(&history),
			[
				("retries", "set"),
				("kept", "set"),
				("retries", "set"),
				("retries", "removed")
			],
			"newest first, the restore a set of its own, the group's own changes left out"
		);
		assert_eq!(history[3]["value"], 3);
		assert_eq!(history[3]["undoable"], true);

		let at_group = post(
			&private,
			"/api/ansible_variables/scope_history",
			group_scope,
		)
		.await;
		assert_eq!(
			changes(&at_group),
			[("timezone", "set"), ("timezone", "removed")]
		);
	})
	.await
}

/// A secret's value a change replaced or removed is held for a week, so the
/// change can be undone, and after it the value is forgotten.
// spec: ANSVAR#history
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_change_is_undone_while_its_value_is_held() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let scope = json!({ "server_group_id": group, "rank": "dev" });
		set_var(&private, scope.clone(), "salt", json!("pepper"), true).await;
		set_var(&private, scope.clone(), "salt", json!("paprika"), true).await;

		let history = post(
			&private,
			"/api/ansible_variables/history",
			named(&scope, "salt"),
		)
		.await;
		assert_eq!(history[0]["secret"], true);
		assert!(history[0]["value"].is_null());
		assert_eq!(history[0]["replaced_secret"], true);
		assert_eq!(history[0]["undoable"], true);
		assert!(
			!history.to_string().contains("pepper"),
			"no secret in the history"
		);

		let mut restore = named(&scope, "salt");
		restore["entry_id"] = history[0]["entry_id"].clone();
		post(&private, "/api/ansible_variables/restore", restore).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["salt"], "pepper");
		assert_eq!(body["secret_vars"], json!(["salt"]));

		// Removed, its value is held too.
		private
			.post("/api/ansible_variables/remove")
			.json(&named(&scope, "salt"))
			.await
			.assert_status_ok();
		let removed = post(&private, "/api/ansible_variables/removed", scope.clone()).await;
		assert_eq!(removed[0]["undoable"], true);

		// Once its week has passed, the value is forgotten and the change can no
		// longer be undone.
		conn.batch_execute(
			"UPDATE ansible_secret_values_held SET held_until = now() - interval '1 minute'",
		)
		.await
		.unwrap();
		let removed = post(&private, "/api/ansible_variables/removed", scope.clone()).await;
		assert_eq!(removed[0]["undoable"], false);
		let mut restore = named(&scope, "salt");
		restore["entry_id"] = removed[0]["entry_id"].clone();
		let refused = private
			.post("/api/ansible_variables/restore")
			.json(&restore)
			.await;
		refused.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(
			refused.json::<Value>()["detail"]
				.as_str()
				.unwrap()
				.contains("escrow")
		);
	})
	.await
}
