//! The `2026-10-05-092449-0000_application_silences_of_box_checks_take_the_machine`
//! migration carries a silence of a box's check from the application it was
//! set on to the box.
//!
//! A check filed against an application is in that application type's
//! namespace, so a silence held at an application in the machine namespace can
//! match nothing. Such silences date from before the machine grain, when the
//! box's checks were filed against the application on it.
//!
//! Runs the migration's SQL for real. It is additive against the current
//! schema, so the pre-state is simply an application-scoped silence in the
//! machine namespace.

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str = include_str!(
	"../../../../migrations/2026-10-05-092449-0000_application_silences_of_box_checks_take_the_machine/up.sql"
);

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(diesel::QueryableByName, Debug, PartialEq)]
struct Silence {
	#[diesel(sql_type = sql_types::Text)]
	check_name: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	created_by: Option<String>,
}

async fn machine(conn: &mut diesel_async::AsyncPgConnection) -> Uuid {
	let row: RowId = diesel::sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result(conn)
		.await
		.expect("machine");
	row.id
}

async fn application(conn: &mut diesel_async::AsyncPgConnection, machine_id: Uuid) -> Uuid {
	let row: RowId = diesel::sql_query(
		"INSERT INTO applications (host, type, machine_id) \
		 VALUES ('http://box-silence.invalid/', 'tamanu-central', $1) RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.get_result(conn)
	.await
	.expect("application");
	row.id
}

/// The application's own namespace, for a silence of a check it reports.
const CENTRAL: (&str, Option<&str>) = ("application", Some("tamanu-central"));
/// The box's namespace.
const BOX: (&str, Option<&str>) = ("machine", None);

/// A silence at `(column, target)` in `(subject, application_type)`.
async fn silence(
	conn: &mut diesel_async::AsyncPgConnection,
	(column, target): (&str, Uuid),
	(subject, application_type): (&str, Option<&str>),
	check: &str,
	by: &str,
	created_at: &str,
) {
	diesel::sql_query(format!(
		"INSERT INTO scoped_check_policies \
		 (source, check_name, {column}, subject, application_type, ceiling, created_by, created_at) \
		 VALUES ('alertd', $1, $2, $3, $4, 'skipped', $5, $6::timestamptz)"
	))
	.bind::<sql_types::Text, _>(check)
	.bind::<sql_types::Uuid, _>(target)
	.bind::<sql_types::Text, _>(subject)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(application_type)
	.bind::<sql_types::Text, _>(by)
	.bind::<sql_types::Text, _>(created_at)
	.execute(conn)
	.await
	.expect("silence");
}

async fn silences_at(
	conn: &mut diesel_async::AsyncPgConnection,
	column: &str,
	target: Uuid,
) -> Vec<Silence> {
	diesel::sql_query(format!(
		"SELECT check_name, created_by FROM scoped_check_policies \
		 WHERE {column} = $1 ORDER BY check_name"
	))
	.bind::<sql_types::Uuid, _>(target)
	.load(conn)
	.await
	.expect("read silences")
}

// spec: CHK#names
#[tokio::test(flavor = "multi_thread")]
async fn an_applications_silence_of_a_box_check_moves_to_the_box() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let m = machine(&mut conn).await;
		let app = application(&mut conn, m).await;
		for check in ["disk_free", "time_sync"] {
			silence(
				&mut conn,
				("application_id", app),
				BOX,
				check,
				"alice",
				"2026-05-25T00:00:00Z",
			)
			.await;
		}
		// The application's own check stays where it is.
		silence(
			&mut conn,
			("application_id", app),
			CENTRAL,
			"db",
			"alice",
			"2026-05-25T00:00:00Z",
		)
		.await;

		conn.batch_execute(UP).await.expect("apply");

		assert_eq!(
			silences_at(&mut conn, "machine_id", m).await,
			vec![
				Silence {
					check_name: "disk_free".into(),
					created_by: Some("alice".into()),
				},
				Silence {
					check_name: "time_sync".into(),
					created_by: Some("alice".into()),
				},
			],
		);
		assert_eq!(
			silences_at(&mut conn, "application_id", app).await,
			vec![Silence {
				check_name: "db".into(),
				created_by: Some("alice".into()),
			}],
		);
	})
	.await
}

/// A box already silenced keeps its own silence, and two workloads on one box
/// carrying the same silence become one, the earliest.
// spec: CHK#names
#[tokio::test(flavor = "multi_thread")]
async fn a_box_ends_up_with_one_silence_per_check() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let shared = machine(&mut conn).await;
		let first = application(&mut conn, shared).await;
		let second = application(&mut conn, shared).await;
		silence(
			&mut conn,
			("application_id", first),
			BOX,
			"disk_free",
			"bob",
			"2026-06-02T00:00:00Z",
		)
		.await;
		silence(
			&mut conn,
			("application_id", second),
			BOX,
			"disk_free",
			"alice",
			"2026-06-01T00:00:00Z",
		)
		.await;

		let held = machine(&mut conn).await;
		let on_held = application(&mut conn, held).await;
		silence(
			&mut conn,
			("machine_id", held),
			BOX,
			"load",
			"carol",
			"2026-09-01T00:00:00Z",
		)
		.await;
		silence(
			&mut conn,
			("application_id", on_held),
			BOX,
			"load",
			"alice",
			"2026-05-01T00:00:00Z",
		)
		.await;

		conn.batch_execute(UP).await.expect("apply");

		assert_eq!(
			silences_at(&mut conn, "machine_id", shared).await,
			vec![Silence {
				check_name: "disk_free".into(),
				created_by: Some("alice".into()),
			}],
		);
		assert_eq!(
			silences_at(&mut conn, "machine_id", held).await,
			vec![Silence {
				check_name: "load".into(),
				created_by: Some("carol".into()),
			}],
		);
		for app in [first, second, on_held] {
			assert_eq!(silences_at(&mut conn, "application_id", app).await, vec![]);
		}
	})
	.await
}
