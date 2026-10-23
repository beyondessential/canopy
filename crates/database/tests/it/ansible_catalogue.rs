//! Reading a document as the Ansible variable catalogue, holding a variable to
//! it, and keeping the last copy that read.
//!
//! spec: ANSVAR#catalogue

use std::collections::{BTreeMap, BTreeSet};

use database::{
	ansible_catalogue::{
		Catalogue, CatalogueState, Loaded, NotACatalogue, Preset, Problem, ScopeKind,
	},
	ansible_leases::RunIntent,
};
use jiff::Timestamp;
use serde_json::{Value, json};

fn document() -> Value {
	json!({
		"$schema": "https://json-schema.org/draft/2020-12/schema",
		"type": "object",
		"properties": {
			"ansible_host": {
				"type": "string",
				"format": "hostname",
				"x-canopy": { "scope": ["machine"] }
			},
			"ansible_port": { "type": "integer", "minimum": 1, "maximum": 65535, "default": 22 },
			"salt": { "type": "string", "x-canopy": { "secret": true } },
			"canopy_intent": { "type": "string", "x-canopy": { "reserved": true } },
			"tamanu_caddy_tls_dns_iam": {
				"type": "object",
				"required": ["access_key_id", "secret_access_key"],
				"properties": {
					"access_key_id": { "type": "string" },
					"secret_access_key": { "type": "string" }
				},
				"x-canopy": { "secret": true, "applications": ["tamanu-central"] }
			},
			"tamanu_facility_id": { "type": "string", "description": "The facility's id." }
		},
		"required": ["tamanu_facility_id"],
		"x-canopy": {
			"playbooks": {
				"tamanu-install.yml": {
					"description": "Everything a Tamanu server needs.",
					"intent": "configure",
					"preset": "install",
					"applications": ["tamanu-central", "tamanu-facility"],
					"tags": ["app", "base"]
				},
				"base-install.yml": { "intent": "configure", "tags": [] },
				"tamanu-upgrade.yml": {
					"description": "Brings Tamanu to a new version.",
					"intent": "upgrade",
					"preset": "upgrade",
					"applications": ["tamanu-central"],
					"tags": ["upgrade"]
				}
			},
			"tags": { "base": "The arrangement of a host." }
		}
	})
}

fn catalogue() -> Catalogue {
	Catalogue::parse(&document()).expect("reads as a catalogue")
}

