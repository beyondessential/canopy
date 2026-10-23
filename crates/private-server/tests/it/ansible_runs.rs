//! Starting a run: what its lease records, what it is refused for, how its limit
//! is read, and what its first read does.
//!
//! spec: ANSRUN

use commons_tests::{axum_test::TestServer, diesel_async::AsyncPgConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::ansible::{
	bind_device, insert_application_on_its_own_machine, insert_group, take_lease,
};

struct Environment {
	group: Uuid,
	central: Uuid,
	facility_a: Uuid,
	facility_b: Uuid,
}

/// A production environment of a central and two facilities, every machine one a
/// run configures.
async fn environment(conn: &mut AsyncPgConnection, private: &TestServer) -> Environment {
	let group = insert_group(conn, "kamaka").await;
	let mut machines = Vec::new();
	for (name, kind, tailnet) in [
		(
			"kamaka-central",
			"tamanu-central",
			"kamaka-central.tail1234.ts.net",
		),
		(
			"kamaka-facility-a",
			"tamanu-facility",
			"kamaka-facility-a.tail1234.ts.net",
		),
		(
			"kamaka-facility-b",
			"tamanu-facility",
			"kamaka-facility-b.tail1234.ts.net",
		),
	] {
		let (application, machine) = insert_application_on_its_own_machine(
			conn,
			group,
			name,
			kind,
			Some("production"),
			None,
		)
		.await;
		bind_device(conn, application, tailnet).await;
		machines.push(machine);
	}
	for machine in &machines {
		private
			.post("/api/ansible_variables/set")
			.json(&json!({ "machine_id": machine, "name": "managed_by_ansible", "value": true }))
			.await
			.assert_status_ok();
	}
	Environment {
		group,
		central: machines[0],
		facility_a: machines[1],
		facility_b: machines[2],
	}
}

async fn read_catalogue(conn: &mut AsyncPgConnection) {
	database::ansible_catalogue::CatalogueState::record_read(
		conn,
		&json!({
			"$schema": "https://json-schema.org/draft/2020-12/schema",
			"type": "object",
			"properties": {},
			"x-canopy": { "playbooks": {
				"tamanu-install.yml": {
					"intent": "configure",
					"preset": "install",
					"applications": ["tamanu-central", "tamanu-facility"],
					"tags": ["app", "base", "tailscale"]
				},
				"base-install.yml": { "intent": "configure", "tags": [] },
				"tamanu-upgrade.yml": { "intent": "upgrade", "tags": [] }
			} }
		}),
		None,
		jiff::Timestamp::now(),
	)
	.await
	.expect("read a catalogue");
}

async fn take(private: &TestServer, body: Value) -> Value {
	let response = private.post("/api/ansible/take_lease").json(&body).await;
	response.assert_status_ok();
	response.json()
}

async fn refused(private: &TestServer, body: Value) -> String {
	let response = private.post("/api/ansible/take_lease").json(&body).await;
	assert!(
		response.status_code().is_client_error(),
		"{}",
		response.text()
	);
	response.json::<Value>()["detail"]
		.as_str()
		.unwrap_or_default()
		.to_owned()
}

/// A lease keeps the playbook, the limit and what it matched, the tags and the
/// assignments, and the entry that records it names them. A run read under it is
/// served the lease, so it takes the intent, playbook, limit and tags from
/// Canopy rather than from how it was started, and its maintenance covers only
/// the machines the limit matched.
// spec: ANSRUN#audit, INV#what-a-run-is-served, ANSRUN#work-under-way
#[tokio::test(flavor = "multi_thread")]
async fn a_lease_records_the_run_it_was_taken_for() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn, &private).await;
		read_catalogue(&mut conn).await;

		let lease = take(
			&private,
			json!({
				"group": "kamaka", "playbook": "tamanu-install.yml", "limit": "facility",
				"tags": ["app"], "skip_tags": ["tailscale"],
			}),
		)
		.await;
		assert_eq!(lease["playbook"], "tamanu-install.yml");
		assert_eq!(lease["limit_pattern"], "facility");
		let mut limited: Vec<Uuid> = serde_json::from_value(lease["limited_to"].clone()).unwrap();
		limited.sort();
		let mut facilities = vec![env.facility_a, env.facility_b];
		facilities.sort();
		assert_eq!(limited, facilities);
		assert_eq!(lease["tags"], json!(["app"]));
		assert_eq!(lease["skip_tags"], json!(["tailscale"]));
		assert!(
			lease["started_at"].is_null(),
			"pending until a run reads it"
		);

		let entries = database::audit::find(
			&mut conn,
			&database::audit::Filter {
				action: Some("inventory_lease_taken".into()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(entries.len(), 1);
		let detail = &entries[0].detail;
		assert_eq!(detail["playbook"], "tamanu-install.yml");
		assert_eq!(detail["limit"], "facility");
		assert_eq!(detail["tags"], json!(["app"]));
		assert_eq!(detail["skip_tags"], json!(["tailscale"]));
		assert_eq!(
			entries[0]
				.records
				.iter()
				.filter(|record| format!("{:?}", record.kind) == "Machine")
				.count(),
			2,
			"the machines the limit matched"
		);

		let response = private
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": lease["id"] }))
			.await;
		response.assert_status_ok();
		let served = &response.json::<Value>()["lease"];
		assert_eq!(served["id"], lease["id"]);
		assert_eq!(served["intent"], "configure");
		assert_eq!(served["playbook"], "tamanu-install.yml");
		assert_eq!(served["limit_pattern"], "facility");
		assert_eq!(served["tags"], json!(["app"]));
		assert_eq!(served["skip_tags"], json!(["tailscale"]));
		assert!(
			served["started_at"].is_string(),
			"the read started the run: {served}"
		);

		let windows = database::maintenance_windows::MaintenanceWindow::list_open(&mut conn)
			.await
			.unwrap();
		assert_eq!(windows.len(), 1);
		let mut named = windows[0].machine_ids.clone();
		named.sort();
		assert_eq!(named, facilities, "the machines the limit matched");
		assert!(!named.contains(&env.central));
	})
	.await
}

