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
	bind_device, insert_application_on_its_own_machine, insert_group, read_served,
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

async fn preview(private: &TestServer, body: Value) -> Value {
	let response = private
		.post("/api/ansible/import_preview")
		.json(&body)
		.await;
	response.assert_status_ok();
	response.json()
}

fn host<'a>(preview: &'a Value, key: &str) -> &'a Value {
	preview["hosts"]
		.as_array()
		.unwrap()
		.iter()
		.find(|host| host["key"] == key)
		.unwrap_or_else(|| panic!("{key} is in the preview: {preview}"))
}

fn variable<'a>(variables: &'a Value, name: &str) -> &'a Value {
	variables
		.as_array()
		.unwrap()
		.iter()
		.find(|variable| variable["name"] == name)
		.unwrap_or_else(|| panic!("{name} is among the variables: {variables}"))
}

fn names(variables: &Value) -> Vec<&str> {
	variables
		.as_array()
		.unwrap()
		.iter()
		.map(|variable| variable["name"].as_str().unwrap())
		.collect()
}

/// A host is proposed for the one machine it matches by the names it gives, and
/// is left for the operator where it matches none.
// spec: ANSIMP#matching-hosts-to-machines
#[tokio::test(flavor = "multi_thread")]
async fn proposes_the_machine_each_host_matches() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let preview = preview(&private, json!({ "group": "kamaka", "file": FILE })).await;

		let central = host(&preview, "kamaka-central");
		assert_eq!(central["state"], "matched");
		assert_eq!(central["machine_id"], env.central.to_string());

		let facility = host(&preview, "kamaka-facility-a");
		assert_eq!(facility["state"], "matched");
		assert_eq!(facility["machine_id"], env.facility.to_string());

		let office = host(&preview, "10.20.30.40:223");
		assert_eq!(office["state"], "unmatched");
		assert!(office["machine_id"].is_null());

		assert_eq!(preview["can_apply"], false);
		assert!(
			preview["problems"][0]
				.as_str()
				.unwrap()
				.contains("10.20.30.40:223"),
			"{preview}"
		);
		assert_eq!(preview["machines"].as_array().unwrap().len(), 3);
	})
	.await
}

/// A machine need not be on the tailnet to be matched: one with no device
/// matches by its name and its applications' hosts.
// spec: ANSIMP#matching-hosts-to-machines
#[tokio::test(flavor = "multi_thread")]
async fn matches_a_machine_with_no_device_by_its_name_and_its_applications_hosts() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let file = "all:
  hosts:
    kamaka-office:
    stranger:
      public_hostname: office.example.internal
";
		let preview = preview(&private, json!({ "group": "kamaka", "file": file })).await;
		assert_eq!(
			host(&preview, "kamaka-office")["machine_id"],
			env.office.to_string()
		);
		// By a host recorded for an application, which it names, but the
		// machine is also named by the first host, so it is ambiguous.
		assert_eq!(host(&preview, "stranger")["state"], "matched");
		assert_eq!(
			host(&preview, "stranger")["machine_id"],
			env.office.to_string()
		);
		assert_eq!(preview["can_apply"], false, "two hosts, one machine");
		assert!(
			preview["problems"]
				.as_array()
				.unwrap()
				.iter()
				.any(|problem| problem.as_str().unwrap().contains("kamaka-office")),
			"{preview}"
		);
	})
	.await
}

/// Variables on `all` are the environment's, a host's own are its machine's, a
/// group's go to each machine in it, and a run's own are left out.
// spec: ANSIMP#reading-the-file
#[tokio::test(flavor = "multi_thread")]
async fn lists_each_variable_where_it_would_be_set() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn).await;
		read_catalogue(&mut conn).await;
		let preview = preview(&private, json!({ "group": "kamaka", "file": FILE })).await;

		assert_eq!(
			names(&preview["environment"]),
			[
				"elastic_agent_enrollment_token",
				"salt",
				"tamanu_caddy_tls_dns_iam",
				"tamanu_version",
				"timezone"
			]
		);
		assert_eq!(
			preview["left_out"],
			json!(["canopy_intent", "tamanu_clone"])
		);
		assert_eq!(
			preview["clone_mismatch"],
			json!({ "file": true, "environment": false })
		);

		let facility = host(&preview, "kamaka-facility-a");
		let vars = &facility["variables"];
		assert_eq!(variable(vars, "tamanu_facility_id")["value"], "facility-a");
		assert_eq!(
			variable(vars, "elastic_agent_enabled")["value"],
			true,
			"a group's variable reaches the machines in it"
		);
		assert_eq!(variable(vars, "managed_by_ansible")["origin"], "added");
		assert!(
			!names(vars).contains(&"ansible_host"),
			"the key is the machine's tailnet name by its first label, which adds nothing"
		);

		let office = host(&preview, "10.20.30.40:223");
		let vars = &office["variables"];
		assert_eq!(variable(vars, "ansible_host")["value"], "10.20.30.40");
		assert_eq!(variable(vars, "ansible_host")["origin"], "from_host_key");
		assert_eq!(variable(vars, "ansible_port")["value"], 223);
		assert_eq!(variable(vars, "ansible_port")["origin"], "from_host_key");
	})
	.await
}