#[test]
fn reads_the_variables_playbooks_and_tags_it_describes() {
	let catalogue = catalogue();

	assert_eq!(catalogue.variables().count(), 6);
	assert!(catalogue.is_secret("salt"));
	assert!(catalogue.is_secret("tamanu_caddy_tls_dns_iam"));
	assert!(!catalogue.is_secret("ansible_port"));
	assert!(!catalogue.is_secret("not_described"));
	assert!(catalogue.is_reserved("canopy_intent"));
	assert_eq!(
		catalogue.variable("ansible_port").unwrap().default,
		Some(json!(22))
	);
	assert_eq!(
		catalogue
			.variable("tamanu_facility_id")
			.unwrap()
			.description
			.as_deref(),
		Some("The facility's id.")
	);

	let playbook = catalogue.playbook("tamanu-install.yml").expect("playbook");
	assert_eq!(playbook.tags, ["app", "base"]);
	assert_eq!(
		playbook.applications.as_deref(),
		Some(&["tamanu-central".to_owned(), "tamanu-facility".to_owned()][..])
	);
	assert!(
		catalogue
			.playbook("base-install.yml")
			.unwrap()
			.applications
			.is_none()
	);
	assert_eq!(catalogue.tag_notes()["base"], "The arrangement of a host.");

	let properties = &document()["properties"];
	let expected_variables = json!({
		"ansible_host": {
			"name": "ansible_host",
			"description": null,
			"schema": properties["ansible_host"],
			"default": null,
			"secret": false,
			"scopes": ["machine"],
			"applications": null,
			"reserved": false,
		},
		"ansible_port": {
			"name": "ansible_port",
			"description": null,
			"schema": properties["ansible_port"],
			"default": 22,
			"secret": false,
			"scopes": null,
			"applications": null,
			"reserved": false,
		},
		"salt": {
			"name": "salt",
			"description": null,
			"schema": properties["salt"],
			"default": null,
			"secret": true,
			"scopes": null,
			"applications": null,
			"reserved": false,
		},
		"canopy_intent": {
			"name": "canopy_intent",
			"description": null,
			"schema": properties["canopy_intent"],
			"default": null,
			"secret": false,
			"scopes": null,
			"applications": null,
			"reserved": true,
		},
		"tamanu_caddy_tls_dns_iam": {
			"name": "tamanu_caddy_tls_dns_iam",
			"description": null,
			"schema": properties["tamanu_caddy_tls_dns_iam"],
			"default": null,
			"secret": true,
			"scopes": null,
			"applications": ["tamanu-central"],
			"reserved": false,
		},
		"tamanu_facility_id": {
			"name": "tamanu_facility_id",
			"description": "The facility's id.",
			"schema": properties["tamanu_facility_id"],
			"default": null,
			"secret": false,
			"scopes": null,
			"applications": null,
			"reserved": false,
		},
	});
	let variables: BTreeMap<&str, Value> = catalogue
		.variables()
		.map(|variable| {
			(
				variable.name.as_str(),
				serde_json::to_value(variable).expect("variable serialises"),
			)
		})
		.collect();
	assert_eq!(
		serde_json::to_value(&variables).unwrap(),
		expected_variables,
		"every variable, whole"
	);

	let expected_playbooks = json!({
		"tamanu-install.yml": {
			"name": "tamanu-install.yml",
			"description": "Everything a Tamanu server needs.",
			"intent": "configure",
			"preset": "install",
			"applications": ["tamanu-central", "tamanu-facility"],
			"tags": ["app", "base"],
		},
		"base-install.yml": {
			"name": "base-install.yml",
			"description": null,
			"intent": "configure",
			"preset": null,
			"applications": null,
			"tags": [],
		},
		"tamanu-upgrade.yml": {
			"name": "tamanu-upgrade.yml",
			"description": "Brings Tamanu to a new version.",
			"intent": "upgrade",
			"preset": "upgrade",
			"applications": ["tamanu-central"],
			"tags": ["upgrade"],
		},
	});
	let playbooks: BTreeMap<&str, Value> = catalogue
		.playbooks()
		.iter()
		.map(|playbook| {
			(
				playbook.name.as_str(),
				serde_json::to_value(playbook).expect("playbook serialises"),
			)
		})
		.collect();
	assert_eq!(
		catalogue.playbooks().len(),
		playbooks.len(),
		"no playbook is listed twice"
	);
	assert_eq!(
		serde_json::to_value(&playbooks).unwrap(),
		expected_playbooks,
		"every playbook, whole, with both intents and both presets read"
	);
	let upgrade = catalogue.playbook("tamanu-upgrade.yml").unwrap();
	assert_eq!(upgrade.intent, RunIntent::Upgrade);
	assert_eq!(upgrade.preset, Some(Preset::Upgrade));
}

#[test]
fn refuses_what_is_not_a_catalogue() {
	for (what, document) in [
		("an array", json!([])),
		("no properties", json!({ "type": "object" })),
		(
			"a schema that is itself invalid",
			json!({ "properties": { "a": { "type": "no-such-type" } } }),
		),
		(
			"a scope that is not one",
			json!({ "properties": { "a": { "x-canopy": { "scope": ["planet"] } } } }),
		),
		(
			"a secret flag that is not a boolean",
			json!({ "properties": { "a": { "x-canopy": { "secret": "yes" } } } }),
		),
		(
			"a playbook with no intent",
			json!({ "properties": {}, "x-canopy": { "playbooks": { "p.yml": {} } } }),
		),
		(
			"a preset that is not one",
			json!({ "properties": {}, "x-canopy": {
				"playbooks": { "p.yml": { "intent": "configure", "preset": "nap" } }
			} }),
		),
	] {
		let refused: NotACatalogue = Catalogue::parse(&document).expect_err(what);
		assert!(!refused.0.is_empty(), "{what} is refused with a reason");
	}
}

