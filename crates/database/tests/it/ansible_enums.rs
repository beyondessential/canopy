//! The string forms of the enums the Ansible feature stores and serves: every
//! variant parses back from what it displays and serialises as the same
//! string, and the two held behind a SQL CHECK constraint are stored and read
//! back through the real entry points. These iterate each enum's `ALL`, so a
//! new variant fails here until it is handled.
//!
//! spec: ANSVAR#catalogue, ANSRUN#audit

use std::collections::BTreeMap;

use commons_tests::diesel_async::{RunQueryDsl, SimpleAsyncConnection};
use commons_types::server::rank::ServerRank;
use database::{
	ansible_catalogue::{Catalogue, CatalogueState, Preset, Problem, ScopeKind},
	ansible_leases::{AnsibleLease, ReleaseReason, RunIntent, RunSpec},
};
use diesel::{dsl::sql, sql_types::Nullable, sql_types::Text};
use jiff::Timestamp;
use serde_json::{Value, json};
use uuid::Uuid;

fn serialised<T: serde::Serialize>(value: &T) -> String {
	match serde_json::to_value(value).unwrap() {
		Value::String(name) => name,
		other => panic!("serialises as {other}, not a string"),
	}
}

#[test]
fn server_rank_round_trips_through_its_string() {
	for rank in ServerRank::ALL {
		let shown = rank.to_string();
		assert_eq!(shown.parse::<ServerRank>().unwrap(), *rank, "{shown}");
		assert_eq!(serialised(rank), shown);
	}
}

#[test]
fn run_intent_round_trips_through_its_string() {
	for intent in RunIntent::ALL {
		let shown = intent.to_string();
		assert_eq!(shown.parse::<RunIntent>().unwrap(), *intent, "{shown}");
		assert_eq!(serialised(intent), shown);
	}
}

#[test]
fn release_reason_round_trips_through_its_string() {
	for reason in ReleaseReason::ALL {
		let shown = reason.to_string();
		assert_eq!(shown.parse::<ReleaseReason>().unwrap(), *reason, "{shown}");
		assert_eq!(serialised(reason), shown);
	}
}

#[test]
fn scope_kind_is_read_from_the_name_it_displays() {
	for kind in ScopeKind::ALL {
		let shown = kind.to_string();
		assert_eq!(serialised(kind), shown);

		let catalogue = Catalogue::parse(&json!({
			"properties": { "a": { "x-canopy": { "scope": [shown] } } }
		}))
		.unwrap_or_else(|error| panic!("{shown} is not read as a scope: {error}"));
		assert_eq!(
			catalogue.variable("a").unwrap().scopes.as_deref(),
			Some(&[*kind][..]),
			"{shown}"
		);
	}
}

#[test]
fn preset_is_read_from_the_name_it_serialises_as() {
	for preset in Preset::ALL {
		let name = serialised(preset);
		for intent in RunIntent::ALL {
			let catalogue = Catalogue::parse(&json!({
				"properties": {},
				"x-canopy": { "playbooks": { "p.yml": {
					"intent": intent.to_string(),
					"preset": name,
				} } }
			}))
			.unwrap_or_else(|error| panic!("{name} is not read as a preset: {error}"));
			let playbook = catalogue.playbook("p.yml").unwrap();
			assert_eq!(playbook.preset, Some(*preset), "{name}");
			assert_eq!(playbook.intent, *intent, "{intent}");
		}
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn every_problem_is_stored_and_read_back() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for problem in Problem::ALL {
			CatalogueState::record_problem(&mut conn, *problem, "detail", Timestamp::now())
				.await
				.unwrap_or_else(|error| panic!("{problem:?} is refused: {error}"));

			let state = CatalogueState::get(&mut conn).await.unwrap().unwrap();
			assert_eq!(state.problem, Some(*problem));

			let stored: Option<String> =
				sql::<Nullable<Text>>("SELECT problem FROM ansible_catalogue")
					.get_result(&mut conn)
					.await
					.unwrap();
			assert_eq!(stored, Some(serialised(problem)));
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_release_reason_is_stored_and_read_back() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = Uuid::new_v4();
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka');
			 INSERT INTO machines (id, name, group_id, rank)
			 VALUES ('{machine}', 'kamaka-0', '{group}', 'production')"
		))
		.await
		.unwrap();

		let none = BTreeMap::new();
		let run = RunSpec {
			intent: RunIntent::Configure,
			note: None,
			playbook: None,
			limit_pattern: None,
			limited_to: None,
			tags: &[],
			skip_tags: &[],
			assignments: &none,
		};

		for reason in ReleaseReason::ALL {
			let lease = AnsibleLease::take(
				&mut conn,
				group,
				ServerRank::Production,
				&[machine],
				&run,
				Some("a"),
				false,
			)
			.await
			.unwrap()
			.unwrap_or_else(|window| panic!("refused by a window: {window:?}"));

			assert!(
				AnsibleLease::release(&mut conn, lease.id, Some("a"), *reason)
					.await
					.unwrap_or_else(|error| panic!("{reason} is refused: {error}"))
			);

			let read = AnsibleLease::get(&mut conn, lease.id).await.unwrap();
			assert_eq!(read.release_reason, Some(*reason));

			let stored: Option<String> = sql::<Nullable<Text>>(&format!(
				"SELECT release_reason FROM ansible_leases WHERE id = '{}'",
				lease.id
			))
			.get_result(&mut conn)
			.await
			.unwrap();
			assert_eq!(stored, Some(serialised(reason)));
		}
	})
	.await
}
