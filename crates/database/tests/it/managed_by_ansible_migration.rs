//! The `managed_by_ansible_on_machines` migration carries a group's or an
//! environment's `managed_by_ansible` onto each of its machines that sets none
//! of its own, the environment's over the group's, then drops the wider rows.
// spec: INV#managed-machines

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const MIGRATION_UP: &str = include_str!(
	"../../../../migrations/2026-10-22-041459-0000_managed_by_ansible_on_machines/up.sql"
);

#[derive(diesel::QueryableByName)]
struct Row {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
	machine_id: Option<Uuid>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Jsonb>)]
	value: Option<serde_json::Value>,
}

#[tokio::test(flavor = "multi_thread")]
async fn carries_a_wider_value_onto_the_machines_that_set_none() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group = Uuid::new_v4();
		let [plain, own, clone, archived] = [(); 4].map(|_| Uuid::new_v4());
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group}', 'site');
			 INSERT INTO machines (id, name, group_id, rank) VALUES
			 	('{plain}', 'plain', '{group}', 'production'),
			 	('{own}', 'own', '{group}', 'production'),
			 	('{clone}', 'clone', '{group}', 'clone');
			 INSERT INTO machines (id, name, group_id, rank, deleted_at)
			 	VALUES ('{archived}', 'archived', '{group}', 'production', now());
			 INSERT INTO ansible_variables (server_group_id, rank, machine_id, name, value) VALUES
			 	('{group}', NULL, NULL, 'managed_by_ansible', 'true'),
			 	('{group}', 'production', NULL, 'managed_by_ansible', 'false'),
			 	(NULL, NULL, '{own}', 'managed_by_ansible', 'true'),
			 	('{group}', 'production', NULL, 'timezone', '\"Pacific/Auckland\"');"
		))
		.await
		.unwrap();

		conn.batch_execute(MIGRATION_UP).await.unwrap();

		let rows: Vec<Row> = diesel::sql_query(
			"SELECT machine_id, value FROM ansible_variables WHERE name = 'managed_by_ansible'",
		)
		.load(&mut conn)
		.await
		.unwrap();
		let value_on = |machine: Uuid| {
			rows.iter()
				.find(|row| row.machine_id == Some(machine))
				.and_then(|row| row.value.clone())
		};
		assert_eq!(rows.len(), 3, "the wider rows are gone");
		assert_eq!(
			value_on(plain),
			Some(serde_json::json!(false)),
			"the environment's over the group's"
		);
		assert_eq!(
			value_on(own),
			Some(serde_json::json!(true)),
			"a machine's own stays"
		);
		assert_eq!(
			value_on(clone),
			Some(serde_json::json!(true)),
			"the group's where its environment sets none"
		);
		assert_eq!(value_on(archived), None);

		let others: i64 = diesel::select(diesel::dsl::sql::<sql_types::BigInt>(
			"(SELECT count(*) FROM ansible_variables WHERE name = 'timezone')",
		))
		.get_result(&mut conn)
		.await
		.unwrap();
		assert_eq!(others, 1, "other variables are untouched");
	})
	.await
}
