//! Endpoint tests for the variables that configure an environment: the three
//! scopes and how they merge, a secret's value reaching a run and never a
//! reader, and a value canopy cannot produce refusing the whole environment.
//!
//! spec: ANSVAR#variables

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ansible::{
	Failures, insert_application, insert_application_on_its_own_machine, insert_group, read_served,
	take_lease,
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

/// The host of a served read for a machine.
fn host_with(body: &Value, machine: Uuid) -> Result<&Value, String> {
	body["hosts"]
		.as_array()
		.and_then(|hosts| hosts.iter().find(|host| host["id"] == machine.to_string()))
		.ok_or_else(|| format!("no host for machine {machine}: {body}"))
}

/// The variables an environment serves a run, each scenario in a group of its
/// own: the three scopes merging name-wise (a machine's over its environment's
/// over its group's, and a group's value reaching every environment in it),
/// values served as the JSON they were stored as, and an `ansible_host`
/// overriding the address Canopy holds.
#[tokio::test(flavor = "multi_thread")]
async fn serves_each_environment_the_variables_set_for_it() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let mut failures = Failures::default();

		let scenario = "the scopes merging";
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
		insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-demo",
			"tamanu-central",
			Some("demo"),
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
			group_scope,
			"elastic_agent_enabled",
			json!(false),
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
		set_var(&private, environment.clone(), "replicas", json!(3), false).await;
		set_var(
			&private,
			environment,
			"extra_hostnames",
			json!(["sync.kamaka.example"]),
			false,
		)
		.await;

		let body = read_served(
			&private,
			json!({ "server_group_id": group, "rank": "production" }),
		)
		.await;

		// The group's and the environment's are served once, beside the
		// machines that carry them.
		failures.eq(
			scenario,
			"the group's timezone",
			&body["vars"]["timezone"],
			"Pacific/Fiji",
		);
		failures.eq(
			scenario,
			"the environment's log level",
			&body["vars"]["log_level"],
			"debug",
		);
		failures.eq(
			scenario,
			"the environment's replicas, a number",
			&body["vars"]["replicas"],
			json!(3),
		);
		failures.eq(
			scenario,
			"a boolean as stored",
			&body["vars"]["elastic_agent_enabled"],
			json!(false),
		);
		failures.eq(
			scenario,
			"an array as stored",
			&body["vars"]["extra_hostnames"],
			json!(["sync.kamaka.example"]),
		);
		failures.eq(
			scenario,
			"the group's timezone on the first host",
			&body["hosts"][0]["vars"]["timezone"],
			"Pacific/Fiji",
		);
		failures.check(
			scenario,
			body["hosts"].as_array().map(Vec::len) == Some(2),
			format!("the demo box is not in production: {body}"),
		);

		if let Some(host) = failures.ok(scenario, host_with(&body, box_of_central)) {
			failures.eq(
				scenario,
				"central's log level",
				&host["vars"]["log_level"],
				"trace",
			);
			failures.eq(
				scenario,
				"central's inherited timezone",
				&host["vars"]["timezone"],
				"Pacific/Fiji",
			);
			// What the machine sets itself is served apart from what it
			// inherits, so a value can be traced to where it is set.
			failures.eq(
				scenario,
				"central's own log level",
				&host["own_vars"]["log_level"],
				"trace",
			);
			failures.check(
				scenario,
				host["own_vars"].get("timezone").is_none(),
				format!("central's own variables carry an inherited one: {host}"),
			);
		}
		// A machine's variables reach no other box in the environment.
		if let Some(host) = failures.ok(scenario, host_with(&body, box_of_facility)) {
			failures.eq(
				scenario,
				"facility's log level, the environment's and not central's",
				&host["vars"]["log_level"],
				"debug",
			);
		}

		let demo = read_served(
			&private,
			json!({ "server_group_id": group, "rank": "demo" }),
		)
		.await;
		failures.eq(
			scenario,
			"the group's timezone at demo",
			&demo["vars"]["timezone"],
			"Pacific/Fiji",
		);
		failures.eq(
			scenario,
			"the group's timezone on the demo host",
			&demo["hosts"][0]["vars"]["timezone"],
			"Pacific/Fiji",
		);
		failures.eq(
			scenario,
			"the group's log level on the demo host, not production's",
			&demo["hosts"][0]["vars"]["log_level"],
			"info",
		);

		let scenario = "an ansible_host";
		let group = insert_group(&mut conn, "kamaka-address").await;
		let (_, machine) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-address-central",
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
		failures.eq(
			scenario,
			"the host's address",
			&body["hosts"][0]["address"],
			"10.0.0.4",
		);

		failures.assert_none();
	})
	.await
}