/// A run started other than as its lease says, in intent, playbook, limit or
/// tags, is refused before anything starts: no run is recorded and no
/// maintenance declared.
// spec: ANSRUN#starting-a-run
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_read_started_otherwise_than_the_lease_says() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn, &private).await;
		read_catalogue(&mut conn).await;
		let lease = take(
			&private,
			json!({
				"server_group_id": env.group,
				"playbook": "tamanu-install.yml",
				"limit": "kamaka-central",
				"tags": ["app", "base"],
				"skip_tags": ["tailscale"],
			}),
		)
		.await;
		let read = |claims: Value| {
			let mut args = claims;
			args["lease_id"] = lease["id"].clone();
			private.post("/api/ansible/for_group").json(&args)
		};

		for (claims, says) in [
			(json!({ "intent": "upgrade" }), &["configure", "upgrade"][..]),
			(
				json!({ "playbook": "base-install.yml" }),
				&["base-install.yml"],
			),
			(json!({ "limit": "" }), &["no limit"]),
			(
				json!({ "limit": "kamaka-facility-a" }),
				&["kamaka-facility-a"],
			),
			(json!({ "tags": ["app"] }), &["app"]),
			(json!({ "skip_tags": [] }), &["none"]),
		] {
			let refused = read(claims.clone()).await;
			refused.assert_status(axum::http::StatusCode::CONFLICT);
			let detail = refused.json::<Value>()["detail"]
				.as_str()
				.unwrap()
				.to_owned();
			for says in says {
				assert!(detail.contains(says), "{claims}: {detail}");
			}
		}

		let id: Uuid = lease["id"].as_str().unwrap().parse().unwrap();
		let started = database::ansible_leases::AnsibleLease::get(&mut conn, id)
			.await
			.unwrap()
			.started_at;
		assert_eq!(started, None, "nothing started");
		let runs = database::audit::find(
			&mut conn,
			&database::audit::Filter {
				action: Some("ansible_run_started".into()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert!(runs.is_empty(), "no run recorded");

		// As the lease says, tags in any order and the limit untrimmed.
		read(json!({
			"intent": "configure",
			"playbook": "tamanu-install.yml",
			"limit": " kamaka-central ",
			"tags": ["base", "app"],
			"skip_tags": ["tailscale"],
		}))
		.await
		.assert_status_ok();
	})
	.await
}

