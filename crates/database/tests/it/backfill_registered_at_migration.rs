//! The `2026-06-03-150906-0000_backfill_registered_at` migration fills
//! `applications.registered_at` for applications enrolled before the column
//! existed (the add_server_archival migration introduced it with no
//! backfill, so live, status-reporting applications showed as "hasn't
//! checked in yet"). Seeds the pre-backfill states and replays the
//! migration's SQL.

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const MIGRATION_UP_HISTORICAL: &str =
	include_str!("../../../../migrations/2026-06-03-150906-0000_backfill_registered_at/up.sql");

/// The migration's own text, retargeted at the schema as it stands now.
///
/// This replays historical SQL against a schema that has moved on twice.
/// `servers` has been renamed to `applications`, and the device has moved off
/// the application onto the machine it runs on, so `s.device_id` becomes a
/// lookup through `s.machine_id`. Editing the migration itself is not an
/// option — it has already run everywhere — so both are applied to the text
/// here instead. `statuses.server_id` is left alone: that column keeps its
/// name.
fn migration_up() -> String {
	MIGRATION_UP_HISTORICAL
		.replace("UPDATE servers s", "UPDATE applications s")
		.replace(
			"s.device_id",
			"(SELECT m.device_id FROM machines m WHERE m.id = s.machine_id)",
		)
}

#[derive(diesel::QueryableByName)]
struct RegisteredRow {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Timestamptz>)]
	registered_at: Option<jiff_diesel::Timestamp>,
}

async fn registered_at(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
) -> Option<jiff::Timestamp> {
	let row: RegisteredRow =
		diesel::sql_query("SELECT registered_at FROM applications WHERE id = $1")
			.bind::<sql_types::Uuid, _>(server_id)
			.get_result(conn)
			.await
			.expect("fetch registered_at");
	row.registered_at.map(Into::into)
}

#[derive(diesel::QueryableByName)]
struct MatchRow {
	#[diesel(sql_type = sql_types::Bool)]
	matches: bool,
}

/// True when the server's `registered_at` equals its earliest status
/// `created_at` (compared in SQL — statuses are seeded relative to
/// NOW(), so there's no literal to compare against on the Rust side).
async fn matches_first_status(conn: &mut diesel_async::AsyncPgConnection, server_id: Uuid) -> bool {
	let row: MatchRow = diesel::sql_query(
		"SELECT s.registered_at = \
			(SELECT MIN(st.created_at) FROM statuses st WHERE st.server_id = s.id) AS matches \
		 FROM applications s WHERE s.id = $1",
	)
	.bind::<sql_types::Uuid, _>(server_id)
	.get_result(conn)
	.await
	.expect("compare registered_at to first status");
	row.matches
}

#[tokio::test(flavor = "multi_thread")]
async fn migration_backfills_enrolled_servers_only() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// The device belongs to the box, and a box carries at most one.
		let device_a = Uuid::new_v4();
		let device_c = Uuid::new_v4();
		let device_e = Uuid::new_v4();
		// Enrolled with device + statuses: backfilled to the first push.
		let with_statuses = Uuid::new_v4();
		// Statuses but the device was since deleted: still backfilled.
		let device_gone = Uuid::new_v4();
		// Device attached but no statuses yet: backfilled (device created_at).
		let device_only = Uuid::new_v4();
		// Never enrolled: stays NULL.
		let unenrolled = Uuid::new_v4();
		// Already stamped organically: untouched.
		let already_set = Uuid::new_v4();

		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES \
				('{device_a}', 'server'), ('{device_c}', 'server'), ('{device_e}', 'server'); \
			 INSERT INTO machines (name, id, device_id) VALUES \
				('box', '{with_statuses}', '{device_a}'), ('box', '{device_gone}', NULL), \
				('box', '{device_only}', '{device_c}'), ('box', '{unenrolled}', NULL), \
				('box', '{already_set}', '{device_e}'); \
			 INSERT INTO applications (id, host, type, registered_at, machine_id) VALUES \
				('{with_statuses}', 'https://a.example.com', 'tamanu-central', NULL, \
				 '{with_statuses}'), \
				('{device_gone}', 'https://b.example.com', 'tamanu-central', NULL, '{device_gone}'), \
				('{device_only}', 'https://c.example.com', 'tamanu-central', NULL, \
				 '{device_only}'), \
				('{unenrolled}', 'https://d.example.com', 'tamanu-central', NULL, '{unenrolled}'), \
				('{already_set}', 'https://e.example.com', 'tamanu-central', \
				 '2026-01-01T00:00:00Z', '{already_set}'); \
			 INSERT INTO statuses (server_id, healthy, health, extra, created_at) VALUES \
				('{with_statuses}', true, '[]'::jsonb, '{{}}'::jsonb, NOW() - interval '3 hours'), \
				('{with_statuses}', true, '[]'::jsonb, '{{}}'::jsonb, NOW() - interval '2 hours'), \
				('{device_gone}', true, '[]'::jsonb, '{{}}'::jsonb, NOW() - interval '1 hour');"
		))
		.await
		.expect("seed");

		conn.batch_execute(&migration_up())
			.await
			.expect("replay migration up.sql");

		assert!(
			matches_first_status(&mut conn, with_statuses).await,
			"earliest status push wins"
		);

		assert!(
			matches_first_status(&mut conn, device_gone).await,
			"statuses alone are enrollment evidence even with the device gone"
		);

		assert!(
			registered_at(&mut conn, device_only).await.is_some(),
			"device attachment alone is enrollment evidence"
		);

		assert_eq!(
			registered_at(&mut conn, unenrolled).await,
			None,
			"never-enrolled applications must keep showing setup instructions"
		);

		let ts = registered_at(&mut conn, already_set).await;
		assert_eq!(
			ts,
			Some("2026-01-01T00:00:00Z".parse::<jiff::Timestamp>().unwrap()),
			"organically-set values are untouched"
		);
	})
	.await
}