#[test]
fn holds_a_value_to_its_format() {
	let catalogue = catalogue();

	assert!(
		catalogue
			.violations(
				ScopeKind::Machine,
				"ansible_host",
				&json!("box.example.org")
			)
			.is_empty()
	);
	let refused = catalogue.violations(
		ScopeKind::Machine,
		"ansible_host",
		&json!("not a host name"),
	);
	assert_eq!(
		refused.len(),
		1,
		"a format is checked, not merely read: {refused:?}"
	);
	assert!(refused[0].starts_with("\"ansible_host\": "), "{refused:?}");
}

#[test]
fn a_requirement_that_an_environment_carry_some_variable_does_not_refuse_one() {
	assert!(
		catalogue()
			.violations(ScopeKind::Machine, "ansible_port", &json!(22))
			.is_empty(),
		"`required` at the root describes an environment, not one variable"
	);
}

#[test]
fn names_where_in_a_structured_value_the_fault_is() {
	let catalogue = catalogue();

	let wrong = catalogue.violations(
		ScopeKind::Environment,
		"tamanu_caddy_tls_dns_iam",
		&json!({ "access_key_id": "AKIA", "secret_access_key": 4 }),
	);
	assert_eq!(wrong.len(), 1, "{wrong:?}");
	assert!(wrong[0].contains("/secret_access_key"), "{wrong:?}");
}