/// A playbook the catalogue does not list, one it runs as an upgrade started as
/// a configure to go around the upgrade plan, a tag it does not take, and a
/// limit that matches nothing or that cannot be read, are refused.
// spec: ANSRUN#presentation, ANSRUN#planned-upgrades
#[tokio::test(flavor = "multi_thread")]
async fn refuses_what_could_not_run() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn, &private).await;
		// Before any catalogue is read, no playbook is known, so none can be
		// labelled with an intent that goes around the upgrade plan.
		assert!(
			refused(
				&private,
				json!({ "group": "kamaka", "playbook": "tamanu-upgrade.yml", "intent": "configure" })
			)
			.await
			.contains("read no catalogue")
		);
		read_catalogue(&mut conn).await;

		assert!(
			refused(
				&private,
				json!({ "group": "kamaka", "playbook": "nope.yml" })
			)
			.await
			.contains("lists no playbook")
		);
		assert!(
			refused(
				&private,
				json!({ "group": "kamaka", "playbook": "tamanu-upgrade.yml", "intent": "configure" }),
			)
			.await
			.contains("is run as upgrade")
		);
		assert!(
			refused(
				&private,
				json!({ "group": "kamaka", "playbook": "tamanu-install.yml", "tags": ["unknown"] }),
			)
			.await
			.contains("is not a tag")
		);
		// The catalogue lists every tag a playbook takes, so one it lists none
		// for takes none.
		assert!(
			refused(
				&private,
				json!({ "group": "kamaka", "playbook": "base-install.yml", "tags": ["app"] }),
			)
			.await
			.contains("takes no tags")
		);
		assert!(
			refused(&private, json!({ "group": "kamaka", "limit": "nowhere" }))
				.await
				.contains("matches no machine")
		);
		assert!(
			refused(
				&private,
				json!({ "group": "kamaka", "limit": "~facility-.*" })
			)
			.await
			.contains("cannot evaluate")
		);
	})
	.await
}

/// A machine with nothing on it can be assigned a type the playbook
/// configures, a Tamanu central only where the environment holds none and then
/// at most once, and a machine with something on it cannot be assigned.
// spec: ANSRUN#machines-with-nothing-on-them
#[tokio::test(flavor = "multi_thread")]
async fn assigns_a_type_to_a_machine_with_nothing_on_it() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "fresh").await;
		let mut ids = Vec::new();
		for name in ["fresh-a", "fresh-b"] {
			let id = Uuid::new_v4();
			let device = Uuid::new_v4();
			use commons_tests::diesel_async::SimpleAsyncConnection;
			conn.batch_execute(&format!(
				"INSERT INTO devices (id, role, tailscale_node_name)
				 VALUES ('{device}', 'server', '{name}.tail1234.ts.net');
				 INSERT INTO machines (id, name, group_id, rank, device_id)
				 VALUES ('{id}', '{name}', '{group}', 'production', '{device}')"
			))
			.await
			.unwrap();
			ids.push(id);
		}
		for machine in &ids {
			private
				.post("/api/ansible_variables/set")
				.json(
					&json!({ "machine_id": machine, "name": "managed_by_ansible", "value": true }),
				)
				.await
				.assert_status_ok();
		}
		read_catalogue(&mut conn).await;

		let options: Value = private
			.post("/api/ansible/run_options")
			.json(&json!({ "group": "fresh" }))
			.await
			.json();
		assert_eq!(options["has_central"], false);
		assert_eq!(options["members"].as_array().unwrap().len(), 2);
		assert_eq!(options["members"][0]["application_types"], json!([]));

		let two_centrals = refused(
			&private,
			json!({
				"group": "fresh", "playbook": "tamanu-install.yml",
				"assignments": [
					{ "machine_id": ids[0], "application_type": "tamanu-central" },
					{ "machine_id": ids[1], "application_type": "tamanu-central" },
				],
			}),
		)
		.await;
		assert!(
			two_centrals.contains("at most one Tamanu central"),
			"{two_centrals}"
		);

		let wrong_type = refused(
			&private,
			json!({
				"group": "fresh", "playbook": "tamanu-install.yml",
				"assignments": [{ "machine_id": ids[0], "application_type": "mysql" }],
			}),
		)
		.await;
		assert!(wrong_type.contains("does not configure"), "{wrong_type}");

		let lease = take(
			&private,
			json!({
				"group": "fresh", "playbook": "tamanu-install.yml",
				"assignments": [
					{ "machine_id": ids[0], "application_type": "tamanu-central" },
					{ "machine_id": ids[1], "application_type": "tamanu-facility" },
				],
			}),
		)
		.await;
		let served: Value = {
			let response = private
				.post("/api/ansible/for_group")
				.json(&json!({ "lease_id": lease["id"] }))
				.await;
			response.assert_status_ok();
			response.json()
		};
		let assigned: Vec<(&str, &str)> = served["hosts"]
			.as_array()
			.unwrap()
			.iter()
			.map(|host| {
				(
					host["name"].as_str().unwrap(),
					host["assigned_type"].as_str().unwrap(),
				)
			})
			.collect();
		assert_eq!(
			assigned,
			[
				("fresh-a", "tamanu-central"),
				("fresh-b", "tamanu-facility")
			]
		);
	})
	.await
}

