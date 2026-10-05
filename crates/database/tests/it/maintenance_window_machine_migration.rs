//! The `2026-08-31-091748-0000_maintenance_windows_take_the_machine` migration
//! moves a window from the application it named onto the box that application
//! runs on. Every pre-split window was over a server that is now an
//! application sitting on exactly one machine, so the backfill is that join.
//!
//! Replays it for real: reverts the migration, seeds windows in the shape they
//! had before it, then re-applies it.

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str = include_str!(
	"../../../../migrations/2026-08-31-091748-0000_maintenance_windows_take_the_machine/up.sql"
);
const DOWN: &str = include_str!(
	"../../../../migrations/2026-08-31-091748-0000_maintenance_windows_take_the_machine/down.sql"
);

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(diesel::QueryableByName)]
struct MachineRef {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Uuid>)]
	machine_id: Option<Uuid>,
}

#[derive(diesel::QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

// spec: MNT#declaring
#[tokio::test(flavor = "multi_thread")]
async fn the_backfill_moves_a_window_onto_its_applications_machine() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let machine: RowId =
			diesel::sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
				.get_result(&mut conn)
				.await
				.expect("machine");
		let application: RowId = diesel::sql_query(
			"INSERT INTO applications (host, type, machine_id) \
			 VALUES ('http://mig.invalid/', 'tamanu-central', $1) RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(machine.id)
		.get_result(&mut conn)
		.await
		.expect("application");
		let group: RowId =
			diesel::sql_query("INSERT INTO server_groups (name) VALUES ('g') RETURNING id")
				.get_result(&mut conn)
				.await
				.expect("group");

		// Back to the pre-split shape, then seed windows as they were written.
		conn.batch_execute(DOWN).await.expect("revert");
		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (server_id, expected_end) \
			 VALUES ('{}', NOW() + INTERVAL '2 hours')",
			application.id
		))
		.await
		.expect("seed a window over the application");
		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (server_group_id, expected_end) \
			 VALUES ('{}', NOW() + INTERVAL '2 hours')",
			group.id
		))
		.await
		.expect("seed a window over the group");

		conn.batch_execute(UP).await.expect("re-apply");

		let moved: MachineRef = diesel::sql_query(
			"SELECT machine_id FROM maintenance_windows WHERE server_group_id IS NULL",
		)
		.get_result(&mut conn)
		.await
		.expect("the moved window");
		assert_eq!(
			moved.machine_id,
			Some(machine.id),
			"the window follows its application to the box it runs on"
		);

		let groups: Count = diesel::sql_query(
			"SELECT COUNT(*) AS n FROM maintenance_windows \
			 WHERE server_group_id IS NOT NULL AND machine_id IS NULL",
		)
		.get_result(&mut conn)
		.await
		.expect("group windows");
		assert_eq!(groups.n, 1, "a group's window is untouched by the move");
	})
	.await;
}