/// Where a variable may be set.
enum Placement {
	Everywhere,
	Only(&'static [ScopeKind]),
	Never,
}

/// A value of the variable's own type, so a refusal is the placement's and not
/// the value's.
fn value_for(name: &str) -> Value {
	match name {
		"ansible_host" => json!("a.example.org"),
		"ansible_port" => json!(22),
		"salt" => json!("pepper"),
		"canopy_intent" => json!("configure"),
		"tamanu_caddy_tls_dns_iam" => {
			json!({ "access_key_id": "AKIA", "secret_access_key": "secret" })
		}
		"tamanu_facility_id" => json!("facility-1"),
		_ => json!({ "any": [1] }),
	}
}

/// Every variable the catalogue describes, and one it does not, is held to the
/// scopes it may be set at, at every scope: none for a reserved variable, the
/// ones it names, or every one. A name the catalogue does not describe is not
/// held to anything.
#[test]
fn holds_every_variable_to_the_scopes_it_may_be_set_at() {
	use Placement::{Everywhere, Never, Only};

	const UNDESCRIBED: &str = "brand_new_variable";
	let catalogue = catalogue();
	let table = [
		("ansible_host", Only(&[ScopeKind::Machine])),
		("ansible_port", Everywhere),
		("salt", Everywhere),
		("canopy_intent", Never),
		("tamanu_caddy_tls_dns_iam", Everywhere),
		("tamanu_facility_id", Everywhere),
		(UNDESCRIBED, Everywhere),
	];

	let rows: BTreeSet<&str> = table
		.iter()
		.map(|(name, _)| *name)
		.filter(|name| *name != UNDESCRIBED)
		.collect();
	let described: BTreeSet<&str> = catalogue
		.variables()
		.map(|variable| variable.name.as_str())
		.collect();
	assert_eq!(
		rows, described,
		"a row for every variable the catalogue describes"
	);
	assert!(
		catalogue.variable(UNDESCRIBED).is_none(),
		"the undescribed name is not described"
	);

	for (name, placement) in &table {
		let value = value_for(name);
		for scope in ScopeKind::ALL {
			let refused = catalogue.violations(*scope, name, &value);
			assert_eq!(
				catalogue.placement_violations(*scope, name),
				refused,
				"{name} at {scope}: the placement is what refuses a value of its own type"
			);
			match placement {
				Only(allowed) if !allowed.contains(scope) => {
					assert_eq!(refused.len(), 1, "{name} at {scope}: {refused:?}");
					let at = allowed
						.iter()
						.map(ToString::to_string)
						.collect::<Vec<_>>()
						.join(" or ");
					assert!(
						refused[0].contains(&format!("only at {at}")),
						"{name} at {scope}: {refused:?}"
					);
				}
				Never => {
					assert_eq!(refused.len(), 1, "{name} at {scope}: {refused:?}");
					assert!(
						refused[0].contains("cannot be set"),
						"{name} at {scope}: {refused:?}"
					);
				}
				Only(_) | Everywhere => {
					assert!(refused.is_empty(), "{name} at {scope}: {refused:?}");
				}
			}
		}
	}
}

/// A number is held to the bounds its schema gives, at every scope.
#[test]
fn holds_a_value_to_the_bounds_of_its_schema() {
	let catalogue = catalogue();
	for scope in ScopeKind::ALL {
		for (value, allowed) in [
			(json!(1), true),
			(json!(22), true),
			(json!(65535), true),
			(json!(0), false),
			(json!(65536), false),
			(json!("22"), false),
		] {
			let refused = catalogue.violations(*scope, "ansible_port", &value);
			if allowed {
				assert!(refused.is_empty(), "{value} at {scope}: {refused:?}");
			} else {
				assert_eq!(refused.len(), 1, "{value} at {scope}: {refused:?}");
				assert!(
					refused[0].starts_with("\"ansible_port\": "),
					"{value} at {scope}: {refused:?}"
				);
			}
		}
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn keeps_the_last_copy_that_read_when_a_later_attempt_does_not() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		assert!(CatalogueState::get(&mut conn).await.unwrap().is_none());
		assert!(Loaded::get(&mut conn).await.unwrap().is_none());

		CatalogueState::record_problem(
			&mut conn,
			Problem::Unreachable,
			"connection refused",
			Timestamp::now(),
		)
		.await
		.unwrap();
		let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
		assert_eq!(state.problem, Some(Problem::Unreachable));
		assert!(state.document.is_none());
		assert!(
			Loaded::get(&mut conn).await.unwrap().is_none(),
			"a problem before any good read leaves no catalogue"
		);

		let now = Timestamp::now();
		assert!(
			CatalogueState::record_read(&mut conn, &document(), Some("abc123"), now)
				.await
				.unwrap(),
			"the first copy is a change"
		);
		assert!(
			!CatalogueState::record_read(&mut conn, &document(), Some("abc123"), now)
				.await
				.unwrap(),
			"the same copy again is not"
		);

		CatalogueState::record_problem(&mut conn, Problem::Invalid, "no properties", now)
			.await
			.unwrap();
		let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
		assert_eq!(state.problem, Some(Problem::Invalid));
		assert_eq!(state.problem_detail.as_deref(), Some("no properties"));
		assert_eq!(state.document, Some(document()), "the copy that read stays");
		assert_eq!(state.source_commit.as_deref(), Some("abc123"));

		let loaded = Loaded::get(&mut conn).await.unwrap().expect("loaded");
		assert!(loaded.catalogue.is_secret("salt"));
		assert_eq!(loaded.source_commit.as_deref(), Some("abc123"));

		let mut changed = document();
		changed["properties"]["ansible_port"]["default"] = json!(2200);
		assert!(
			CatalogueState::record_read(&mut conn, &changed, Some("def456"), now)
				.await
				.unwrap()
		);
		let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
		assert_eq!(state.problem, None, "a good read clears the problem");
		let loaded = Loaded::get(&mut conn).await.unwrap().expect("loaded");
		assert_eq!(
			loaded.catalogue.variable("ansible_port").unwrap().default,
			Some(json!(2200))
		);
	})
	.await
}

