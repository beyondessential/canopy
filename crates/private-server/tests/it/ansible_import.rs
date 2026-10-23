//! Importing an Ansible inventory file into an environment: what the preview
//! proposes and refuses, and what applying sets, stores and records.
//!
//! spec: ANSIMP

use commons_tests::{
	axum_test::TestServer,
	diesel_async::{AsyncPgConnection, SimpleAsyncConnection},
};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ansible::{
	Failures, bind_device, insert_application_on_its_own_machine, insert_group, read_served,
};

const FILE: &str = "# Use: --key-file /path/to/ssh-key
# SSH key is in the password manager
all:
  vars:
    timezone: Pacific/Auckland
    tamanu_clone: true
    canopy_intent: configure
    salt: pepper
    elastic_agent_enrollment_token: abc
    tamanu_caddy_tls_dns_iam:
      access_key_id: AKIAEXAMPLE
      secret_access_key: xyz
    tamanu_version: v2.64.3
  hosts:
    # Central is in the cloud
    kamaka-central:
      public_hostname: central.example.org
      tailscale_name: kamaka-central
  children:
    facility:
      vars:
        elastic_agent_enabled: yes
      hosts:
        kamaka-facility-a:
          public_hostname: facility-a.example.org
          tamanu_facility_id: facility-a
        10.20.30.40:223:
          # Behind the site VPN
          ansible_user: siteadmin
          tamanu_facility_id: office
";

struct Environment {
	group: Uuid,
	central: Uuid,
	facility: Uuid,
	office: Uuid,
}

/// A production environment of a central and a facility, each with a device on
/// the tailnet, and an office box that has reported but has no device.
async fn environment(conn: &mut AsyncPgConnection) -> Environment {
	let group = insert_group(conn, "kamaka").await;
	let (central_app, central) = insert_application_on_its_own_machine(
		conn,
		group,
		"kamaka-central",
		"tamanu-central",
		Some("production"),
		Some("https://central.example.org"),
	)
	.await;
	bind_device(conn, central_app, "kamaka-central.tail1234.ts.net").await;
	let (facility_app, facility) = insert_application_on_its_own_machine(
		conn,
		group,
		"kamaka-facility-a",
		"tamanu-facility",
		Some("production"),
		None,
	)
	.await;
	bind_device(conn, facility_app, "kamaka-facility-a.tail1234.ts.net").await;
	let (_, office) = insert_application_on_its_own_machine(
		conn,
		group,
		"kamaka-office",
		"tamanu-facility",
		Some("production"),
		Some("https://office.example.internal"),
	)
	.await;
	Environment {
		group,
		central,
		facility,
		office,
	}
}

/// What infra's catalogue says of the variables the file sets.
async fn read_catalogue(conn: &mut AsyncPgConnection) {
	database::ansible_catalogue::CatalogueState::record_read(
		conn,
		&json!({
			"$schema": "https://json-schema.org/draft/2020-12/schema",
			"type": "object",
			"properties": {
				"salt": { "type": "string", "x-canopy": { "secret": true } },
				"elastic_agent_enrollment_token": { "type": "string", "x-canopy": { "secret": true } },
				"tamanu_caddy_tls_dns_iam": {
					"type": "object",
					"x-canopy": { "secret": true },
					"required": ["access_key_id", "secret_access_key"],
					"properties": {
						"access_key_id": { "type": "string" },
						"secret_access_key": { "type": "string" }
					}
				},
				"elastic_agent_enabled": { "type": "boolean" },
				"canopy_intent": { "type": "string", "x-canopy": { "reserved": true } },
				"tamanu_clone": { "type": "boolean", "x-canopy": { "reserved": true } }
			}
		}),
		None,
		jiff::Timestamp::now(),
	)
	.await
	.expect("read a catalogue");
}

async fn try_preview(private: &TestServer, body: Value) -> Result<Value, String> {
	let response = private
		.post("/api/ansible/import_preview")
		.json(&body)
		.await;
	if !response.status_code().is_success() {
		return Err(format!("import_preview answered {}", response.text()));
	}
	Ok(response.json())
}

