//! The scoping rules the table itself enforces: a name is unique within its
//! scope, a row sits at exactly one scope, and a secret carries no value here.
//!
//! spec: INV#inventory-variables

use commons_tests::diesel_async::SimpleAsyncConnection;
use commons_types::server::rank::ServerRank;
use database::inventory_variables::{InventoryVariable, VariableScope};
use serde_json::json;
use uuid::Uuid;

async fn seed_group(conn: &mut commons_tests::diesel_async::AsyncPgConnection) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{id}', 'kamaka')"
	))
	.await
	.expect("seed group");
	id
}

async fn seed_machine(
	conn: &mut commons_tests::diesel_async::AsyncPgConnection,
	group: Uuid,
) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, name, group_id) VALUES ('{id}', 'kamaka-box', '{group}')"
	))
	.await
	.expect("seed machine");
	id
}

/// One name per scope, so setting it again replaces the value rather than
/// leaving two rows for a merge to pick between.
#[tokio::test(flavor = "multi_thread")]
async fn a_name_is_unique_within_its_scope() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = seed_group(&mut conn).await;
		let scope = VariableScope::Environment {
			group_id: group,
			rank: ServerRank::Production,
		};

		InventoryVariable::set(
			&mut conn,
			scope,
			"log_level",
			Some(&json!("info")),
			Some("me"),
		)
		.await
		.expect("first set");
		let replaced = InventoryVariable::set(
			&mut conn,
			scope,
			"log_level",
			Some(&json!("trace")),
			Some("me"),
		)
		.await
		.expect("second set");
		assert_eq!(replaced.value, Some(json!("trace")));

		let held = InventoryVariable::list_at(&mut conn, scope)
			.await
			.expect("list");
		assert_eq!(held.len(), 1, "no duplicate row");

		// And the index behind that is the database's, not the code's: a row
		// inserted around `set` is refused too.
		let straight_in = conn
			.batch_execute(&format!(
				"INSERT INTO inventory_variables (server_group_id, rank, name, value)
				 VALUES ('{group}', 'production', 'log_level', '\"warn\"')"
			))
			.await;
		assert!(straight_in.is_err(), "the unique index admits a second row");
	})
	.await
}

/// The same name at three scopes is three rows, since that is what a merge
/// exists to resolve.
#[tokio::test(flavor = "multi_thread")]
async fn the_same_name_sits_at_every_scope_at_once() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = seed_group(&mut conn).await;
		let machine = seed_machine(&mut conn, group).await;
		let scopes = [
			VariableScope::Group { group_id: group },
			VariableScope::Environment {
				group_id: group,
				rank: ServerRank::Production,
			},
			VariableScope::Machine {
				machine_id: machine,
			},
		];

		for scope in scopes {
			InventoryVariable::set(&mut conn, scope, "log_level", Some(&json!("info")), None)
				.await
				.expect("set");
		}

		for scope in scopes {
			let held = InventoryVariable::list_at(&mut conn, scope)
				.await
				.expect("list");
			assert_eq!(held.len(), 1, "{scope:?}");
			assert_eq!(held[0].scope(), scope, "the row reads back at its scope");
		}

		let under = InventoryVariable::list_under_group(&mut conn, group)
			.await
			.expect("list under group");
		assert_eq!(under.len(), 3, "the group page sees all three");
	})
	.await
}

/// A row at two scopes at once, or at none, would make the merge order
/// undefined.
#[tokio::test(flavor = "multi_thread")]
async fn a_row_sits_at_exactly_one_scope() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = seed_group(&mut conn).await;
		let machine = seed_machine(&mut conn, group).await;

		for values in [
			format!("'{group}', 'production', '{machine}'"),
			format!("NULL, 'production', NULL"),
			format!("NULL, NULL, NULL"),
		] {
			let bad = conn
				.batch_execute(&format!(
					"INSERT INTO inventory_variables (server_group_id, rank, machine_id, name, value)
					 VALUES ({values}, 'log_level', '\"info\"')"
				))
				.await;
			assert!(bad.is_err(), "admitted {values}");
		}
	})
	.await
}

/// A secret's value lives in the secret store, so a row holding both, or a
/// plain variable holding neither, is refused.
#[tokio::test(flavor = "multi_thread")]
async fn a_secret_carries_no_value_and_a_plain_one_must() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = seed_group(&mut conn).await;

		for columns in [
			"name, value, is_secret) VALUES ('salt', '\"pepper\"', TRUE",
			"name, is_secret) VALUES ('salt', FALSE",
		] {
			let bad = conn
				.batch_execute(&format!(
					"INSERT INTO inventory_variables (server_group_id, {columns})"
				))
				.await;
			assert!(bad.is_err(), "admitted {columns}");
		}

		// And the code's own two paths land on the right side of it.
		let scope = VariableScope::Group { group_id: group };
		let secret = InventoryVariable::set(&mut conn, scope, "salt", None, Some("me"))
			.await
			.expect("set a secret");
		assert!(secret.is_secret);
		assert_eq!(secret.value, None);

		let plain = InventoryVariable::set(
			&mut conn,
			scope,
			"salt",
			Some(&json!("visible")),
			Some("me"),
		)
		.await
		.expect("set it plain");
		assert!(!plain.is_secret);
		assert_eq!(plain.value, Some(json!("visible")));
	})
	.await
}

/// Each scope keys its own Secret, so a name at two of them is two values
/// rather than one overwriting the other, and every one of those keys is a
/// name kubernetes will accept.
#[tokio::test(flavor = "multi_thread")]
async fn each_scope_keys_its_own_secret() {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let names = [
		VariableScope::Group { group_id: group }.secret_name(),
		VariableScope::Environment {
			group_id: group,
			rank: ServerRank::Production,
		}
		.secret_name(),
		VariableScope::Environment {
			group_id: group,
			rank: ServerRank::Demo,
		}
		.secret_name(),
		VariableScope::Machine {
			machine_id: machine,
		}
		.secret_name(),
	];
	let unique: std::collections::BTreeSet<&String> = names.iter().collect();
	assert_eq!(unique.len(), names.len(), "{names:?}");

	// A DNS label, which is what a kubernetes name is held to. `production` is
	// the longest rank, so the longest name any scope produces is here.
	for name in &names {
		assert!(name.len() <= 63, "{name} is {} characters", name.len());
		assert!(
			name.chars()
				.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-'),
			"{name}"
		);
	}
}

/// Dropping a machine takes its variables with it, so a name cannot outlive
/// the box it configured.
#[tokio::test(flavor = "multi_thread")]
async fn a_machines_variables_go_with_the_machine() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = seed_group(&mut conn).await;
		let machine = seed_machine(&mut conn, group).await;
		let scope = VariableScope::Machine {
			machine_id: machine,
		};
		InventoryVariable::set(&mut conn, scope, "log_level", Some(&json!("info")), None)
			.await
			.expect("set");

		conn.batch_execute(&format!("DELETE FROM machines WHERE id = '{machine}'"))
			.await
			.expect("delete the machine");

		let left = InventoryVariable::list_all(&mut conn).await.expect("list");
		assert!(left.is_empty(), "{left:?}");
	})
	.await
}