async fn alert_active(conn: &mut commons_tests::diesel_async::AsyncPgConnection) -> bool {
	database::self_alerts::current(conn, database::self_alerts::ANSIBLE_CATALOGUE_REF)
		.await
		.unwrap()
		.is_some_and(|issue| issue.active)
}

async fn alert_message(conn: &mut commons_tests::diesel_async::AsyncPgConnection) -> String {
	database::self_alerts::current(conn, database::self_alerts::ANSIBLE_CATALOGUE_REF)
		.await
		.unwrap()
		.expect("the alert has been raised")
		.message
}

/// A copy that does not read warns straight away, and a good read clears it.
/// An unreachable repository is not a fault until a day has gone by without a
/// good read, counted from the first try where there never was one.
// spec: SELF
#[tokio::test(flavor = "multi_thread")]
async fn a_copy_that_does_not_read_raises_an_alert_until_one_does() {
	use database::self_alerts::sweep_ansible_catalogue;
	use jiff::SignedDuration;

	commons_tests::db::TestDb::run(async |mut conn, _| {
		let start = Timestamp::now();
		let at = |hours: i64| start + SignedDuration::from_hours(hours);

		assert!(
			sweep_ansible_catalogue(&mut conn, start)
				.await
				.unwrap()
				.is_none(),
			"no state: nothing is raised"
		);
		assert!(
			!alert_active(&mut conn).await,
			"no state: nothing has been tried yet"
		);

		CatalogueState::record_problem(&mut conn, Problem::Unreachable, "timed out", start)
			.await
			.unwrap();
		sweep_ansible_catalogue(&mut conn, at(23)).await.unwrap();
		assert!(
			!alert_active(&mut conn).await,
			"unreachable with no read: within the day"
		);
		sweep_ansible_catalogue(&mut conn, at(25)).await.unwrap();
		assert!(
			alert_active(&mut conn).await,
			"unreachable with no read: a day on from the first try"
		);
		assert!(
			alert_message(&mut conn)
				.await
				.contains("has not been read since"),
			"unreachable with no read: raised as not read"
		);

		CatalogueState::record_problem(&mut conn, Problem::Invalid, "no properties", at(26))
			.await
			.unwrap();
		sweep_ansible_catalogue(&mut conn, at(26)).await.unwrap();
		assert!(alert_active(&mut conn).await, "invalid: active");
		assert!(
			alert_message(&mut conn)
				.await
				.contains("does not read as one"),
			"invalid: raised as not reading, whatever the time"
		);

		CatalogueState::record_read(&mut conn, &document(), None, at(27))
			.await
			.unwrap();
		sweep_ansible_catalogue(&mut conn, at(27)).await.unwrap();
		assert!(!alert_active(&mut conn).await, "a read clears it");

		CatalogueState::record_problem(&mut conn, Problem::Invalid, "no properties", at(28))
			.await
			.unwrap();
		sweep_ansible_catalogue(&mut conn, at(28)).await.unwrap();
		assert!(
			alert_active(&mut conn).await,
			"invalid within a day of a read: warns straight away"
		);

		CatalogueState::record_read(&mut conn, &document(), None, at(29))
			.await
			.unwrap();
		sweep_ansible_catalogue(&mut conn, at(29)).await.unwrap();
		assert!(
			!alert_active(&mut conn).await,
			"a second read clears it again"
		);

		// Read once, then unreachable for more than a day since that read.
		CatalogueState::record_problem(&mut conn, Problem::Unreachable, "timed out", at(60))
			.await
			.unwrap();
		sweep_ansible_catalogue(&mut conn, at(60)).await.unwrap();
		assert!(
			alert_active(&mut conn).await,
			"unreachable for more than a day since the last read"
		);
	})
	.await
}
