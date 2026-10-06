//! Only an Error/Critical issue may *open* an incident. A Warning may only
//! *join* one that's already open — and that "already open" read is the whole
//! justification for its join.
//!
//! So the read and the open have to be serialised against a concurrent close.
//! Without a lock covering both, a close landing between them turns "join the
//! open incident" into "open a new one", and a Warning pages the operator.

use std::time::Duration;

use commons_tests::db::TestDb;
use commons_types::status::CheckResult;
use database::diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use database::issues::{CheckStateStamp, Incident, NewEvent};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

const GROUP: Uuid = Uuid::from_u128(0xA1);
const SERVER: Uuid = Uuid::from_u128(0xA2);

async fn seed(conn: &mut AsyncPgConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'Race'); \
		 WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', '{SERVER}', '{GROUP}') RETURNING id) INSERT INTO applications (id, host, name, type, group_id, rank, machine_id) VALUES \
			('{SERVER}', 'https://race.invalid', 'race', 'tamanu-central', '{GROUP}', 'production', '{SERVER}');"
	))
	.await
	.expect("seed");
}

async fn file(conn: &mut AsyncPgConnection, r#ref: &str, effective: CheckResult) {
	let stamp = CheckStateStamp {
		check: r#ref.into(),
		observed: effective,
		effective,
		escalates: false,
		detail: None,
		title: None,
		instanced: None,
	};
	NewEvent {
		source: "test".into(),
		r#ref: r#ref.into(),
		description: None,
		message: "m".into(),
		active: Some(true),
		occurred_at: None,
	}
	.save_with_state(conn, SERVER, None, Some(&stamp), false)
	.await
	.expect("file event");
}

async fn incident_count(conn: &mut AsyncPgConnection) -> i64 {
	#[derive(QueryableByName)]
	struct Count {
		#[diesel(sql_type = sql_types::BigInt)]
		n: i64,
	}
	sql_query("SELECT count(*) AS n FROM incidents WHERE server_group_id = $1")
		.bind::<sql_types::Uuid, _>(GROUP)
		.get_result::<Count>(conn)
		.await
		.expect("count incidents")
		.n
}

/// Drive the interleave the audit describes: a Warning's join decision runs
/// while a close is in flight for the same group.
///
/// The holder transaction takes the group lock the open path takes, so the
/// Warning's filing is pinned *behind* it — no sleeps racing real work. Once
/// the close commits and the lock is released, the Warning must observe the
/// post-close state (no open incident) and decline to open one.
#[tokio::test(flavor = "multi_thread")]
async fn a_warning_does_not_open_an_incident_when_a_close_races_its_join() {
	TestDb::run(|mut conn, url| async move {
		seed(&mut conn).await;

		// An Error issue opens the incident the Warning would otherwise join.
		file(&mut conn, "boom", CheckResult::Failed).await;
		assert_eq!(incident_count(&mut conn).await, 1, "error opened one");
		let incident = Incident::list_for_group(&mut conn, GROUP, false, 10)
			.await
			.expect("list")
			.into_iter()
			.next()
			.expect("open incident");

		// A second connection holds the group lock — the same lock the open
		// path takes — standing in for a close that is mid-transaction.
		let mut holder = AsyncPgConnection::establish(&url)
			.await
			.expect("holder connection");
		holder
			.batch_execute(&format!(
				"BEGIN; SELECT id FROM server_groups WHERE id = '{GROUP}' FOR UPDATE;"
			))
			.await
			.expect("take group lock");

		// The Warning's filing must block on that lock rather than reading
		// through it.
		let mut warner = AsyncPgConnection::establish(&url)
			.await
			.expect("warner connection");
		let warning = tokio::spawn(async move {
			file(&mut warner, "twitchy", CheckResult::Warning).await;
		});
		tokio::time::sleep(Duration::from_millis(300)).await;
		assert!(
			!warning.is_finished(),
			"the join decision must serialise behind the target lock, not read past it",
		);

		// Now let the close land and release the lock.
		holder
			.batch_execute(&format!(
				"UPDATE incidents SET closed_at = NOW() WHERE id = '{}'; COMMIT;",
				incident.id
			))
			.await
			.expect("close and commit");

		tokio::time::timeout(Duration::from_secs(10), warning)
			.await
			.expect("warning filing completes once the lock frees")
			.expect("warning task did not panic");

		assert_eq!(
			incident_count(&mut conn).await,
			1,
			"the warning saw the post-close state and opened nothing; \
			 a second incident here is a Slack page for a Warning",
		);
	})
	.await;
}