/// A machine with something on it, or an environment that holds a central, is
/// not assigned one.
// spec: ANSRUN#machines-with-nothing-on-them
#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_assignment_that_could_not_hold() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn, &private).await;
		read_catalogue(&mut conn).await;

		let occupied = refused(
			&private,
			json!({
				"group": "kamaka", "playbook": "tamanu-install.yml",
				"assignments": [{ "machine_id": env.facility_a, "application_type": "tamanu-facility" }],
			}),
		)
		.await;
		assert!(
			occupied.contains("already has an application"),
			"{occupied}"
		);

		// A machine with nothing on it cannot be made a second central.
		let bare = Uuid::new_v4();
		let device = Uuid::new_v4();
		use commons_tests::diesel_async::SimpleAsyncConnection;
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role, tailscale_node_name)
			 VALUES ('{device}', 'server', 'kamaka-new.tail1234.ts.net');
			 INSERT INTO machines (id, name, group_id, rank, device_id)
			 VALUES ('{bare}', 'kamaka-new', '{}', 'production', '{device}');
			 INSERT INTO ansible_variables (machine_id, name, value)
			 VALUES ('{bare}', 'managed_by_ansible', 'true')",
			env.group
		))
		.await
		.unwrap();
		let second_central = refused(
			&private,
			json!({
				"group": "kamaka", "playbook": "tamanu-install.yml",
				"assignments": [{ "machine_id": bare, "application_type": "tamanu-central" }],
			}),
		)
		.await;
		assert!(
			second_central.contains("at most one Tamanu central"),
			"{second_central}"
		);
		let _ = env.central;
	})
	.await
}

/// A limit preview names what the pattern matches, the groups and hosts a limit
/// can be picked from, and says why a pattern cannot be read. The pattern
/// language itself is covered by the unit tests of `fns::ansible::limit`.
// spec: ANSRUN#presentation
#[tokio::test(flavor = "multi_thread")]
async fn previews_the_machines_a_limit_matches() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn, &private).await;
		let preview = async |limit: &str| -> Value {
			private
				.post("/api/ansible/limit_preview")
				.json(&json!({ "group": "kamaka", "limit": limit }))
				.await
				.json()
		};

		let matched = preview("facility:!kamaka-facility-b").await;
		let names: Vec<&str> = matched["matches"]
			.as_array()
			.unwrap()
			.iter()
			.map(|m| m["name"].as_str().unwrap())
			.collect();
		assert_eq!(names, ["kamaka-facility-a"]);
		assert_eq!(matched["groups"], json!(["central", "facility", "tamanu"]));
		assert_eq!(
			matched["hosts"],
			json!(["kamaka-central", "kamaka-facility-a", "kamaka-facility-b"])
		);

		let unreadable = preview("~kamaka").await;
		assert!(
			unreadable["error"]
				.as_str()
				.unwrap()
				.contains("cannot evaluate")
		);
	})
	.await
}