async fn preview(private: &TestServer, body: Value) -> Value {
	try_preview(private, body)
		.await
		.unwrap_or_else(|error| panic!("{error}"))
}

/// The entry of a list whose `field` is `key`, or null where it has none.
fn find<'a>(items: &'a Value, field: &str, key: &str) -> &'a Value {
	static MISSING: Value = Value::Null;
	items
		.as_array()
		.into_iter()
		.flatten()
		.find(|item| item[field] == key)
		.unwrap_or(&MISSING)
}

fn names(variables: &Value) -> Vec<&str> {
	variables
		.as_array()
		.into_iter()
		.flatten()
		.filter_map(|variable| variable["name"].as_str())
		.collect()
}

/// The preview proposes and refuses without writing anything, so each scenario
/// previews the one environment and stands on its own file: the machine each
/// host matches (by the names it gives, by a machine's applications' hosts, and
/// left for the operator where none), hosts skipped or assigned by hand, two
/// hosts on one machine or two machines at one address, a file that is not an
/// inventory, secrets by name and then by the catalogue with their values hidden,
/// where each variable would be set, and a variable that breaks the rules. The
/// catalogue is read part way, so the scenarios that must see none come first.
// spec: ANSIMP#matching-hosts-to-machines
// spec: ANSIMP#preview
// spec: ANSIMP#reading-the-file
#[tokio::test(flavor = "multi_thread")]
async fn previews_what_an_import_would_do() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let mut failures = Failures::default();

		let scenario = "a host is proposed the one machine it matches";
		if let Some(preview) = failures.ok(
			scenario,
			try_preview(&private, json!({ "group": "kamaka", "file": FILE })).await,
		) {
			let central = find(&preview["hosts"], "key", "kamaka-central");
			failures.eq(scenario, "central's state", &central["state"], "matched");
			failures.eq(
				scenario,
				"central's machine",
				&central["machine_id"],
				env.central.to_string(),
			);
			let facility = find(&preview["hosts"], "key", "kamaka-facility-a");
			failures.eq(
				scenario,
				"the facility's state",
				&facility["state"],
				"matched",
			);
			failures.eq(
				scenario,
				"the facility's machine",
				&facility["machine_id"],
				env.facility.to_string(),
			);
			let office = find(&preview["hosts"], "key", "10.20.30.40:223");
			failures.eq(
				scenario,
				"the office's state",
				&office["state"],
				"unmatched",
			);
			failures.check(
				scenario,
				office["machine_id"].is_null(),
				format!("the office has no machine: {preview}"),
			);
			failures.eq(scenario, "can_apply", &preview["can_apply"], false);
			failures.check(
				scenario,
				preview["problems"][0]
					.as_str()
					.is_some_and(|problem| problem.contains("10.20.30.40:223")),
				format!("the first problem names the office: {preview}"),
			);
			failures.check(
				scenario,
				preview["machines"].as_array().map(Vec::len) == Some(3),
				format!("three machines: {preview}"),
			);
		}

		let scenario = "a machine with no device matches by its name and its applications' hosts";
		let file = "all:
  hosts:
    kamaka-office:
    stranger:
      public_hostname: office.example.internal
