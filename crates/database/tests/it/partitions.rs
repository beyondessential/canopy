//! History-storage runway (spec `HST`): provisioning weekly ranges is
//! idempotent and doesn't lock the history it extends, a week with no range
//! cannot be written at all, and the runway self-alert warns, fails, and
//! recovers.

use commons_types::status::CheckResult;
use database::{
	partitions::{self, FAIL_DAYS, HISTORIES, WARN_DAYS},
	self_alerts::{self, PARTITION_RUNWAY_REF},
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

#[derive(QueryableByName)]
struct RowName {
	#[diesel(sql_type = sql_types::Text)]
	relname: String,
}

async fn insert_server(conn: &mut AsyncPgConnection, host: &str) -> Uuid {
	let machine: RowId = sql_query("INSERT INTO machines (name) VALUES ('box') RETURNING id")
		.get_result(conn)
		.await
		.expect("insert machine");
	let row: RowId =
		sql_query("INSERT INTO applications (type, host, machine_id) VALUES ('tamanu-central', $1, $2) RETURNING id")
			.bind::<sql_types::Text, _>(host)
			.bind::<sql_types::Uuid, _>(machine.id)
			.get_result(conn)
			.await
			.expect("insert server");
	row.id
}

/// Drop every partition of `parent` whose range starts on or after
/// `CURRENT_DATE + from_days`, shrinking the runway to a known band. A large
/// negative `from_days` strips the history of ranges entirely.
async fn drop_partitions_from(conn: &mut AsyncPgConnection, parent: &str, from_days: i32) -> usize {
	let names: Vec<RowName> = sql_query(
		r#"
			SELECT c.relname::text AS relname
			FROM pg_inherits i
			JOIN pg_class c ON c.oid = i.inhrelid
			JOIN pg_class p ON p.oid = i.inhparent
			WHERE p.relname = $1
			  AND SUBSTRING(pg_get_expr(c.relpartbound, c.oid) FROM 'FROM \(''(\d{4}-\d{2}-\d{2})')::date
			      >= CURRENT_DATE + $2
		"#,
	)
	.bind::<sql_types::Text, _>(parent)
	.bind::<sql_types::Integer, _>(from_days)
	.load(conn)
	.await
	.expect("list partitions");

	let dropped = names.len();
	for name in names {
		// The name comes from pg_class, and only ever matches the weekly
		// partition shape.
		sql_query(format!("DROP TABLE {}", name.relname))
			.execute(conn)
			.await
			.expect("drop partition");
	}
	dropped
}

async fn days_remaining(conn: &mut AsyncPgConnection, parent: &str) -> i32 {
	partitions::runway(conn)
		.await
		.expect("runway")
		.into_iter()
		.find(|r| r.parent == parent)
		.unwrap_or_else(|| panic!("{parent} missing from runway"))
		.days_remaining
}

#[tokio::test(flavor = "multi_thread")]
async fn provisioning_is_idempotent_and_reports_runway() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let acted = partitions::ensure_runway(&mut conn, 6)
			.await
			.expect("provision");
		assert!(
			acted.iter().all(|week| !week.failed()),
			"provisioning reported failures: {acted:?}"
		);

		// Second pass over the same weeks does nothing at all.
		let again = partitions::ensure_runway(&mut conn, 6)
			.await
			.expect("provision again");
		assert!(again.is_empty(), "second pass acted on: {again:?}");

		for history in HISTORIES {
			let runway = partitions::runway(&mut conn)
				.await
				.expect("runway")
				.into_iter()
				.find(|r| r.parent == history)
				.unwrap_or_else(|| panic!("{history} missing from runway"));
			assert!(
				runway.days_remaining >= 6 * 7,
				"{history} covered only {} day(s) after asking for 6 weeks",
				runway.days_remaining
			);
			assert!(runway.covered_to.is_some(), "{history} has no bound");
			assert!(
				!runway.short(),
				"{history} reads as short right after a pass"
			);
		}
	})
	.await;
}

/// A history stripped of every range is the worst case, not an absent one: it
/// must still be reported, or the alert would read the emptiest possible state
/// as healthy.
#[tokio::test(flavor = "multi_thread")]
async fn a_history_with_no_ranges_reports_as_out_of_runway() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		assert!(
			drop_partitions_from(&mut conn, "device_connections", -100_000).await > 0,
			"nothing to drop"
		);

		let runway = partitions::runway(&mut conn)
			.await
			.expect("runway")
			.into_iter()
			.find(|r| r.parent == "device_connections")
			.expect("history still reported with no partitions");
		assert_eq!(runway.partitions, 0);
		assert_eq!(runway.covered_to, None);
		assert!(
			runway.critical(),
			"{} day(s) reads as fine",
			runway.days_remaining
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_unprovisioned_week_cannot_be_written() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let server = insert_server(&mut conn, "https://unprovisioned.example.com").await;
		drop_partitions_from(&mut conn, "statuses", 14).await;

		let write = sql_query(
			"INSERT INTO statuses (server_id, created_at, extra) \
			 VALUES ($1, NOW() + INTERVAL '21 days', '{}'::jsonb)",
		)
		.bind::<sql_types::Uuid, _>(server)
		.execute(&mut conn)
		.await;
		assert!(
			write.is_err(),
			"a week with no range accepted a write — the whole point of provisioning ahead"
		);

		partitions::ensure_runway(&mut conn, 4)
			.await
			.expect("provision");

		sql_query(
			"INSERT INTO statuses (server_id, created_at, extra) \
			 VALUES ($1, NOW() + INTERVAL '21 days', '{}'::jsonb)",
		)
		.bind::<sql_types::Uuid, _>(server)
		.execute(&mut conn)
		.await
		.expect("write into a provisioned week");
	})
	.await;
}