/// Only a machine a run configures is offered and matched: the plugin builds an
/// inventory of the machines that carry `managed_by_ansible`.
// spec: INV#reaching-a-machine
#[tokio::test(flavor = "multi_thread")]
async fn offers_only_the_machines_a_run_configures() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn, &private).await;
		private
			.post("/api/ansible_variables/set")
			.json(
				&json!({ "machine_id": env.facility_b, "name": "managed_by_ansible", "value": false }),
			)
			.await
			.assert_status_ok();

		let options: Value = private
			.post("/api/ansible/run_options")
			.json(&json!({ "group": "kamaka" }))
			.await
			.json();
		let names: Vec<&str> = options["members"]
			.as_array()
			.unwrap()
			.iter()
			.map(|m| m["name"].as_str().unwrap())
			.collect();
		assert_eq!(names, ["kamaka-central", "kamaka-facility-a"]);
		assert_eq!(options["has_central"], true);
	})
	.await
}

/// A run's first read starts it: the lease is stamped, maintenance is declared
/// over what the run acts on for the holder, and the entry says a run began.
/// The reads after it change nothing.
// spec: ANSRUN#work-under-way
#[tokio::test(flavor = "multi_thread")]
async fn the_first_read_starts_the_run_and_declares_its_maintenance() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let env = environment(&mut conn, &private).await;
		let lease = take_lease(&private, json!({ "group": "kamaka" })).await;

		let read = async || {
			let response = private
				.post("/api/ansible/for_group")
				.json(&json!({ "lease_id": lease }))
				.await;
			response.assert_status_ok();
		};
		read().await;

		let state: Value = private
			.post("/api/ansible/run_state")
			.json(&json!({ "group": "kamaka" }))
			.await
			.json();
		assert!(!state["lease"]["started_at"].is_null(), "{state}");
		assert_eq!(
			state["lease_is_yours"], true,
			"the holder is not offered taking over their own lease"
		);
		let window = &state["window"];
		assert_eq!(window["lease_id"], lease.to_string());
		assert_eq!(window["server_group_id"], env.group.to_string());
		assert_eq!(window["rank"], "production");
		assert_eq!(window["note"], "configure");
		assert_eq!(window["declared_by"], crate::ansible::ME);

		read().await;
		let windows = database::maintenance_windows::MaintenanceWindow::list_open(&mut conn)
			.await
			.unwrap();
		assert_eq!(windows.len(), 1, "a second read declares nothing more");

		let started = database::audit::find(
			&mut conn,
			&database::audit::Filter {
				action: Some("ansible_run_started".into()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(started.len(), 1, "the run starts once");
		assert_eq!(started[0].status, database::audit::Status::Done);
		let reads = database::audit::find(
			&mut conn,
			&database::audit::Filter {
				action: Some("inventory_read".into()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(reads.len(), 2, "every read is recorded, as routine");

		// Releasing the lease ends the window.
		private
			.post("/api/ansible/release_lease")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status_ok();
		assert!(
			database::maintenance_windows::MaintenanceWindow::list_open(&mut conn)
				.await
				.unwrap()
				.is_empty()
		);
		let released = database::audit::find(
			&mut conn,
			&database::audit::Filter {
				action: Some("inventory_lease_released".into()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(released[0].detail["reason"], "released");
	})
	.await
}

/// A run names the machines it acts on among themselves, as the limit preview
/// does, so a machine it leaves out cannot change what one it acts on is called.
// spec: INV#reaching-a-machine
#[tokio::test(flavor = "multi_thread")]
async fn names_what_a_run_acts_on_as_the_limit_preview_does() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		for (name, tailnet) in [
			("kamaka managed", "box.east.tail1234.ts.net"),
			("kamaka other", "box.west.tail1234.ts.net"),
		] {
			let (application, machine) = insert_application_on_its_own_machine(
				&mut conn,
				group,
				name,
				"tamanu-facility",
				Some("production"),
				None,
			)
			.await;
			bind_device(&mut conn, application, tailnet).await;
			private
				.post("/api/ansible_variables/set")
				.json(&json!({
					"machine_id": machine, "name": "managed_by_ansible",
					"value": name == "kamaka managed",
				}))
				.await
				.assert_status_ok();
		}

		let preview: Value = private
			.post("/api/ansible/limit_preview")
			.json(&json!({ "group": "kamaka", "limit": "all" }))
			.await
			.json();
		assert_eq!(preview["hosts"], json!(["box"]));

		let lease = take_lease(&private, json!({ "group": "kamaka" })).await;
		let served: Value = private
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": lease }))
			.await
			.json();
		let managed = served["hosts"]
			.as_array()
			.unwrap()
			.iter()
			.find(|host| host["name"] == "kamaka managed")
			.unwrap();
		assert_eq!(managed["host_name"], "box", "what the preview called it");
	})
	.await
}

/// A run started for a playbook over an environment where nothing is managed
/// would hold it shut and act on nothing.
// spec: ANSRUN#presentation
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_run_that_would_act_on_nothing() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "bare").await;
		insert_application_on_its_own_machine(
			&mut conn,
			group,
			"bare-central",
			"tamanu-central",
			Some("production"),
			Some("https://bare.example.org"),
		)
		.await;
		read_catalogue(&mut conn).await;

		let refusal = refused(
			&private,
			json!({ "group": "bare", "playbook": "base-install.yml" }),
		)
		.await;
		assert!(refusal.contains("would act on nothing"), "{refusal}");
		// A lease the plugin takes for itself is left to the plugin to refuse.
		take_lease(&private, json!({ "group": "bare" })).await;
	})
	.await
}