";
		if let Some(preview) = failures.ok(
			scenario,
			try_preview(&private, json!({ "group": "kamaka", "file": file })).await,
		) {
			failures.eq(
				scenario,
				"the office by its name",
				&find(&preview["hosts"], "key", "kamaka-office")["machine_id"],
				env.office.to_string(),
			);
			// By a host recorded for an application, which it names, but the
			// machine is also named by the first host, so it is ambiguous.
			let stranger = find(&preview["hosts"], "key", "stranger");
			failures.eq(
				scenario,
				"the stranger's state",
				&stranger["state"],
				"matched",
			);
			failures.eq(
				scenario,
				"the stranger's machine",
				&stranger["machine_id"],
				env.office.to_string(),
			);
			failures.eq(
				scenario,
				"can_apply with two hosts on one machine",
				&preview["can_apply"],
				false,
			);
			failures.check(
				scenario,
				preview["problems"].as_array().is_some_and(|problems| {
					problems.iter().any(|problem| {
						problem
							.as_str()
							.is_some_and(|p| p.contains("kamaka-office"))
					})
				}),
				format!("a problem names kamaka-office: {preview}"),
			);
		}

		let scenario = "a skipped host carries nothing and an assigned one applies";
		if let Some(skipped) = failures.ok(
			scenario,
			try_preview(
				&private,
				json!({
					"group": "kamaka", "file": FILE,
					"hosts": [{ "host": "10.20.30.40:223", "machine_id": null }],
				}),
			)
			.await,
		) {
			let office = find(&skipped["hosts"], "key", "10.20.30.40:223");
			failures.eq(scenario, "the skipped state", &office["state"], "skipped");
			failures.eq(
				scenario,
				"the skipped variables",
				&office["variables"],
				json!([]),
			);
			failures.eq(scenario, "the skipped notes", &office["notes"], "");
			failures.eq(
				scenario,
				"can_apply once skipped",
				&skipped["can_apply"],
				true,
			);
		}
		if let Some(assigned) = failures.ok(
			scenario,
			try_preview(
				&private,
				json!({
					"group": "kamaka", "file": FILE,
					"hosts": [{ "host": "10.20.30.40:223", "machine_id": env.office }],
				}),
			)
			.await,
		) {
			failures.eq(
				scenario,
				"the assigned state",
				&find(&assigned["hosts"], "key", "10.20.30.40:223")["state"],
				"assigned",
			);
			failures.eq(
				scenario,
				"can_apply once assigned",
				&assigned["can_apply"],
				true,
			);
		}
		let outside = private
			.post("/api/ansible/import_preview")
			.json(&json!({
				"group": "kamaka", "file": FILE,
				"hosts": [{ "host": "10.20.30.40:223", "machine_id": Uuid::new_v4() }],
			}))
			.await;
		failures.check(
			scenario,
			outside.status_code() == axum::http::StatusCode::BAD_REQUEST,
			format!(
				"a machine outside the group answered {}",
				outside.status_code()
			),
		);

		let scenario = "two hosts to one machine, or two machines at one address";
		if let Some(both) = failures.ok(
			scenario,
			try_preview(
				&private,
				json!({
					"group": "kamaka", "file": FILE,
					"hosts": [
						{ "host": "kamaka-facility-a", "machine_id": env.central },
						{ "host": "10.20.30.40:223", "machine_id": null },
					],
				}),
			)
			.await,
		) {
			failures.eq(
				scenario,
				"can_apply for two hosts",
				&both["can_apply"],
				false,
			);
			failures.check(
				scenario,
				both["problems"][0]
					.as_str()
					.is_some_and(|problem| problem.contains("are all matched to machine")),
				format!("the first problem says the hosts share a machine: {both}"),
			);
		}
		let file = "all:
  hosts:
    kamaka-central:
      ansible_host: shared.example.org
    kamaka-facility-a:
      ansible_host: shared.example.org
";
		if let Some(shared) = failures.ok(
			scenario,
			try_preview(&private, json!({ "group": "kamaka", "file": file })).await,
		) {
			failures.eq(
				scenario,
				"can_apply for a shared address",
				&shared["can_apply"],
				false,
			);
			failures.check(
				scenario,
				shared["problems"].as_array().is_some_and(|problems| {
					problems.iter().any(|problem| {
						problem
							.as_str()
							.is_some_and(|p| p.contains("shared.example.org"))
					})
				}),
				format!("a problem names the shared address: {shared}"),
			);
		}

		let scenario = "a file that is not an inventory";
		let response = private
			.post("/api/ansible/import_preview")
			.json(&json!({ "group": "kamaka", "file": "plugin: canopy\n" }))
			.await;
		failures.check(
			scenario,
			response.status_code() == axum::http::StatusCode::BAD_REQUEST,
			format!("answered {}", response.status_code()),
		);
		let detail = response.json::<Value>()["detail"]
			.as_str()
			.unwrap_or_default()
			.to_owned();
		failures.check(
			scenario,
			detail.contains("not an inventory file"),
			format!("the detail does not say so: {detail}"),
		);
		failures.check(
			scenario,
			detail.contains("no `all` group"),
			format!("the detail does not name the missing group: {detail}"),
		);

		let scenario = "secrets by name, with no catalogue";
		let secrets_file = "all:
  vars:
    salt: pepper
    api_token: abc
    plain: yes
    canopy_intent: configure
    tamanu_clone: true
    nested:
      password_hint: no
    tamanu_caddy_tls_dns_iam:
      access_key_id: AKIA
      secret_access_key: xyz
