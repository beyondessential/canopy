//! The `2026-09-08-093000-0000_maintenance_window_application` migration lets a
//! window cover one application. Rolling it back keeps those windows: they are
//! operational records, so each is attributed to the box its application runs
//! on and closed, since a machine-only model has nowhere else to put it.
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

const ALREADY_ENDED: &str = "2020-01-01 00:00:00+00";

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
async fn the_revert_keeps_an_application_window_on_its_machine() {
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

		// A window of every kind, plus an application window that is already closed.
		// The machine window is open on the same box the application runs on, so an
		// attribution that left the application's window open would collide with it
		// under the machine-only model's one-open-per-machine index.
		conn.batch_execute(&format!(
			"INSERT INTO maintenance_windows (application_id, expected_end) \
			 VALUES ('{app}', NOW() + INTERVAL '2 hours'); \
			 INSERT INTO maintenance_windows (application_id, expected_end, ended_at, updated_at) \
			 VALUES ('{app}', NOW() + INTERVAL '2 hours', '{ended}', '{ended}'); \
			 INSERT INTO maintenance_windows (machine_id, expected_end) \
			 VALUES ('{machine}', NOW() + INTERVAL '2 hours'); \
			 INSERT INTO maintenance_windows (server_group_id, expected_end) \
			 VALUES ('{group}', NOW() + INTERVAL '2 hours'); \
			 INSERT INTO maintenance_windows (server_group_id, rank, expected_end) \
			 VALUES ('{group}', 'clone', NOW() + INTERVAL '2 hours');",
			app = application.id,
			ended = ALREADY_ENDED,
			machine = machine.id,
			group = group.id
		))
		.await
		.expect("seed a window of every kind");

		conn.batch_execute(DOWN).await.expect("revert");

		let total: Count = diesel::sql_query("SELECT COUNT(*) AS n FROM maintenance_windows")
			.get_result(&mut conn)
			.await
			.expect("windows");
		assert_eq!(
			total.n, 5,
			"an application's window is kept through the revert rather than dropped"
		);

		let on_machine: Count = diesel::sql_query(
			"SELECT COUNT(*) AS n FROM maintenance_windows \
			 WHERE machine_id = $1 AND ended_at IS NOT NULL",
		)
		.bind::<sql_types::Uuid, _>(machine.id)
		.get_result(&mut conn)
		.await
		.expect("attributed windows");
		assert_eq!(
			on_machine.n, 2,
			"both application windows land on the box the application runs on, closed"
		);

		let untouched: Count = diesel::sql_query(
			"SELECT COUNT(*) AS n FROM maintenance_windows \
			 WHERE machine_id = $1 AND ended_at = $2::timestamptz AND updated_at = $2::timestamptz",
		)
		.bind::<sql_types::Uuid, _>(machine.id)
		.bind::<sql_types::Text, _>(ALREADY_ENDED)
		.get_result(&mut conn)
		.await
		.expect("the window that was already closed");
		assert_eq!(
			untouched.n, 1,
			"a window already closed keeps the time it closed at and is not touched again"
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