/// A secret is served to a run and marked as one, overlays the way a plain
/// value does, is listed by name and never by value, and turning it plain or
/// removing it forgets the value it held. Each scenario has a group of its own.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_is_served_to_a_run_and_shown_to_no_reader() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let mut failures = Failures::default();

		let scenario = "a secret reaching a run";
		let group = insert_group(&mut conn, "kamaka-served").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-served-central",
			"tamanu-central",
			None,
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
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		failures.eq(scenario, "the value", &body["vars"]["sync_salt"], "pepper");
		failures.eq(
			scenario,
			"the names marked secret",
			&body["secret_vars"],
			json!(["sync_salt"]),
		);
		let host = &body["hosts"][0];
		failures.eq(
			scenario,
			"the host's value",
			&host["vars"]["sync_salt"],
			"pepper",
		);
		failures.eq(
			scenario,
			"the host's names marked secret",
			&host["secret_vars"],
			json!(["sync_salt"]),
		);

		let scenario = "a machine's secret over the environment's";
		let group = insert_group(&mut conn, "kamaka-overlay").await;
		let (_, machine) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-overlay-central",
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
		failures.eq(
			scenario,
			"the environment's value",
			&body["vars"]["api_key"],
			"shared",
		);
		failures.eq(
			scenario,
			"the machine's value",
			&body["hosts"][0]["vars"]["api_key"],
			"mine",
		);
		failures.eq(
			scenario,
			"the merge stays marked secret",
			&body["hosts"][0]["secret_vars"],
			json!(["api_key"]),
		);

		// A name a group holds as a secret and an environment sets in the open
		// is served as the plain value it is, so nothing tells a run to
		// withhold it.
		let scenario = "a secret set plain at the environment";
		let group = insert_group(&mut conn, "kamaka-plain-over").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-plain-over-central",
			"tamanu-central",
			None,
		)
		.await;
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
		failures.eq(scenario, "the value", &body["vars"]["salt"], "visible");
		failures.eq(
			scenario,
			"the names marked secret",
			&body["secret_vars"],
			json!([]),
		);
		failures.eq(
			scenario,
			"the host's value",
			&body["hosts"][0]["vars"]["salt"],
			"visible",
		);
		failures.eq(
			scenario,
			"the host's names marked secret",
			&body["hosts"][0]["secret_vars"],
			json!([]),
		);

		let scenario = "the group page's listing";
		let group = insert_group(&mut conn, "kamaka-listed").await;
		let (_, machine) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-listed-central",
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
		failures.check(
			scenario,
			listed.len() == 3,
			format!("three variables listed: {listed:?}"),
		);
		let listing_of = |name: &str| -> Result<&Value, String> {
			listed
				.iter()
				.find(|variable| variable["name"] == name)
				.ok_or_else(|| format!("{name} is listed: {listed:?}"))
		};
		// A secret is listed by name, with the scope it is set at and when it
		// last changed, and never its value.
		if let Some(secret) = failures.ok(scenario, listing_of("sync_salt")) {
			failures.eq(
				scenario,
				"the secret's flag",
				&secret["is_secret"],
				json!(true),
			);
			failures.eq(
				scenario,
				"the secret's value",
				&secret["value"],
				Value::Null,
			);
			failures.eq(scenario, "the secret's rank", &secret["rank"], "dev");
			failures.eq(
				scenario,
				"who set the secret",
				&secret["set_by"],
				crate::ansible::ME,
			);
		}
		if let Some(plain) = failures.ok(scenario, listing_of("timezone")) {
			failures.eq(
				scenario,
				"the group's value",
				&plain["value"],
				"Pacific/Fiji",
			);
			failures.eq(scenario, "the group's rank", &plain["rank"], Value::Null);
		}
		if let Some(on_machine) = failures.ok(scenario, listing_of("log_level")) {
			failures.eq(
				scenario,
				"the machine's machine",
				&on_machine["machine_id"],
				machine.to_string(),
			);
		}

		let scenario = "a secret removed";
		let group = insert_group(&mut conn, "kamaka-removed").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-removed-central",
			"tamanu-central",
			None,
		)
		.await;
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
		let again = private
			.post("/api/ansible_variables/remove")
			.json(&args)
			.await;
		failures.check(
			scenario,
			again.status_code() == axum::http::StatusCode::NOT_FOUND,
			format!("removing it again answered {}", again.status_code()),
		);
		// And the name is free to come back as a plain variable, carrying no
		// trace of the value it held.
		set_var(&private, scope, "sync_salt", json!("visible"), false).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		failures.eq(scenario, "the value", &body["vars"]["sync_salt"], "visible");
		failures.eq(
			scenario,
			"the names marked secret",
			&body["secret_vars"],
			json!([]),
		);

		// Turning a secret into a plain variable forgets the stored value, so a
		// name that comes back as a secret cannot resurrect the one it used to
		// hold.
		let scenario = "a secret turned plain";
		let group = insert_group(&mut conn, "kamaka-turned-plain").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-turned-plain-central",
			"tamanu-central",
			None,
		)
		.await;
		let scope = json!({ "server_group_id": group, "rank": "dev" });
		set_var(&private, scope.clone(), "salt", json!("pepper"), true).await;
		set_var(&private, scope, "salt", json!("visible"), false).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		failures.eq(scenario, "the value", &body["vars"]["salt"], "visible");
		failures.eq(
			scenario,
			"the names marked secret",
			&body["secret_vars"],
			json!([]),
		);

		failures.assert_none();
	})
	.await
}