/// Giving up on a run releases its lease only while no run has read it, so a run
/// that started in the meantime keeps the lease and the maintenance it declared.
// spec: ANSRUN#starting-a-run
#[tokio::test(flavor = "multi_thread")]
async fn giving_up_releases_only_a_lease_no_run_has_read() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		environment(&mut conn, &private).await;
		let give_up = async |lease: &Uuid| {
			private
				.post("/api/ansible/release_lease")
				.json(&json!({ "lease_id": lease, "unless_started": true }))
				.await
		};

		let unread = take_lease(&private, json!({ "group": "kamaka" })).await;
		give_up(&unread).await.assert_status_ok();

		let read = take_lease(&private, json!({ "group": "kamaka" })).await;
		private
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": read }))
			.await
			.assert_status_ok();
		give_up(&read)
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
		assert_eq!(
			database::maintenance_windows::MaintenanceWindow::list_open(&mut conn)
				.await
				.unwrap()
				.len(),
			1,
			"the run's maintenance stands"
		);
	})
	.await
}

/// A window another operator holds over a machine at the rank that no run can
/// reach yet refuses the take, as it would refuse the maintenance the run
/// declares on its first read.
// spec: ANSRUN#work-under-way
#[tokio::test(flavor = "multi_thread")]
async fn a_window_over_a_machine_no_run_reaches_yet_refuses_the_take() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		use commons_tests::diesel_async::SimpleAsyncConnection;
		let env = environment(&mut conn, &private).await;
		let spare = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id, group_id, rank)
			 VALUES ('kamaka-spare', '{spare}', '{}', 'production');
			 INSERT INTO maintenance_windows (machine_id, expected_end, declared_by)
			 VALUES ('{spare}', NOW() + INTERVAL '2 hours', 'someone.else@bes.au')",
			env.group
		))
		.await
		.unwrap();

		assert!(
			refused(&private, json!({ "group": "kamaka" }))
				.await
				.contains("someone.else@bes.au")
		);
		let state: Value = private
			.post("/api/ansible/run_state")
			.json(&json!({ "group": "kamaka" }))
			.await
			.json();
		assert_eq!(
			state["refuses"], true,
			"the page shows what refuses the take"
		);
		assert_eq!(state["lease_is_yours"], false);
		assert_eq!(state["window"]["declared_by"], "someone.else@bes.au");
	})
	.await
}