/// A variable the catalogue calls secret is one, and any other is guessed by
/// its name or the names inside its value, which the operator can change. A
/// secret's value is not shown.
// spec: ANSIMP#preview
#[tokio::test(flavor = "multi_thread")]
async fn proposes_secrets_by_the_catalogue_then_by_name_and_hides_their_values() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn).await;
		let file = "all:
  vars:
    salt: pepper
    api_token: abc
    plain: yes
    nested:
      password_hint: no
    tamanu_caddy_tls_dns_iam:
      access_key_id: AKIA
      secret_access_key: xyz
";
		// No catalogue: all by name.
		let by_name = preview(&private, json!({ "group": "kamaka", "file": file })).await;
		let vars = &by_name["environment"];
		assert_eq!(variable(vars, "salt")["secret"], true);
		assert_eq!(variable(vars, "salt")["locked"], false);
		assert!(variable(vars, "salt")["value"].is_null(), "masked");
		assert_eq!(variable(vars, "api_token")["secret"], true);
		assert_eq!(
			variable(vars, "nested")["secret"],
			true,
			"a key inside the value"
		);
		assert_eq!(variable(vars, "tamanu_caddy_tls_dns_iam")["secret"], true);
		assert_eq!(variable(vars, "plain")["secret"], false);
		assert_eq!(variable(vars, "plain")["value"], true);

		// The operator unmarks one the name guessed, and marks one it did not.
		let chosen = preview(
			&private,
			json!({
				"group": "kamaka", "file": file,
				"secrets": [
					{ "name": "api_token", "secret": false },
					{ "name": "plain", "secret": true },
				]
			}),
		)
		.await;
		assert_eq!(
			variable(&chosen["environment"], "api_token")["secret"],
			false
		);
		assert_eq!(variable(&chosen["environment"], "plain")["secret"], true);

		// With a catalogue, what it describes is decided by it.
		read_catalogue(&mut conn).await;
		let decided = preview(
			&private,
			json!({
				"group": "kamaka", "file": file,
				"secrets": [{ "name": "salt", "secret": false }]
			}),
		)
		.await;
		let salt = variable(&decided["environment"], "salt");
		assert_eq!(salt["secret"], true, "the catalogue decides");
		assert_eq!(salt["locked"], true);
	})
	.await
}

/// A variable breaking the rules for setting one by hand is named, and stops
/// the import.
// spec: ANSIMP#preview
#[tokio::test(flavor = "multi_thread")]
async fn names_a_variable_that_breaks_the_rules_and_refuses_the_import() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		read_catalogue(&mut conn).await;
		let file = "all:
  vars:
    ansible_host: 10.0.0.1
    elastic_agent_enabled: maybe
    'bad name': 1
  hosts:
    kamaka-central:
    kamaka-facility-a:
    kamaka-office:
";
		let preview = preview(
			&private,
			json!({
				"group": "kamaka", "file": file,
				"hosts": [{ "host": "kamaka-office", "machine_id": env.office }],
			}),
		)
		.await;
		assert_eq!(preview["can_apply"], false);
		let problems = |name: &str| -> Vec<&str> {
			variable(&preview["environment"], name)["problems"]
				.as_array()
				.unwrap()
				.iter()
				.map(|problem| problem.as_str().unwrap())
				.collect()
		};
		assert!(
			problems("ansible_host")[0].contains("on a machine"),
			"{preview}"
		);
		assert!(problems("elastic_agent_enabled")[0].contains("elastic_agent_enabled"));
		assert!(problems("bad name")[0].contains("not a usable variable name"));
	})
	.await
}

/// A skipped host carries no variables, and the import applies once every host
/// is matched or skipped.
// spec: ANSIMP#preview
#[tokio::test(flavor = "multi_thread")]
async fn applies_once_every_host_is_matched_or_skipped() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;

		let skipped = preview(
			&private,
			json!({
				"group": "kamaka", "file": FILE,
				"hosts": [{ "host": "10.20.30.40:223", "machine_id": null }],
			}),
		)
		.await;
		assert_eq!(host(&skipped, "10.20.30.40:223")["state"], "skipped");
		assert_eq!(host(&skipped, "10.20.30.40:223")["variables"], json!([]));
		assert_eq!(skipped["can_apply"], true, "{skipped}");

		let assigned = preview(
			&private,
			json!({
				"group": "kamaka", "file": FILE,
				"hosts": [{ "host": "10.20.30.40:223", "machine_id": env.office }],
			}),
		)
		.await;
		assert_eq!(host(&assigned, "10.20.30.40:223")["state"], "assigned");
		assert_eq!(assigned["can_apply"], true, "{assigned}");

		let outside = private
			.post("/api/ansible/import_preview")
			.json(&json!({
				"group": "kamaka", "file": FILE,
				"hosts": [{ "host": "10.20.30.40:223", "machine_id": Uuid::new_v4() }],
			}))
			.await;
		outside.assert_status(axum::http::StatusCode::BAD_REQUEST);
	})
	.await
}