/// A run receiving a machine that looks configured and is missing a value, or
/// two at one address, is worse than one that does not run, so the read is
/// refused. Each scenario has a group of its own.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_environment_it_cannot_serve_whole() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let mut failures = Failures::default();

		let scenario = "a secret with no value";
		let group = insert_group(&mut conn, "kamaka-no-value").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-no-value-central",
			"tamanu-central",
			None,
		)
		.await;
		secret_without_value(&mut conn, group, "dev", "sync_salt").await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let (status, detail) = read_under(&private, lease).await;
		failures.check(
			scenario,
			status == axum::http::StatusCode::BAD_GATEWAY,
			format!("answered {status}: {detail}"),
		);
		failures.check(
			scenario,
			detail.contains("sync_salt"),
			format!("the detail names the secret: {detail}"),
		);

		// The name coming back as a secret after it was turned plain finds
		// nothing behind it, which refuses the read rather than serving what it
		// held before.
		let scenario = "a name turned plain and declared a secret again";
		let group = insert_group(&mut conn, "kamaka-turned-plain-back").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-turned-plain-back-central",
			"tamanu-central",
			None,
		)
		.await;
		let scope = json!({ "server_group_id": group, "rank": "dev" });
		set_var(&private, scope.clone(), "salt", json!("pepper"), true).await;
		set_var(&private, scope.clone(), "salt", json!("visible"), false).await;
		let mut args = scope;
		args["name"] = json!("salt");
		private
			.post("/api/ansible_variables/remove")
			.json(&args)
			.await
			.assert_status_ok();
		secret_without_value(&mut conn, group, "dev", "salt").await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let (status, detail) = read_under(&private, lease).await;
		failures.check(
			scenario,
			status == axum::http::StatusCode::BAD_GATEWAY,
			format!("answered {status}: {detail}"),
		);
		failures.check(
			scenario,
			detail.contains("salt"),
			format!("the detail names the secret: {detail}"),
		);

		let scenario = "two machines at one address";
		let group = insert_group(&mut conn, "kamaka-shared-address").await;
		let (_, central) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-shared-address-central",
			"tamanu-central",
			None,
			Some("https://shared.kamaka.example/"),
		)
		.await;
		insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-shared-address-facility",
			"tamanu-facility",
			None,
			Some("https://shared.kamaka.example/"),
		)
		.await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		let (status, detail) = read_under(&private, lease).await;
		failures.check(
			scenario,
			status == axum::http::StatusCode::CONFLICT,
			format!("answered {status}: {detail}"),
		);
		failures.check(
			scenario,
			detail.contains("both reached at"),
			format!("the detail says where: {detail}"),
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
		let (status, detail) = read_under(&private, lease).await;
		failures.check(
			scenario,
			status == axum::http::StatusCode::OK,
			format!("answered {status} once one box is named: {detail}"),
		);

		failures.assert_none();
	})
	.await
}