";
		if let Some(by_name) = failures.ok(
			scenario,
			try_preview(&private, json!({ "group": "kamaka", "file": secrets_file })).await,
		) {
			let vars = &by_name["environment"];
			failures.eq(
				scenario,
				"salt is secret",
				&find(vars, "name", "salt")["secret"],
				true,
			);
			failures.eq(
				scenario,
				"salt is unlocked",
				&find(vars, "name", "salt")["locked"],
				false,
			);
			failures.check(
				scenario,
				find(vars, "name", "salt")["value"].is_null(),
				format!("salt's value is masked: {by_name}"),
			);
			failures.eq(
				scenario,
				"api_token is secret",
				&find(vars, "name", "api_token")["secret"],
				true,
			);
			failures.eq(
				scenario,
				"a key inside the value makes it secret",
				&find(vars, "name", "nested")["secret"],
				true,
			);
			failures.eq(
				scenario,
				"the credentials are secret",
				&find(vars, "name", "tamanu_caddy_tls_dns_iam")["secret"],
				true,
			);
			failures.eq(
				scenario,
				"plain is not secret",
				&find(vars, "name", "plain")["secret"],
				false,
			);
			failures.eq(
				scenario,
				"plain's value is shown",
				&find(vars, "name", "plain")["value"],
				true,
			);
			failures.eq(
				scenario,
				"what is left out",
				&by_name["left_out"],
				json!(["canopy_intent", "tamanu_clone"]),
			);
			failures.check(
				scenario,
				!names(vars).contains(&"canopy_intent") && !names(vars).contains(&"tamanu_clone"),
				format!("a run's own variables are not proposed: {by_name}"),
			);
		}

		let scenario = "the operator marks secrets, with no catalogue";
		if let Some(chosen) = failures.ok(
			scenario,
			try_preview(
				&private,
				json!({
					"group": "kamaka", "file": secrets_file,
					"secrets": [
						{ "name": "api_token", "secret": false },
						{ "name": "plain", "secret": true },
					]
				}),
			)
			.await,
		) {
			failures.eq(
				scenario,
				"api_token is unmarked",
				&find(&chosen["environment"], "name", "api_token")["secret"],
				false,
			);
			failures.eq(
				scenario,
				"plain is marked",
				&find(&chosen["environment"], "name", "plain")["secret"],
				true,
			);
		}

		read_catalogue(&mut conn).await;

		let scenario = "the catalogue decides what it describes";
		if let Some(decided) = failures.ok(
			scenario,
			try_preview(
				&private,
				json!({
					"group": "kamaka", "file": secrets_file,
					"secrets": [{ "name": "salt", "secret": false }]
				}),
			)
			.await,
		) {
			let salt = find(&decided["environment"], "name", "salt");
			failures.eq(scenario, "salt is secret", &salt["secret"], true);
			failures.eq(scenario, "salt is locked", &salt["locked"], true);
		}

		let scenario = "each variable is listed where it would be set";
		if let Some(preview) = failures.ok(
			scenario,
			try_preview(&private, json!({ "group": "kamaka", "file": FILE })).await,
		) {
			failures.eq(
				scenario,
				"the environment's variables",
				&json!(names(&preview["environment"])),
				json!([
					"elastic_agent_enrollment_token",
					"salt",
					"tamanu_caddy_tls_dns_iam",
					"tamanu_version",
					"timezone"
				]),
			);
			failures.eq(
				scenario,
				"what is left out",
				&preview["left_out"],
				json!(["canopy_intent", "tamanu_clone"]),
			);
			failures.eq(
				scenario,
				"the clone mismatch",
				&preview["clone_mismatch"],
				json!({ "file": true, "environment": false }),
			);

			let vars = &find(&preview["hosts"], "key", "kamaka-facility-a")["variables"];
			failures.eq(
				scenario,
				"the facility's own variable",
				&find(vars, "name", "tamanu_facility_id")["value"],
				"facility-a",
			);
			failures.eq(
				scenario,
				"a group's variable reaches the machines in it",
				&find(vars, "name", "elastic_agent_enabled")["value"],
				true,
			);
			failures.eq(
				scenario,
				"managed_by_ansible's origin",
				&find(vars, "name", "managed_by_ansible")["origin"],
				"added",
			);
			failures.check(
				scenario,
				!names(vars).contains(&"ansible_host"),
				format!(
					"the key is the machine's tailnet name by its first label, which adds nothing: {vars}"
				),
			);

			let vars = &find(&preview["hosts"], "key", "10.20.30.40:223")["variables"];
			failures.eq(
				scenario,
				"the office's address",
				&find(vars, "name", "ansible_host")["value"],
				"10.20.30.40",
			);
			failures.eq(
				scenario,
				"the address's origin",
				&find(vars, "name", "ansible_host")["origin"],
				"from_host_key",
			);
			failures.eq(
				scenario,
				"the office's port",
				&find(vars, "name", "ansible_port")["value"],
				223,
			);
			failures.eq(
				scenario,
				"the port's origin",
				&find(vars, "name", "ansible_port")["origin"],
				"from_host_key",
			);
		}

		let scenario = "a variable that breaks the rules stops the import";
		let file = "all:
  vars:
    elastic_agent_enabled: maybe
  hosts:
    kamaka-central:
    kamaka-facility-a:
    kamaka-office:
";
		if let Some(preview) = failures.ok(
			scenario,
			try_preview(
				&private,
				json!({
					"group": "kamaka", "file": file,
					"hosts": [{ "host": "kamaka-office", "machine_id": env.office }],
				}),
			)
			.await,
		) {
			failures.eq(scenario, "can_apply", &preview["can_apply"], false);
			let problems =
				find(&preview["environment"], "name", "elastic_agent_enabled")["problems"]
					.as_array()
					.cloned()
					.unwrap_or_default();
			failures.check(
				scenario,
				problems.len() == 1,
				format!("one problem: {preview}"),
			);
			failures.check(
				scenario,
				problems
					.first()
					.and_then(Value::as_str)
					.is_some_and(|problem| problem.contains("elastic_agent_enabled")),
				format!("the problem names the variable: {preview}"),
			);
		}

		failures.assert_none();
	})
	.await
}

/// Applying sets every variable where the preview said, writes a secret to the
/// secret store and not beside its name, appends the notes to the group and to
/// each machine a host's comments are about, and records one entry that holds
/// no secret.
// spec: ANSIMP#applying
#[tokio::test(flavor = "multi_thread")]
async fn applying_sets_the_variables_and_notes_and_records_one_entry() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		conn.batch_execute(&format!(
			"UPDATE server_groups SET notes = 'Existing note.' WHERE id = '{}'",
			env.group
		))
		.await
		.unwrap();
		read_catalogue(&mut conn).await;

		let request = json!({
			"group": "kamaka", "file": FILE,
			"hosts": [{ "host": "10.20.30.40:223", "machine_id": env.office }],
		});
		let previewed = preview(&private, request.clone()).await;
		assert_eq!(previewed["can_apply"], true, "{previewed}");
		let proposed = |key: &str| {
			previewed["hosts"]
				.as_array()
				.unwrap()
				.iter()
				.find(|host| host["key"] == key)
				.unwrap()["notes"]
				.clone()
		};
		assert_eq!(proposed("kamaka-central"), "Central is in the cloud");
		assert_eq!(proposed("10.20.30.40:223"), "Behind the site VPN");
		assert_eq!(proposed("kamaka-facility-a"), "");

		conn.batch_execute(&format!(
			"UPDATE machines SET notes = 'Racked in the server room.' WHERE id = '{}'",
			env.office
		))
		.await
		.unwrap();
		let mut apply = request.clone();
		apply["members"] = previewed["members"].clone();
		apply["host_notes"] = json!({ "10.20.30.40:223": "  Behind the site VPN, ask the site lead  " });
		let response = private.post("/api/ansible/import_apply").json(&apply).await;
		response.assert_status_ok();
		let applied: Value = response.json();
		assert_eq!(applied["machines"], 3);
		assert_eq!(applied["notes_added"], true);
		assert_eq!(applied["machine_notes_added"], 2);

		// A host's comments go on its machine, as the operator left them, after
		// what was there.
		let central = database::machines::Machine::get_by_id(&mut conn, env.central)
			.await
			.unwrap();
		assert_eq!(central.notes, "Central is in the cloud");
		let office = database::machines::Machine::get_by_id(&mut conn, env.office)
			.await
			.unwrap();
		assert_eq!(
			office.notes,
			"Racked in the server room.\n\nBehind the site VPN, ask the site lead"
		);
		let facility = database::machines::Machine::get_by_id(&mut conn, env.facility)
			.await
			.unwrap();
		assert_eq!(facility.notes, "");

		// What a run is served: the environment's variables everywhere, each
		// machine's own, and a secret's value from the store.
		let served = read_served(&private, json!({ "group": "kamaka" })).await;
		assert_eq!(served["vars"]["timezone"], "Pacific/Auckland");
		assert_eq!(served["vars"]["salt"], "pepper");
		assert_eq!(
			served["vars"]["tamanu_caddy_tls_dns_iam"],
			json!({ "access_key_id": "AKIAEXAMPLE", "secret_access_key": "xyz" })
		);
		assert!(served["vars"].get("canopy_intent").is_none());
		assert!(served["vars"].get("tamanu_clone").is_none());
		let hosts = served["hosts"].as_array().unwrap();
		let by_name = |name: &str| hosts.iter().find(|host| host["name"] == name).unwrap();
		assert_eq!(by_name("kamaka-facility-a")["vars"]["tamanu_facility_id"], "facility-a");
		assert_eq!(by_name("kamaka-facility-a")["vars"]["elastic_agent_enabled"], true);
		assert_eq!(by_name("kamaka-office")["vars"]["ansible_host"], "10.20.30.40");
		assert_eq!(by_name("kamaka-office")["vars"]["ansible_port"], 223);
		assert_eq!(by_name("kamaka-central")["vars"]["managed_by_ansible"], true);
		assert_eq!(by_name("kamaka-office")["address"], "10.20.30.40");

		// A secret is held by name only.
		let rows: Value = private
			.post("/api/ansible_variables/for_group")
			.json(&json!({ "server_group_id": env.group }))
			.await
			.json();
		let salt = rows
			.as_array()
			.unwrap()
			.iter()
			.find(|row| row["name"] == "salt")
			.unwrap();
		assert_eq!(salt["is_secret"], true);
		assert!(salt["value"].is_null());
		assert!(!rows.to_string().contains("pepper"));

		// The notes follow what was there, under a heading naming the environment.
		let notes = database::server_groups::ServerGroup::get_by_id(&mut conn, env.group)
			.await
			.unwrap()
			.notes;
		assert_eq!(
			notes,
			"Existing note.\n\n## production\nUse: --key-file /path/to/ssh-key\nSSH key is in the password manager"
		);

		// One entry, naming the environment and each variable with its machine.
		let entries = database::audit::find(
			&mut conn,
			&database::audit::Filter {
				action: Some("ansible_inventory_imported".into()),
				limit: 50,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(entries.len(), 1, "{entries:?}");
		let entry = &entries[0];
		let detail = &entry.detail;
		assert_eq!(detail["notes_added"], true);
		assert_eq!(
			detail["machine_notes"],
			json!(["kamaka-office", "kamaka-central"])
		);
		assert_eq!(detail["rank"], "production");
		let variables = detail["variables"].as_array().unwrap();
		assert!(
			variables
				.iter()
				.any(|v| v["name"] == "salt" && v["secret"] == true && v["machine"].is_null())
		);
		assert!(
			variables
				.iter()
				.any(|v| v["name"] == "tamanu_facility_id" && v["machine"] == "kamaka-facility-a")
		);
		let logged = format!("{entry:?}");
		assert!(!logged.contains("pepper"), "no secret value in the log");
		assert!(!logged.contains("xyz"));
		assert_eq!(entry.status, database::audit::Status::Done);
		assert_eq!(entry.records.len(), 4, "the group and its three machines");
	})
	.await
}

