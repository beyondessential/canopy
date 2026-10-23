//! Reading a document as the Ansible variable catalogue, holding a variable to
//! it, and keeping the last copy that read.
//!
//! spec: ANSVAR#catalogue

use database::ansible_catalogue::{
	Catalogue, CatalogueState, Loaded, NotACatalogue, Problem, ScopeKind,
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
				"base-install.yml": { "intent": "configure", "tags": [] }
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
fn a_variable_the_catalogue_does_not_describe_is_not_held_to_anything() {
	assert!(
		catalogue()
			.violations(
				ScopeKind::Group,
				"brand_new_variable",
				&json!({ "any": [1] })
			)
			.is_empty()
	);
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

#[test]
fn holds_a_variable_to_the_scopes_it_may_be_set_at() {
	let catalogue = catalogue();

	assert!(
		catalogue
			.violations(ScopeKind::Machine, "ansible_host", &json!("a.example.org"))
			.is_empty()
	);
	for scope in [ScopeKind::Group, ScopeKind::Environment] {
		let refused = catalogue.violations(scope, "ansible_host", &json!("a.example.org"));
		assert_eq!(refused.len(), 1);
		assert!(refused[0].contains("only at machine"), "{refused:?}");
	}
}

#[test]
fn a_reserved_variable_cannot_be_set_anywhere() {
	let catalogue = catalogue();
	for scope in [ScopeKind::Group, ScopeKind::Environment, ScopeKind::Machine] {
		assert_eq!(
			catalogue
				.violations(scope, "canopy_intent", &json!("configure"))
				.len(),
			1
		);
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