/// What a read under a lease answered, and the detail of a refusal.
async fn read_under(
	private: &commons_tests::axum_test::TestServer,
	lease: Uuid,
) -> (axum::http::StatusCode, String) {
	let response = private
		.post("/api/ansible/for_group")
		.json(&json!({ "lease_id": lease }))
		.await;
	let status = response.status_code();
	let detail = response.json::<Value>()["detail"]
		.as_str()
		.unwrap_or_default()
		.to_owned();
	(status, detail)
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
/// read, it is offered, a value set before it that no longer matches stays,
/// marked where it does not, a variable it describes must be secret exactly as
/// it says, a violation of it is a 400, and a name it does not describe is free.
/// Whether a run acts on a machine is set by a switch on the machine, so a wider
/// value is refused; the rule is `variable_problems`'s, and this is the endpoint
/// applying it.
// spec: ANSVAR#catalogue
// spec: INV#managed-machines
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

		let machine_only = refused(
			&private,
			json!({ "server_group_id": group }),
			"managed_by_ansible",
			json!(true),
			false,
		)
		.await;
		assert!(
			machine_only.contains("set on a machine"),
			"a machine-only variable is refused at group scope: {machine_only}"
		);
		set_var(
			&private,
			json!({ "server_group_id": group }),
			"ansible_user",
			json!("ubuntu"),
			false,
		)
		.await;

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

		let held: Value = private
			.post("/api/ansible_variables/catalogue")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(held["source_commit"], "abc123");
		assert_eq!(held["variables"].as_array().unwrap().len(), 4);

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

		// While its value is held, undoing the removal puts the secret back.
		let mut restore = named(&scope, "salt");
		restore["entry_id"] = removed[0]["entry_id"].clone();
		post(&private, "/api/ansible_variables/restore", restore).await;
		let body = read_served(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["vars"]["salt"], "pepper");
		assert_eq!(body["secret_vars"], json!(["salt"]));

		// A name removed twice is listed once, by its latest removal.
		let second = post(
			&private,
			"/api/ansible_variables/remove",
			named(&scope, "salt"),
		)
		.await;
		let removed = post(&private, "/api/ansible_variables/removed", scope.clone()).await;
		assert_eq!(removed.as_array().unwrap().len(), 1, "{removed}");
		assert_eq!(removed[0]["name"], "salt");
		assert_eq!(removed[0]["entry_id"], second["entry_id"]);
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