/// The reason partitions are attached rather than created in place: `CREATE
/// TABLE ... PARTITION OF` takes ACCESS EXCLUSIVE on the parent, so it waits
/// behind any in-flight write and queues every new reader and writer behind
/// itself. `ATTACH PARTITION` takes SHARE UPDATE EXCLUSIVE, which an INSERT
/// does not conflict with.
///
/// The SQL function caps its own lock wait, so a regression here surfaces as a
/// `failed: canceling statement due to lock timeout` week rather than a hang.
#[tokio::test(flavor = "multi_thread")]
async fn provisioning_does_not_block_ingestion() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		let server = insert_server(&mut conn, "https://concurrent.example.com").await;
		// Leave real work for the provisioning pass to do.
		assert!(drop_partitions_from(&mut conn, "statuses", 7).await > 0);

		// Hold an uncommitted write on the history being extended.
		sql_query("BEGIN").execute(&mut conn).await.expect("begin");
		sql_query("INSERT INTO statuses (server_id, extra) VALUES ($1, '{}'::jsonb)")
			.bind::<sql_types::Uuid, _>(server)
			.execute(&mut conn)
			.await
			.expect("ingest");

		let pool = database::init_to(&url);
		let mut other = pool.get().await.expect("second connection");
		let acted = partitions::ensure_runway(&mut other, 4)
			.await
			.expect("provision alongside an open write");

		assert!(
			acted.iter().any(|week| week.action == "created"),
			"nothing was provisioned, so the test proved nothing: {acted:?}"
		);
		assert!(
			acted.iter().all(|week| !week.failed()),
			"provisioning blocked behind an open write: {acted:?}"
		);

		sql_query("COMMIT")
			.execute(&mut conn)
			.await
			.expect("commit");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn runway_alert_warns_fails_and_recovers() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		partitions::ensure_runway(&mut conn, 4)
			.await
			.expect("provision");

		// Nothing short, so nothing is ever raised.
		assert!(
			self_alerts::sweep_partition_runway(&mut conn)
				.await
				.expect("sweep")
				.is_none()
		);
		assert!(
			self_alerts::current(&mut conn, PARTITION_RUNWAY_REF)
				.await
				.expect("current")
				.is_none(),
			"a healthy runway raised an alert"
		);

		// Trim both histories into the warning band. Weeks start on Mondays, so
		// dropping from a week out leaves between one and two weeks of range
		// whatever day the test runs on.
		for history in HISTORIES {
			drop_partitions_from(&mut conn, history, 7).await;
			let days = days_remaining(&mut conn, history).await;
			assert!(
				(FAIL_DAYS..WARN_DAYS).contains(&days),
				"{history} left with {days} day(s), outside the warning band"
			);
		}

		let warned = self_alerts::sweep_partition_runway(&mut conn)
			.await
			.expect("sweep")
			.expect("alert raised");
		assert!(warned.active);
		assert_eq!(warned.observed_result, Some(CheckResult::Warning));
		assert!(
			warned.message.contains("statuses"),
			"alert doesn't name the short history: {}",
			warned.message
		);

		// Strip one history entirely: past warning, into failure.
		drop_partitions_from(&mut conn, "device_connections", -100_000).await;
		let failed = self_alerts::sweep_partition_runway(&mut conn)
			.await
			.expect("sweep")
			.expect("alert still raised");
		assert!(failed.active);
		assert_eq!(failed.observed_result, Some(CheckResult::Failed));

		// One successful pass is all it takes to clear.
		partitions::ensure_runway(&mut conn, 4)
			.await
			.expect("provision");
		let recovered = self_alerts::sweep_partition_runway(&mut conn)
			.await
			.expect("sweep")
			.expect("recovery filed");
		assert!(!recovered.active, "alert stayed active after provisioning");
	})
	.await;
}

/// Two callers provisioning at once must not collide. The functions this
/// replaced probed for a partition and then created it without a lock or an
/// `IF NOT EXISTS`, so two concurrent invocations — the in-process loop and
/// the external schedule named in the old COMMENTs, or two schedulers —
/// could both pass the check and one would abort on the `CREATE`, rolling
/// back every week it had already provisioned in that call.
///
/// Both callers must come back clean, and every week must end up attached.
#[tokio::test(flavor = "multi_thread")]
async fn concurrent_provisioning_does_not_collide() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		// Leave real work to race over.
		assert!(drop_partitions_from(&mut conn, "statuses", 7).await > 0);
		let before = days_remaining(&mut conn, "statuses").await;

		let pool = database::init_to(&url);
		let mut first = pool.get().await.expect("first connection");
		let mut second = pool.get().await.expect("second connection");

		// Genuinely concurrent: both futures are in flight together, so
		// whichever reaches the existence probe first, the other is inside
		// the same window.
		let (a, b) = tokio::join!(
			partitions::ensure_runway(&mut first, 6),
			partitions::ensure_runway(&mut second, 6),
		);
		let a = a.expect("first caller must not error");
		let b = b.expect("second caller must not error");

		assert!(
			a.iter().chain(b.iter()).all(|week| !week.failed()),
			"a caller reported a failed week: {a:?} / {b:?}",
		);
		assert!(
			a.iter()
				.chain(b.iter())
				.any(|week| week.action == "created"),
			"nothing was provisioned, so the test proved nothing: {a:?} / {b:?}",
		);

		// The point of the whole exercise: the runway really was extended,
		// and no week was left behind by a rolled-back call.
		let after = days_remaining(&mut conn, "statuses").await;
		assert!(
			after > before,
			"runway did not grow ({before} -> {after} days)",
		);
		assert!(
			after >= 6 * 7 - 7,
			"six weeks were asked for, got {after} days"
		);
	})
	.await;
}
