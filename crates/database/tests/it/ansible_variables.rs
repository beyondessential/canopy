//! The scoping rules the table itself enforces: a name is unique within its
//! scope, a row sits at exactly one scope, and a secret carries no value here.
//!
//! spec: ANSVAR#variables

use commons_tests::diesel_async::SimpleAsyncConnection;
use commons_types::server::rank::ServerRank;
use database::ansible_variables::{AnsibleVariable, HeldSecretValue, VariableScope};
use jiff::{SignedDuration, Timestamp};
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

		AnsibleVariable::set(
			&mut conn,
			scope,
			"log_level",
			Some(&json!("info")),
			Some("me"),
		)
		.await
		.expect("first set");
		let replaced = AnsibleVariable::set(
			&mut conn,
			scope,
			"log_level",
			Some(&json!("trace")),
			Some("me"),
		)
		.await
		.expect("second set");
		assert_eq!(replaced.value, Some(json!("trace")));

		let held = AnsibleVariable::list_at(&mut conn, scope)
			.await
			.expect("list");
		assert_eq!(held.len(), 1, "no duplicate row");

		// And the index behind that is the database's, not the code's: a row
		// inserted around `set` is refused too.
		let straight_in = conn
			.batch_execute(&format!(
				"INSERT INTO ansible_variables (server_group_id, rank, name, value)
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
			AnsibleVariable::set(&mut conn, scope, "log_level", Some(&json!("info")), None)
				.await
				.expect("set");
		}

		for scope in scopes {
			let held = AnsibleVariable::list_at(&mut conn, scope)
				.await
				.expect("list");
			assert_eq!(held.len(), 1, "{scope:?}");
			assert_eq!(held[0].scope(), scope, "the row reads back at its scope");
		}

		let under = AnsibleVariable::list_under_group(&mut conn, group)
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
			"NULL, 'production', NULL".to_string(),
			"NULL, NULL, NULL".to_string(),
		] {
			let bad = conn
				.batch_execute(&format!(
					"INSERT INTO ansible_variables (server_group_id, rank, machine_id, name, value)
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
					"INSERT INTO ansible_variables (server_group_id, {columns})"
				))
				.await;
			assert!(bad.is_err(), "admitted {columns}");
		}

		// And the code's own two paths land on the right side of it.
		let scope = VariableScope::Group { group_id: group };
		let secret = AnsibleVariable::set(&mut conn, scope, "salt", None, Some("me"))
			.await
			.expect("set a secret");
		assert!(secret.is_secret);
		assert_eq!(secret.value, None);

		let plain = AnsibleVariable::set(
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
	let mut scopes = vec![
		VariableScope::Group { group_id: group },
		VariableScope::Machine {
			machine_id: machine,
		},
	];
	scopes.extend(ServerRank::ALL.iter().map(|rank| VariableScope::Environment {
		group_id: group,
		rank: *rank,
	}));

	let names: Vec<String> = scopes
		.iter()
		.flat_map(|scope| [scope.secret_name(), scope.held_secret_name()])
		.collect();
	let unique: std::collections::BTreeSet<&String> = names.iter().collect();
	assert_eq!(unique.len(), names.len(), "{names:?}");

	// A DNS label, which is what a kubernetes name is held to.
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
		AnsibleVariable::set(&mut conn, scope, "log_level", Some(&json!("info")), None)
			.await
			.expect("set");

		conn.batch_execute(&format!("DELETE FROM machines WHERE id = '{machine}'"))
			.await
			.expect("delete the machine");

		let left = AnsibleVariable::list_all(&mut conn).await.expect("list");
		assert!(left.is_empty(), "{left:?}");
	})
	.await
}

/// Setting a variable on a machine holds its group's row, which an import into
/// the group holds too, so the import's check that nothing is set and its writes
/// cannot have a variable land between them.
// spec: ANSIMP#applying
#[tokio::test(flavor = "multi_thread")]
async fn a_variable_on_a_machine_holds_its_group_as_an_import_does() {
	use commons_tests::diesel_async::{AsyncConnection, AsyncPgConnection};

	commons_tests::db::TestDb::run(async |mut conn, url| {
		let group = seed_group(&mut conn).await;
		let machine = seed_machine(&mut conn, group).await;
		let mut other = AsyncPgConnection::establish(&url)
			.await
			.expect("second connection");

		conn.batch_execute("BEGIN").await.unwrap();
		VariableScope::Machine {
			machine_id: machine,
		}
		.lock_group(&mut conn)
		.await
		.expect("lock");
		let held = other
			.batch_execute(&format!(
				"SELECT id FROM server_groups WHERE id = '{group}' FOR NO KEY UPDATE NOWAIT"
			))
			.await;
		assert!(held.is_err(), "the group's row is held");
		conn.batch_execute("ROLLBACK").await.unwrap();
	})
	.await
}

/// A secret changing is a secret variable set or a value beginning to be held,
/// and a plain variable is neither.
// spec: ANSVAR#history
#[tokio::test(flavor = "multi_thread")]
async fn notices_when_a_secret_last_changed() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		assert_eq!(
			AnsibleVariable::latest_secret_change(&mut conn)
				.await
				.unwrap(),
			None
		);
		let group = seed_group(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO ansible_variables (server_group_id, name, value, is_secret)
			 VALUES ('{group}', 'timezone', '\"Pacific/Fiji\"', FALSE);"
		))
		.await
		.unwrap();
		assert_eq!(
			AnsibleVariable::latest_secret_change(&mut conn)
				.await
				.unwrap(),
			None,
			"a plain value is not a secret changing"
		);

		let before_held = Timestamp::now();
		HeldSecretValue::hold(
			&mut conn,
			Uuid::new_v4(),
			VariableScope::Group { group_id: group },
			"salt",
		)
		.await
		.unwrap();
		let held = AnsibleVariable::latest_secret_change(&mut conn)
			.await
			.unwrap()
			.expect("a held value is a change");
		assert!(held >= before_held - SignedDuration::from_secs(1));

		conn.batch_execute(&format!(
			"INSERT INTO ansible_variables (server_group_id, rank, name, is_secret)
			 VALUES ('{group}', 'production', 'salt', TRUE);"
		))
		.await
		.unwrap();
		let set = AnsibleVariable::latest_secret_change(&mut conn)
			.await
			.unwrap()
			.expect("a secret variable set is a change");
		assert!(set > held, "a secret variable set is the latest change");
	})
	.await
}
