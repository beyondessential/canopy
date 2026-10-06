//! The `2026-10-05-012620-0000_resolve_abandoned_grain_check_states` migration
//! resolves Canopy's own check-states left on applications by checks that now
//! file at another grain, and retires the broken-thread rows as check-states.
//!
//! Runs the migration's SQL for real against seeded leftovers.

use diesel::sql_types;
use diesel_async::{RunQueryDsl, SimpleAsyncConnection as _};
use uuid::Uuid;

const UP: &str = include_str!(
	"../../../../migrations/2026-10-05-012620-0000_resolve_abandoned_grain_check_states/up.sql"
);

#[derive(diesel::QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(diesel::QueryableByName)]
struct State {
	#[diesel(sql_type = sql_types::Text)]
	r#ref: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	check_name: Option<String>,
	#[diesel(sql_type = sql_types::Bool)]
	resolved: bool,
	#[diesel(sql_type = sql_types::Bool)]
	active: bool,
}

#[derive(diesel::QueryableByName)]
struct Closed {
	#[diesel(sql_type = sql_types::Bool)]
	closed: bool,
}

async fn issue(
	conn: &mut diesel_async::AsyncPgConnection,
	application: Uuid,
	source: &str,
	r#ref: &str,
	check: &str,
	result: &str,
	active: bool,
) -> Uuid {
	let row: RowId = diesel::sql_query(
		"INSERT INTO issues (application_id, source, ref, check_name, observed_result, \
		 effective_result, message, active, first_seen, last_seen) \
		 VALUES ($1, $2, $3, $4, $5, $5, 'seeded', $6, NOW() - interval '40 days', \
		 NOW() - interval '40 days') RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(application)
	.bind::<sql_types::Text, _>(source)
	.bind::<sql_types::Text, _>(r#ref)
	.bind::<sql_types::Text, _>(check)
	.bind::<sql_types::Text, _>(result)
	.bind::<sql_types::Bool, _>(active)
	.get_result(conn)
	.await
	.expect("issue");
	row.id
}

// spec: CHK#each-check-is-held-at-one-grain
#[tokio::test(flavor = "multi_thread")]
async fn canopys_states_at_an_abandoned_grain_are_resolved() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let group: RowId =
			diesel::sql_query("INSERT INTO server_groups (name) VALUES ('g') RETURNING id")
				.get_result(&mut conn)
				.await
				.expect("group");
		let machine: RowId = diesel::sql_query(
			"INSERT INTO machines (group_id, name) VALUES ($1, 'box') RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(group.id)
		.get_result(&mut conn)
		.await
		.expect("machine");
		let application: RowId = diesel::sql_query(
			"INSERT INTO applications (host, type, machine_id) \
			 VALUES ('http://grain.invalid/', 'tamanu-central', $1) RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(machine.id)
		.get_result(&mut conn)
		.await
		.expect("application");
		let app = application.id;

		// Canopy's backup check, filed at the machine now, left warning on
		// the application.
		let staleness = issue(
			&mut conn,
			app,
			"canopy",
			"backup-staleness",
			"backup-staleness",
			"warning",
			true,
		)
		.await;
		// A check Canopy still files on the application stays as it is.
		issue(
			&mut conn,
			app,
			"canopy",
			"certificate-expiry",
			"certificate-expiry",
			"warning",
			true,
		)
		.await;
		// A reported check is the reporter's to recover, whatever its age.
		issue(
			&mut conn,
			app,
			"alertd",
			"health/disk_free",
			"disk_free",
			"passed",
			false,
		)
		.await;
		// A broken thread the earlier merge left unresolved.
		issue(
			&mut conn,
			app,
			"alertd",
			"health-broken/caddy_version",
			"caddy_version",
			"passed",
			false,
		)
		.await;

		// An incident whose only live member is the leftover.
		let incident: RowId = diesel::sql_query(
			"INSERT INTO incidents (server_group_id, rank, opened_at) VALUES ($1, 'production', NOW()) RETURNING id",
		)
		.bind::<sql_types::Uuid, _>(group.id)
		.get_result(&mut conn)
		.await
		.expect("incident");
		diesel::sql_query(
			"INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ($1, $2, NOW())",
		)
		.bind::<sql_types::Uuid, _>(incident.id)
		.bind::<sql_types::Uuid, _>(staleness)
		.execute(&mut conn)
		.await
		.expect("member");

		conn.batch_execute(UP).await.expect("migrate");

		let states: Vec<State> = diesel::sql_query(
			"SELECT ref, check_name, resolved_at IS NOT NULL AS resolved, active \
			 FROM issues WHERE application_id = $1 ORDER BY ref",
		)
		.bind::<sql_types::Uuid, _>(app)
		.load(&mut conn)
		.await
		.expect("states");
		let by_ref: std::collections::HashMap<&str, &State> =
			states.iter().map(|s| (s.r#ref.as_str(), s)).collect();

		let staleness = by_ref["backup-staleness"];
		assert!(
			staleness.resolved && !staleness.active,
			"the leftover is closed"
		);
		let expiry = by_ref["certificate-expiry"];
		assert!(
			!expiry.resolved && expiry.active,
			"a check still filed here is untouched"
		);
		assert!(
			!by_ref["health/disk_free"].resolved,
			"a reported check is untouched"
		);
		let broken = by_ref["health-broken/caddy_version"];
		assert!(broken.resolved, "the broken thread is resolved");
		assert_eq!(
			broken.check_name, None,
			"and no longer reads as a check-state"
		);

		let closed: Closed = diesel::sql_query(
			"SELECT closed_at IS NOT NULL AS closed FROM incidents WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(incident.id)
		.get_result(&mut conn)
		.await
		.expect("incident");
		assert!(
			closed.closed,
			"an incident left with no live member is closed"
		);
	})
	.await
}