/// Two hosts to one machine, or an import giving two members one address, is
/// refused.
// spec: ANSIMP#preview
#[tokio::test(flavor = "multi_thread")]
async fn refuses_two_hosts_on_one_machine_and_two_machines_at_one_address() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let both = preview(
			&private,
			json!({
				"group": "kamaka", "file": FILE,
				"hosts": [
					{ "host": "kamaka-facility-a", "machine_id": env.central },
					{ "host": "10.20.30.40:223", "machine_id": null },
				],
			}),
		)
		.await;
		assert_eq!(both["can_apply"], false);
		assert!(
			both["problems"][0]
				.as_str()
				.unwrap()
				.contains("are all matched to machine"),
			"{both}"
		);

		let file = "all:
  hosts:
    kamaka-central:
      ansible_host: shared.example.org
    kamaka-facility-a:
      ansible_host: shared.example.org
";
		let shared = preview(&private, json!({ "group": "kamaka", "file": file })).await;
		assert_eq!(shared["can_apply"], false);
		assert!(
			shared["problems"]
				.as_array()
				.unwrap()
				.iter()
				.any(|problem| problem.as_str().unwrap().contains("shared.example.org")),
			"{shared}"
		);
	})
	.await
}

/// A file that is not an inventory, or holds a vault-encrypted value, is
/// refused naming why.
// spec: ANSIMP#reading-the-file
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_file_that_is_not_an_inventory() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn).await;
		let refuse = async |file: &str| -> String {
			let response = private
				.post("/api/ansible/import_preview")
				.json(&json!({ "group": "kamaka", "file": file }))
				.await;
			response.assert_status(axum::http::StatusCode::BAD_REQUEST);
			response.json::<Value>()["detail"]
				.as_str()
				.unwrap()
				.to_owned()
		};

		assert!(refuse("plugin: canopy\n").await.contains("no `all` group"));
		assert!(refuse("all: [\n").await.contains("line"));
		let vault = refuse(
			"all:\n  vars:\n    salt: !vault |\n      $ANSIBLE_VAULT;1.1;AES256\n      61\n",
		)
		.await;
		assert!(vault.contains("salt") && vault.contains("vault"), "{vault}");
	})
	.await
}

/// Applying sets every variable where the preview said, writes a secret to the
/// secret store and not beside its name, appends the notes, and records one
/// entry that holds no secret.
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

		let mut apply = request.clone();
		apply["members"] = previewed["members"].clone();
		let response = private.post("/api/ansible/import_apply").json(&apply).await;
		response.assert_status_ok();
		let applied: Value = response.json();
		assert_eq!(applied["machines"], 3);
		assert_eq!(applied["notes_added"], true);

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

/// An import is offered only to an environment that carries no variables, and
/// is refused once its machines have changed since the preview.
// spec: ANSIMP#applying
#[tokio::test(flavor = "multi_thread")]
async fn is_refused_where_variables_are_set_or_the_machines_changed() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
		let request = json!({
			"group": "kamaka", "file": FILE,
			"hosts": [{ "host": "10.20.30.40:223", "machine_id": env.office }],
		});
		let previewed = preview(&private, request.clone()).await;

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

/// A variable set on the whole group does not hold an import back, and an
/// imported environment variable overriding one is named.
// spec: ANSIMP#preview
#[tokio::test(flavor = "multi_thread")]
async fn a_group_wide_variable_does_not_hold_it_back_and_is_named_where_overridden() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn).await;
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

		let previewed = preview(&private, json!({ "group": "kamaka", "file": FILE })).await;
		assert_eq!(previewed["overrides"], json!(["timezone"]));
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

/// Before Canopy has read a catalogue, the variables a run sets for itself are
/// left out all the same.
// spec: ANSIMP#reading-the-file
#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_the_variables_a_run_sets_before_any_catalogue_is_read() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn).await;
		let preview = preview(&private, json!({ "group": "kamaka", "file": FILE })).await;

		assert_eq!(
			preview["left_out"],
			json!(["canopy_intent", "tamanu_clone"])
		);
		assert!(!names(&preview["environment"]).contains(&"canopy_intent"));
		assert!(!names(&preview["environment"]).contains(&"tamanu_clone"));
	})
	.await
}
