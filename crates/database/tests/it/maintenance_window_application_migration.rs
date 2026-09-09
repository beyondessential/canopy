//! The `2026-09-08-093000-0000_maintenance_window_application` migration lets a
//! window cover one application. Rolling it back removes those windows: the
//! machine-only model has nowhere to put them, and a row carrying neither a
//! machine nor a group fails the constraint the revert restores.
//!
//! Replays it for real: seeds a window of every kind, reverts, then re-applies.

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str = include_str!(
	"../../../../migrations/2026-09-08-093000-0000_maintenance_window_application/up.sql"
);
const DOWN: &str = include_str!(
	"../../../../migrations/2026-09-08-093000-0000_maintenance_window_application/down.sql"
);

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(diesel::QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

// spec: MNT#declaring
#[tokio::test(flavor = "multi_thread")]
async fn the_revert_drops_an_application_window_and_keeps_the_others() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let machine: RowId = diesel::sql_query("INSERT INTO machines DEFAULT VALUES RETURNING id")
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

		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (application_id, expected_end) \
			 VALUES ('{}', NOW() + INTERVAL '2 hours'); \
			 INSERT INTO maintenance_windows (machine_id, expected_end) \
			 VALUES ('{}', NOW() + INTERVAL '2 hours'); \
			 INSERT INTO maintenance_windows (server_group_id, expected_end) \
			 VALUES ('{}', NOW() + INTERVAL '2 hours'); \
			 INSERT INTO maintenance_windows (server_group_id, rank, expected_end) \
			 VALUES ('{}', 'clone', NOW() + INTERVAL '2 hours');",
			application.id, machine.id, group.id, group.id
		))
		.await
		.expect("seed a window of every kind");

		conn.batch_execute(DOWN).await.expect("revert");

		let total: Count = diesel::sql_query("SELECT COUNT(*) AS n FROM maintenance_windows")
			.get_result(&mut conn)
			.await
			.expect("windows");
		assert_eq!(
			total.n, 3,
			"the application's window is removed rather than ended, which the restored constraint requires"
		);

		let open: Count = diesel::sql_query(
			"SELECT COUNT(*) AS n FROM maintenance_windows WHERE ended_at IS NULL",
		)
		.get_result(&mut conn)
		.await
		.expect("open windows");
		assert_eq!(
			open.n, 3,
			"the machine, group and environment windows come through the revert still open"
		);

		conn.batch_execute(UP).await.expect("re-apply");
	})
	.await;
}