/// An import is offered only to an environment that carries no variables: a
/// machine switched to managed beforehand and a variable on the whole group do
/// not hold it back (an environment variable overriding a group's is named),
/// but a variable on one of its machines does, and it is refused once its
/// machines have changed since the preview.
// spec: ANSIMP
// spec: ANSIMP#preview
// spec: ANSIMP#applying
#[tokio::test(flavor = "multi_thread")]
async fn is_refused_where_variables_are_set_or_the_machines_changed() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let request = json!({
			"group": "kamaka", "file": FILE,
			"hosts": [{ "host": "10.20.30.40:223", "machine_id": env.office }],
		});

		// Whether a machine is managed is a switch, not a variable.
		private
			.post("/api/ansible_variables/set")
			.json(&json!({ "machine_id": env.office, "name": "managed_by_ansible", "value": true }))
			.await
			.assert_status_ok();
		let managed = preview(&private, request.clone()).await;
		assert_eq!(managed["can_apply"], true, "{managed}");

		private
			.post("/api/ansible_variables/set")
			.json(&json!({ "server_group_id": env.group, "name": "timezone", "value": "UTC" }))
			.await
			.assert_status_ok();
		private
			.post("/api/ansible_variables/set")
			.json(&json!({ "server_group_id": env.group, "name": "unrelated", "value": 1 }))
			.await
			.assert_status_ok();
		let previewed = preview(&private, request.clone()).await;
		assert_eq!(previewed["can_apply"], true, "{previewed}");
		assert_eq!(previewed["overrides"], json!(["timezone"]));

		// A machine arrives between the preview and the apply.
		insert_application_on_its_own_machine(
			&mut conn,
			env.group,
			"kamaka-late",
			"tamanu-facility",
			Some("production"),
			None,
		)
		.await;
		let mut apply = request.clone();
		apply["members"] = previewed["members"].clone();
		let stale = private.post("/api/ansible/import_apply").json(&apply).await;
		stale.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(stale.text().contains("changed since the preview"));

		// A variable on one of its machines holds an import back, from the
		// preview as well as the apply.
		private
			.post("/api/ansible_variables/set")
			.json(&json!({ "machine_id": env.office, "name": "timezone", "value": "UTC" }))
			.await
			.assert_status_ok();
		let held = private
			.post("/api/ansible/import_preview")
			.json(&request)
			.await;
		held.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(held.text().contains("already carries variables"));
	})
	.await
}

/// What an environment serves changes under a run that holds it, so nothing is
/// imported into one meanwhile.
// spec: ANSIMP#applying
#[tokio::test(flavor = "multi_thread")]
async fn is_refused_while_a_run_holds_the_environment() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let request = json!({
			"group": "kamaka", "file": FILE,
			"hosts": [{ "host": "10.20.30.40:223", "machine_id": env.office }],
		});
		let previewed = preview(&private, request.clone()).await;
		crate::ansible::take_lease(&private, json!({ "group": "kamaka" })).await;

		let mut apply = request;
		apply["members"] = previewed["members"].clone();
		let refused = private.post("/api/ansible/import_apply").json(&apply).await;
		refused.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(refused.text().contains("a run holds this environment"));
	})
	.await
}
