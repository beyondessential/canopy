//! DB-level tests for the backup-detection sweeps
//! (`database::backup::staleness` + `database::backup::reconcile`) and the
//! group-level alert path (`database::issues::raise_group_event`).
//!
//! `classify` is exercised as a pure function (no DB); the sweeps and the
//! group-event path are exercised against a fresh migrated DB.
//!
//! Incident-gate assertions deliberately query `incident_issues` for the
//! *specific* issue's open link (joined by `ref`) rather than "does the group
//! have any open incident". `staleness::sweep` always runs `sweep_maintenance`
//! over every `status='ready'` group, and a freshly-seeded group has no recent
//! maintenance run, so it files a group-level `backup-maintenance-stale` that
//! opens an incident on the group regardless. Asserting on the per-server
//! issue's own membership isolates the `is_monitored` gate from that noise.

use commons_tests::db::TestDb;
use commons_types::backup::{
	BackupType,
	schedule::{EffectiveSchedule, Schedule, ScheduleLayer},
};
use commons_types::status::CheckResult;
use database::backup::refs;
use database::backup::staleness::{ScanRow, StalenessVerdict};
use database::diesel_async::AsyncPgConnection;
use database::{
	BackupConfigStatus, BackupPurpose, BackupRepoObservedSnapshot, BackupRepoSnapshot, BackupRun,
	MachineBackupCapability, NewBackupRun, NewObservedSnapshot, NewServerGroupBackupConfig,
	RunOutcome, ServerGroupBackupConfig, ServerGroupBackupSchedule,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

// --- seeding helpers --------------------------------------------------------

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.get_result::<RowId>(conn)
		.await
		.expect("insert group")
		.id
}

/// A box with one workload on it, returning the *machine*. The staleness scan
/// is rooted at the box, so the monitoring gate under test is the box's.
// spec: BAK
async fn insert_server(conn: &mut AsyncPgConnection, group_id: Uuid, is_monitored: bool) -> Uuid {
	let machine = sql_query(
		"INSERT INTO machines (name, group_id, is_monitored) VALUES ('box', $1, $2) RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Bool, _>(is_monitored)
	.get_result::<RowId>(conn)
	.await
	.expect("insert machine")
	.id;
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	sql_query(
		"INSERT INTO applications (host, type, group_id, rank, is_monitored, machine_id) \
		 VALUES ($1, 'tamanu-central', $2, 'production', $3, $4)",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Bool, _>(is_monitored)
	.bind::<sql_types::Uuid, _>(machine)
	.execute(conn)
	.await
	.expect("insert application");
	machine
}

async fn insert_device(conn: &mut AsyncPgConnection) -> Uuid {
	sql_query("INSERT INTO devices (role) VALUES ('machine') RETURNING id")
		.get_result::<RowId>(conn)
		.await
		.expect("insert device")
		.id
}

fn retention() -> serde_json::Value {
	serde_json::json!({"keep_daily": 7, "keep_weekly": 4, "keep_monthly": 6})
}

fn new_config(group_id: Uuid) -> NewServerGroupBackupConfig {
	NewServerGroupBackupConfig {
		group_id,
		bucket: "bes-kopia-backups-test".into(),
		prefix: String::new(),
		target_role_arn: "arn:aws:iam::123456789012:role/canopy-backups-test".into(),
		maintenance_role_arn: "arn:aws:iam::123456789012:role/canopy-maint-test".into(),
		region: None,
		repo_password_ref: "kopia-repo-pw-test".into(),
		status: BackupConfigStatus::Provisioning,
		mode: commons_types::backup::BackupRepoMode::FromBirth,
		placement: commons_types::backup::BackupPlacement::External,
	}
}

/// Create a `status='ready'` config with `created_at` backdated by `age` so
/// the never/anchor logic can be exercised without sleeping.
async fn insert_ready_config(conn: &mut AsyncPgConnection, group_id: Uuid, age: SignedDuration) {
	ServerGroupBackupConfig::upsert(conn, new_config(group_id))
		.await
		.expect("insert config");
	ServerGroupBackupConfig::set_status(conn, group_id, BackupConfigStatus::Ready)
		.await
		.expect("set ready");
	let secs = age.as_secs();
	sql_query("UPDATE server_group_backup_config SET created_at = NOW() - ($2 || ' seconds')::INTERVAL WHERE group_id = $1")
		.bind::<sql_types::Uuid, _>(group_id)
		.bind::<sql_types::Text, _>(secs.to_string())
		.execute(conn)
		.await
		.expect("backdate config created_at");
}

/// Insert a finished `backup_maintenance_runs` row, backdating both
/// `started_at` and `finished_at` by `finished_age`.
async fn insert_maintenance_run(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	kind: &str,
	outcome: &str,
	error: Option<&str>,
	finished_age: SignedDuration,
) {
	let secs = finished_age.as_secs().to_string();
	sql_query(
		"INSERT INTO backup_maintenance_runs (group_id, kind, started_at, finished_at, outcome, error) \
		 VALUES ($1, $2, NOW() - ($5 || ' seconds')::INTERVAL, NOW() - ($5 || ' seconds')::INTERVAL, $3, $4)",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Text, _>(kind)
	.bind::<sql_types::Text, _>(outcome)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(error)
	.bind::<sql_types::Text, _>(secs)
	.execute(conn)
	.await
	.expect("insert maintenance run");
}

async fn insert_schedule(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	ty: &BackupType,
	interval: SignedDuration,
) {
	ServerGroupBackupSchedule::set_schedule(
		conn,
		group_id,
		ty,
		&Schedule::Interval {
			seconds: interval.as_secs(),
		},
	)
	.await
	.expect("insert schedule");
	ServerGroupBackupSchedule::set_retention(conn, group_id, ty, &retention(), false)
		.await
		.expect("insert retention");
}

async fn enable_capability(conn: &mut AsyncPgConnection, machine_id: Uuid, ty: &BackupType) {
	MachineBackupCapability::register(conn, machine_id, ty, true)
		.await
		.expect("register capability");
	MachineBackupCapability::set_enabled(conn, machine_id, ty, true)
		.await
		.expect("enable capability");
}

/// Record a `purpose='backup'` success and backdate its `reported_at` by `age`.
async fn insert_backup_success_aged(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
	group_id: Uuid,
	machine_id: Uuid,
	ty: &BackupType,
	age: SignedDuration,
) {
	let id = Uuid::new_v4();
	BackupRun::record(
		conn,
		NewBackupRun {
			id,
			device_id,
			group_id,
			machine_id: Some(machine_id),
			r#type: ty.clone(),
			purpose: BackupPurpose::Backup,
			outcome: RunOutcome::Success,
			error: None,
			bytes_uploaded: Some(42),
			snapshot_id: Some("kopia-snap".into()),
			s3_sent_raw_bytes: None,
			s3_sent_payload_bytes: None,
			s3_received_raw_bytes: None,
			s3_received_payload_bytes: None,
			snapshot_taken_at: None,
		},
	)
	.await
	.expect("record backup run");
	let secs = age.as_secs();
	sql_query(
		"UPDATE backup_runs SET reported_at = NOW() - ($2 || ' seconds')::INTERVAL WHERE id = $1",
	)
	.bind::<sql_types::Uuid, _>(id)
	.bind::<sql_types::Text, _>(secs.to_string())
	.execute(conn)
	.await
	.expect("backdate reported_at");
}

/// Record a `purpose='backup'` success `age` ago that names the snapshot it
/// created and says when it froze its data — what the reconcile checks need
/// before they will decide anything.
async fn insert_backup_success_with_snapshot(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
	group_id: Uuid,
	machine_id: Uuid,
	ty: &BackupType,
	age: SignedDuration,
	snapshot_id: &str,
) {
	let id = Uuid::new_v4();
	BackupRun::record(
		conn,
		NewBackupRun {
			id,
			device_id,
			group_id,
			machine_id: Some(machine_id),
			r#type: ty.clone(),
			purpose: BackupPurpose::Backup,
			outcome: RunOutcome::Success,
			error: None,
			bytes_uploaded: Some(42),
			snapshot_id: Some(snapshot_id.into()),
			s3_sent_raw_bytes: None,
			s3_sent_payload_bytes: None,
			s3_received_raw_bytes: None,
			s3_received_payload_bytes: None,
			snapshot_taken_at: Some(Timestamp::now() - age),
		},
	)
	.await
	.expect("record backup run");
	let secs = age.as_secs();
	sql_query(
		"UPDATE backup_runs SET reported_at = NOW() - ($2 || ' seconds')::INTERVAL WHERE id = $1",
	)
	.bind::<sql_types::Uuid, _>(id)
	.bind::<sql_types::Text, _>(secs.to_string())
	.execute(conn)
	.await
	.expect("backdate reported_at");
}

/// Stand in for an inspection that itemised the group's repo just now and found
/// exactly these snapshots.
async fn record_observed_snapshots(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	snapshot_ids: &[&str],
) {
	let observed: Vec<NewObservedSnapshot> = snapshot_ids
		.iter()
		.map(|id| NewObservedSnapshot {
			snapshot_id: (*id).into(),
			source: "canopy@repo:/data".into(),
			snapshot_at: None,
		})
		.collect();
	BackupRepoObservedSnapshot::replace_for_group(conn, group_id, &observed)
		.await
		.expect("record observed snapshots");
}

/// Back-date every observation of a group's snapshots, so the itemised set is
/// older than the runs being reconciled against it.
async fn age_observed_snapshots(conn: &mut AsyncPgConnection, group_id: Uuid) {
	sql_query(
		"UPDATE backup_repo_observed_snapshots SET observed_at = NOW() - INTERVAL '30 days' \
		 WHERE group_id = $1",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.execute(conn)
	.await
	.expect("age observed snapshots");
}

// --- issue / incident assertion helpers -------------------------------------

/// The most recent server-scoped issue for `(server, source=canopy, ref)`,
/// if any.
#[derive(QueryableByName, Debug)]
struct IssueRow {
	/// What the sweep observed. Unaffected by policy — a backup check still
	/// observes a failure even though its ceiling caps the effect to a
	/// warning.
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	observed_result: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	effective_result: Option<String>,
	#[diesel(sql_type = sql_types::Bool)]
	active: bool,
	#[diesel(sql_type = sql_types::Bool)]
	escalates: bool,
}

async fn machine_issue(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#ref: &str,
) -> Option<IssueRow> {
	sql_query(
		"SELECT observed_result, effective_result, active, escalates FROM issues \
		 WHERE machine_id = $1 AND source = $2 AND \"ref\" = $3",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result::<IssueRow>(conn)
	.await
	.ok()
}

#[derive(QueryableByName, Debug)]
struct MessageRow {
	#[diesel(sql_type = sql_types::Text)]
	message: String,
}

/// The alert text of a server-scoped issue, for asserting on how it reads.
async fn issue_message(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#ref: &str,
) -> Option<String> {
	sql_query(
		"SELECT message FROM issues \
		 WHERE machine_id = $1 AND source = $2 AND \"ref\" = $3",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result::<MessageRow>(conn)
	.await
	.ok()
	.map(|r| r.message)
}

#[derive(QueryableByName, Debug)]
struct NameRow {
	#[diesel(sql_type = sql_types::Text)]
	name: String,
}

/// Every canopy issue ref on a server matching a LIKE pattern. Used to assert
/// a check has exactly one entry rather than one per instance.
async fn issues_matching(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	pattern: &str,
) -> Vec<String> {
	sql_query(
		// A backup staleness check is the box's, so it files at machine scope.
		// spec: BAK
		"SELECT \"ref\" AS name FROM issues \
		 WHERE machine_id = $1 AND source = $2 AND \"ref\" LIKE $3 ORDER BY \"ref\"",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
	.bind::<sql_types::Text, _>(pattern)
	.load::<NameRow>(conn)
	.await
	.expect("load issue refs")
	.into_iter()
	.map(|r| r.name)
	.collect()
}

/// Every catalog entry matching a LIKE pattern — what an operator would have
/// to configure.
async fn catalog_names(conn: &mut AsyncPgConnection, pattern: &str) -> Vec<String> {
	sql_query(
		"SELECT check_name AS name FROM check_policies \
		 WHERE source = $1 AND check_name LIKE $2 ORDER BY check_name",
	)
	.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
	.bind::<sql_types::Text, _>(pattern)
	.load::<NameRow>(conn)
	.await
	.expect("load catalog names")
	.into_iter()
	.map(|r| r.name)
	.collect()
}

#[derive(QueryableByName, Debug)]
struct DetailRow {
	#[diesel(sql_type = sql_types::Nullable<sql_types::Jsonb>)]
	detail: Option<serde_json::Value>,
}

/// The instances a machine's check state holds, by key.
async fn issue_instances(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#ref: &str,
) -> Option<serde_json::Value> {
	sql_query(
		"SELECT instances AS detail FROM issues \
		 WHERE machine_id = $1 AND source = $2 AND \"ref\" = $3",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result::<DetailRow>(conn)
	.await
	.ok()
	.and_then(|r| r.detail)
}

async fn group_issue(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	r#ref: &str,
) -> Option<IssueRow> {
	sql_query(
		"SELECT observed_result, effective_result, active, escalates FROM issues \
		 WHERE server_group_id = $1 AND source = $2 AND \"ref\" = $3",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result::<IssueRow>(conn)
	.await
	.ok()
}

#[derive(QueryableByName)]
struct CountRow {
	#[diesel(sql_type = sql_types::BigInt)]
	n: i64,
}

/// Count open (`left_at IS NULL`) incident links for the issue identified by
/// `(machine_id, ref)`. This isolates one per-server issue's incident
/// membership from any group-level incident on the same group.
async fn machine_issue_open_links(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	r#ref: &str,
) -> i64 {
	sql_query(
		"SELECT COUNT(*) AS n FROM incident_issues ii \
		 JOIN issues i ON i.id = ii.issue_id \
		 WHERE i.machine_id = $1 AND i.\"ref\" = $2 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(machine_id)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result::<CountRow>(conn)
	.await
	.expect("count server issue links")
	.n
}

/// Count open incident links for a group-scoped issue identified by
/// `(group_id, ref)`.
async fn group_issue_open_links(conn: &mut AsyncPgConnection, group_id: Uuid, r#ref: &str) -> i64 {
	sql_query(
		"SELECT COUNT(*) AS n FROM incident_issues ii \
		 JOIN issues i ON i.id = ii.issue_id \
		 WHERE i.server_group_id = $1 AND i.\"ref\" = $2 AND ii.left_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result::<CountRow>(conn)
	.await
	.expect("count group issue links")
	.n
}

// The per-(server,type) ref suffix that `sweep` folds onto STALENESS/NEVER and
// reconcile folds onto its refs (`:{type}`).
// ===========================================================================
// Case 1 — `classify` boundaries (pure, no DB)
// ===========================================================================

/// Build a bare `ScanRow` with no associations; `anchor` degenerates to
/// `config_created_at`.
fn scan_row(
	interval: SignedDuration,
	config_created_at: Timestamp,
	last_success_at: Option<Timestamp>,
) -> ScanRow {
	ScanRow {
		machine_id: Uuid::nil(),
		group_id: Uuid::nil(),
		device_id: None,
		r#type: BackupType::TamanuPostgres,
		is_monitored: true,
		schedule: EffectiveSchedule {
			schedule: Schedule::Interval {
				seconds: interval.as_secs(),
			},
			layer: Some(ScheduleLayer::Group),
			zone: None,
			since: None,
			layers_since: None,
		},
		config_created_at,
		machine_registered_at: None,
		last_success_at,
	}
}

#[test]
fn classify_boundaries() {
	let now: Timestamp = "2026-06-10T00:00:00Z".parse().unwrap();
	let interval = SignedDuration::from_hours(12);
	// grace = interval * 2 = 24h.
	let old_anchor = now - SignedDuration::from_hours(48);

	// Success well within the interval → Ok (no prior open issue).
	let fresh = now - SignedDuration::from_hours(6);
	assert_eq!(
		scan_row(interval, old_anchor, Some(fresh)).classify(now, false),
		StalenessVerdict::Ok,
		"a recent success is healthy",
	);

	// Success past the grace (>24h) → Stale.
	let stale = now - SignedDuration::from_hours(30);
	assert_eq!(
		scan_row(interval, old_anchor, Some(stale)).classify(now, false),
		StalenessVerdict::Stale,
		"a success older than 2x interval is stale",
	);

	// Just inside the grace (exactly 24h is NOT > grace) → Ok.
	let edge = now - SignedDuration::from_hours(24);
	assert_eq!(
		scan_row(interval, old_anchor, Some(edge)).classify(now, false),
		StalenessVerdict::Ok,
		"grace boundary is inclusive (> grace, not >=)",
	);

	// Never succeeded, anchor older than grace → Never.
	assert_eq!(
		scan_row(interval, old_anchor, None).classify(now, false),
		StalenessVerdict::Never,
		"no success ever, past anchor+grace → never",
	);

	// Never succeeded but freshly authorized (anchor inside grace) → Ok.
	let young_anchor = now - SignedDuration::from_hours(6);
	assert_eq!(
		scan_row(interval, young_anchor, None).classify(now, false),
		StalenessVerdict::Ok,
		"freshly-authorized group must not false-alarm as never",
	);

	// Fresh success while an issue was open → Recovered (clear).
	assert_eq!(
		scan_row(interval, old_anchor, Some(fresh)).classify(now, true),
		StalenessVerdict::Recovered,
		"a fresh success while was_active=true is a recovery",
	);

	// Stale stays Stale even if an issue is already open (no premature clear).
	assert_eq!(
		scan_row(interval, old_anchor, Some(stale)).classify(now, true),
		StalenessVerdict::Stale,
		"still-stale does not clear just because an issue is open",
	);
}

/// `classify` only sees `last_success_at`, which `scan_rows` derives from
/// `purpose='backup' AND outcome='success'`. The DB-level companion
/// `latest_success_ignores_restore_and_failure` in `backups.rs` proves a
/// restore/failure run never populates `last_success_at`. Here we just confirm
/// that an absent success (what a restore-only history yields) classifies as
/// Never past the anchor — i.e. a restore does not reset staleness.
#[test]
fn classify_restore_only_history_is_never() {
	let now: Timestamp = "2026-06-10T00:00:00Z".parse().unwrap();
	let interval = SignedDuration::from_hours(12);
	let old_anchor = now - SignedDuration::from_hours(48);
	assert_eq!(
		scan_row(interval, old_anchor, None).classify(now, false),
		StalenessVerdict::Never,
		"restore-only history (no backup success) is never-backed-up",
	);
}

// ===========================================================================
// Case 1b — the scan set resolves the effective interval like the schedulers
// ===========================================================================

/// The out-of-the-box path: no per-group override at all, so the pair inherits
/// the canopy-wide `tamanu-postgres` default (6h, seeded by migration). The
/// schedulers back this pair up on that cadence, so the scan must monitor it —
/// skipping it would make every group without an explicit override an
/// unmonitored backup blindspot.
#[tokio::test(flavor = "multi_thread")]
async fn scan_includes_pair_inheriting_the_type_default_interval() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let group_id = insert_group(&mut conn, "inherits-default").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		enable_capability(&mut conn, machine_id, &pg).await;
		// Deliberately no `server_group_backup_schedule` row.

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		let row = rows
			.iter()
			.find(|r| r.machine_id == machine_id && r.r#type == pg)
			.expect("pair inheriting the type default is in the scan set");
		assert_eq!(
			row.schedule.schedule,
			Schedule::Interval {
				seconds: SignedDuration::from_hours(6).as_secs(),
			},
			"the inherited interval is the type default",
		);
		assert_eq!(row.schedule.layer, Some(ScheduleLayer::Fleet));
	})
	.await;
}

/// An override row with a NULL interval is manual-only, which the type default
/// must not resurrect: no cadence means nothing to be stale against.
#[tokio::test(flavor = "multi_thread")]
async fn scan_excludes_pair_whose_override_makes_it_manual_only() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let group_id = insert_group(&mut conn, "manual-only").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		enable_capability(&mut conn, machine_id, &pg).await;
		ServerGroupBackupSchedule::set_schedule(&mut conn, group_id, &pg, &Schedule::Manual)
			.await
			.expect("insert manual-only override");

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		assert!(
			!rows.iter().any(|r| r.machine_id == machine_id),
			"a manual-only pair has no cadence to be stale against",
		);
	})
	.await;
}

// ===========================================================================
// Case 2 — staleness::sweep files the right server-level events
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn sweep_files_staleness_for_monitored_server_with_old_success() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12); // grace = 24h.
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;
		// Latest success is 48h old → past the 24h grace → stale.
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(48),
		)
		.await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		assert_eq!(rows.len(), 1, "exactly one scanned (server, type)");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("sweep");

		let sref = refs::STALENESS;
		let issue = machine_issue(&mut conn, machine_id, sref)
			.await
			.expect("staleness issue filed");
		// The sweep still observes a failure; the shipped ceiling is what caps
		// it to a warning, so an operator raising the ceiling gets the failure
		// back without a code change.
		assert_eq!(issue.observed_result.as_deref(), Some("failed"));
		assert_eq!(
			issue.effective_result.as_deref(),
			Some("warning"),
			"a late backup is not a live-service failure",
		);
		assert!(!issue.escalates);
		assert!(issue.active, "staleness issue is active");
		assert_eq!(
			machine_issue_open_links(&mut conn, machine_id, sref).await,
			0,
			"a warning does not open an incident on its own",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_files_never_for_server_that_never_succeeded() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12); // grace = 24h.
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;

		// Config created 72h ago, no success ever → anchor+grace exceeded.
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("sweep");

		let nref = refs::NEVER;
		let issue = machine_issue(&mut conn, machine_id, nref)
			.await
			.expect("never issue filed");
		// Never-reported is a warning (so first-time setup doesn't open an
		// incident); a *missed* backup is the error that pages.
		assert_eq!(issue.effective_result.as_deref(), Some("warning"));
		assert!(issue.active);
		// Staleness ref must NOT be filed when there's never been a success.
		assert!(
			machine_issue(&mut conn, machine_id, refs::STALENESS)
				.await
				.is_none(),
			"never path does not also file backup-staleness",
		);
	})
	.await;
}

// ===========================================================================
// Case 3 — the is_monitored gate (per-server)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn unmonitored_staleness_records_issue_but_no_incident_link() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		// Unmonitored server.
		let machine_id = insert_server(&mut conn, group_id, false).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(48),
		)
		.await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("sweep");

		let sref = refs::STALENESS;
		// Issue/event is still recorded unconditionally.
		let issue = machine_issue(&mut conn, machine_id, sref)
			.await
			.expect("issue still recorded for unmonitored server");
		assert!(issue.active);
		// ...but it must NOT contribute to any incident (is_monitored gate).
		assert_eq!(
			machine_issue_open_links(&mut conn, machine_id, sref).await,
			0,
			"unmonitored staleness must not open/join an incident",
		);
	})
	.await;
}

// ===========================================================================
// Case 4 — reconcile::sweep
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_files_report_gap_when_snapshot_fresh_but_no_report() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12); // grace = 24h.
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		// No backup_runs success at all (report path silent), but a FRESH repo
		// snapshot landed for this server's source → report-gap.
		let source = format!("canopy@{machine_id}:/data");
		let fresh_snap = Timestamp::now() - SignedDuration::from_hours(2);
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&source,
			Some(machine_id),
			Some(&pg),
			Some(fresh_snap),
		)
		.await
		.expect("upsert snapshot");

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let gref = refs::RECONCILE_REPORT_GAP;
		let issue = machine_issue(&mut conn, machine_id, gref)
			.await
			.expect("report-gap issue filed");
		assert_eq!(
			issue.effective_result.as_deref(),
			Some("warning"),
			"report-gap is a warning (non-paging)",
		);
		assert!(issue.active);
		// Warning never opens an incident on its own.
		assert_eq!(
			machine_issue_open_links(&mut conn, machine_id, gref).await,
			0,
			"Warning report-gap does not open an incident by itself",
		);
	})
	.await;
}

/// The finding the check's name has always claimed: the device named the
/// snapshot it created and the repository doesn't hold it. Decided by looking
/// the id up, not by comparing timestamps.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_files_missing_when_the_reported_snapshot_is_absent_from_the_repo() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		// A recent run naming the snapshot it cut...
		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-reported",
		)
		.await;
		// ...and an inspection since that run which found other snapshots but
		// not that one.
		record_observed_snapshots(&mut conn, group_id, &["snap-elsewhere"]).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let mref = refs::RECONCILE_MISSING;
		let issue = machine_issue(&mut conn, machine_id, mref)
			.await
			.expect("reconcile-missing issue filed against the server it concerns");
		assert_eq!(issue.observed_result.as_deref(), Some("warning"));
		assert_eq!(
			issue.effective_result.as_deref(),
			Some("warning"),
			"no backup signal is a failure by default",
		);
		assert!(issue.active);
		assert!(
			group_issue(&mut conn, group_id, mref).await.is_none(),
			"the finding is about one server, not the group",
		);
		assert_eq!(
			machine_issue_open_links(&mut conn, machine_id, mref).await,
			0,
			"a warning does not open an incident on its own",
		);

		let message = issue_message(&mut conn, machine_id, mref)
			.await
			.expect("message recorded");
		assert!(
			message.contains("its snapshot is not in the repo"),
			"the message describes the lookup that was made: {message}",
		);
		let detail = issue_instances(&mut conn, machine_id, mref)
			.await
			.expect("instances recorded");
		let degraded: Vec<&serde_json::Value> = detail
			.as_object()
			.expect("instances by key")
			.values()
			.filter(|i| i["effective"] != "passed")
			.collect();
		assert_eq!(
			degraded[0]["detail"]["snapshot_id"], "snap-reported",
			"the id looked up is in the detail: {detail}",
		);
		assert_eq!(
			detail
				.as_object()
				.unwrap()
				.keys()
				.map(String::as_str)
				.collect::<Vec<_>>(),
			[pg.as_str()],
			"the instance is keyed by its backup type",
		);
	})
	.await;
}

/// The inspection job only writes a per-source inventory row for sources it
/// actually finds, so the pair a "missing" verdict is about has no row of its
/// own. The itemised snapshot set is what decides the case, and it belongs to
/// the group rather than to any one pair.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_files_missing_when_the_pair_has_no_snapshot_row_at_all() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let other_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-reported",
		)
		.await;
		// The inspector ran since the report, but only found another server's
		// source — nothing for ours, so ours has no inventory row.
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&format!("canopy@{other_id}:/data"),
			Some(other_id),
			Some(&pg),
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("upsert other server's snapshot");
		record_observed_snapshots(&mut conn, group_id, &["snap-of-the-other-server"]).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let mref = refs::RECONCILE_MISSING;
		let issue = machine_issue(&mut conn, machine_id, mref)
			.await
			.expect("reconcile-missing issue filed for the pair with no snapshot");
		assert_eq!(issue.observed_result.as_deref(), Some("warning"));
		assert_eq!(issue.effective_result.as_deref(), Some("warning"));
		assert!(issue.active);
	})
	.await;
}

/// The regression that motivated splitting this check in two. Inspection runs
/// on its own, slower cadence, so for a server backing up more often than the
/// repo is inspected the newest snapshot on record is routinely older than the
/// last run — with nothing wrong. A verdict measured against `now` instead of
/// against the run it contradicts turned that into an alert on every healthy
/// server in the fleet.
// spec: BKJ#detection
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_leaves_a_healthy_server_alone_when_inspection_lags_the_backup_cadence() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		// Backs up every 6h; the repo is inspected weekly.
		let interval = SignedDuration::from_hours(6);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(24 * 30)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-two-hours-ago",
		)
		.await;

		// The last inspection was three days ago and, of course, found the
		// snapshot that existed then.
		let three_days = SignedDuration::from_hours(72);
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&format!("canopy@{machine_id}:/data"),
			Some(machine_id),
			Some(&pg),
			Some(Timestamp::now() - three_days),
		)
		.await
		.expect("upsert snapshot");
		record_observed_snapshots(&mut conn, group_id, &["snap-three-days-ago"]).await;
		sql_query(
			"UPDATE backup_repo_snapshots SET observed_at = NOW() - INTERVAL '72 hours' \
			 WHERE group_id = $1",
		)
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(&mut conn)
		.await
		.expect("age the inventory to the last inspection");
		age_observed_snapshots(&mut conn, group_id).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_MISSING)
				.await
				.is_none(),
			"a slower inspection cadence is not evidence a backup is missing",
		);
		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_RECENCY)
				.await
				.is_none(),
			"and nothing is recorded either: no inspection has looked since the run",
		);
	})
	.await;
}

/// The timestamp comparison is kept for context, and kept from alerting: it is
/// recorded with what it observed while its effective result stays passed.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_records_the_recency_observation_without_alerting() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-reported",
		)
		.await;
		// Inspected since the run, and holding the reported snapshot — so there
		// is nothing missing — but the newest snapshot time on record for the
		// source is older than the run.
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&format!("canopy@{machine_id}:/data"),
			Some(machine_id),
			Some(&pg),
			Some(Timestamp::now() - SignedDuration::from_hours(72)),
		)
		.await
		.expect("upsert snapshot");
		record_observed_snapshots(&mut conn, group_id, &["snap-reported"]).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let rref = refs::RECONCILE_RECENCY;
		let issue = machine_issue(&mut conn, machine_id, rref)
			.await
			.expect("the observation is recorded");
		assert_eq!(
			issue.observed_result.as_deref(),
			Some("warning"),
			"what was seen is recorded as seen",
		);
		assert_eq!(
			issue.effective_result.as_deref(),
			Some("passed"),
			"a timestamp comparison never ranks as an alert",
		);
		assert!(!issue.active);
		assert_eq!(
			machine_issue_open_links(&mut conn, machine_id, rref).await,
			0,
		);

		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_MISSING)
				.await
				.is_none(),
			"the repo holds the reported snapshot, so nothing is missing",
		);

		// The next inspection catches up. The observation has to come with it:
		// this check is never active, so nothing else would ever retract it.
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&format!("canopy@{machine_id}:/data"),
			Some(machine_id),
			Some(&pg),
			Some(Timestamp::now() - SignedDuration::from_hours(1)),
		)
		.await
		.expect("refresh snapshot");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("second reconcile sweep");
		assert_eq!(
			machine_issue(&mut conn, machine_id, rref)
				.await
				.expect("still recorded")
				.observed_result
				.as_deref(),
			Some("passed"),
			"the recorded observation is brought up to date once the repo catches up",
		);
	})
	.await;
}

/// Two applications in one group failing the same check hold two separate alerts.
/// While this was group-scoped they collided on one row, so whichever server
/// the sweep reached last owned the message — and the healthy one's recovery
/// filed a `Passed` that cleared the broken one's alert.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_missing_is_per_server_not_shared_across_the_group() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let broken_id = insert_server(&mut conn, group_id, true).await;
		let healthy_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, broken_id, &pg).await;
		enable_capability(&mut conn, healthy_id, &pg).await;

		// Both report success recently, each naming its own snapshot.
		for (sid, snap) in [(broken_id, "snap-broken"), (healthy_id, "snap-healthy")] {
			insert_backup_success_with_snapshot(
				&mut conn,
				device_id,
				group_id,
				sid,
				&pg,
				SignedDuration::from_hours(2),
				snap,
			)
			.await;
		}
		// The repo holds only the healthy server's snapshot.
		record_observed_snapshots(&mut conn, group_id, &["snap-healthy"]).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let mref = refs::RECONCILE_MISSING;
		let broken = machine_issue(&mut conn, broken_id, mref)
			.await
			.expect("the server whose snapshot is missing holds the alert");
		assert_eq!(broken.observed_result.as_deref(), Some("warning"));
		assert_eq!(broken.effective_result.as_deref(), Some("warning"));
		assert!(
			broken.active,
			"the healthy server in the same group must not clear it",
		);
		assert!(
			machine_issue(&mut conn, healthy_id, mref).await.is_none(),
			"the healthy server has no reconcile-missing alert of its own",
		);
	})
	.await;
}

/// Alert text names the server the way an operator knows it, not by id.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_missing_names_the_server_rather_than_its_id() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		// The message names the box, so that is what carries the name.
		sql_query("UPDATE machines SET name = 'kotare-central' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(machine_id)
			.execute(&mut conn)
			.await
			.expect("name the machine");

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-reported",
		)
		.await;
		record_observed_snapshots(&mut conn, group_id, &["snap-elsewhere"]).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let mref = refs::RECONCILE_MISSING;
		let message = issue_message(&mut conn, machine_id, mref)
			.await
			.expect("reconcile-missing issue filed");
		assert!(
			message.contains("kotare-central"),
			"message names the server: {message}",
		);
		assert!(
			!message.contains(&machine_id.to_string()),
			"message does not fall back to the id: {message}",
		);
	})
	.await;
}

/// A group the inspector has never run against has nothing to conclude
/// "missing" from: absence of a snapshot from an inventory nobody has written
/// says only that nobody has looked.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_skips_missing_when_the_group_was_never_inspected() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;
		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-reported",
		)
		.await;
		// No inventory rows of either kind for the group.

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		assert!(
			group_issue(&mut conn, group_id, refs::RECONCILE_MISSING)
				.await
				.is_none(),
			"an uninspected group must not be accused of losing backups",
		);
		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_MISSING)
				.await
				.is_none(),
			"and no per-server finding either",
		);
		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_RECENCY)
				.await
				.is_none(),
			"nor is a recency observation recorded from an inventory that doesn't exist",
		);
	})
	.await;
}

/// An inventory older than the run makes the missing verdict undecidable, not
/// resolved. With every type undecidable there is nothing to conclude, so an
/// already-open finding stays open rather than being cleared on the strength of
/// a lagging inspector.
#[tokio::test(flavor = "multi_thread")]
async fn reconcile_leaves_an_open_missing_alone_when_the_inventory_goes_stale() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;
		insert_backup_success_with_snapshot(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
			"snap-reported",
		)
		.await;

		// Inspected since the run, and the reported snapshot isn't there → the
		// finding is raised.
		record_observed_snapshots(&mut conn, group_id, &["snap-elsewhere"]).await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("first sweep");
		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_MISSING)
				.await
				.expect("finding raised")
				.active,
		);

		// Now the inspector falls behind: every observation predates the run it
		// would be judging.
		age_observed_snapshots(&mut conn, group_id).await;

		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("second sweep");
		assert!(
			machine_issue(&mut conn, machine_id, refs::RECONCILE_MISSING)
				.await
				.expect("finding still present")
				.active,
			"a lagging inspector must not clear an open finding",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_clears_report_gap_when_report_and_snapshot_agree() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		let source = format!("canopy@{machine_id}:/data");
		let fresh_snap = Timestamp::now() - SignedDuration::from_hours(2);
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&source,
			Some(machine_id),
			Some(&pg),
			Some(fresh_snap),
		)
		.await
		.expect("upsert snapshot");

		// First sweep: snapshot fresh, no report → report-gap opens.
		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("first reconcile");
		let gref = refs::RECONCILE_REPORT_GAP;
		assert!(
			machine_issue(&mut conn, machine_id, gref)
				.await
				.expect("report-gap open")
				.active,
			"precondition: report-gap is active",
		);

		// Now a fresh report lands too → (report_fresh, snapshot_fresh) clears it.
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(1),
		)
		.await;
		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan2");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("second reconcile");

		let issue = machine_issue(&mut conn, machine_id, gref)
			.await
			.expect("report-gap still present (now cleared)");
		assert!(
			!issue.active,
			"report-gap cleared once report and snapshot agree"
		);
		assert_eq!(issue.effective_result.as_deref(), Some("passed"));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_files_size_mismatch_when_reported_size_differs_from_repo() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		// A reported run (bytes_uploaded = 42, snapshot_id "kopia-snap")...
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(2),
		)
		.await;
		// ...but inspection observed a different logical size for that snapshot.
		BackupRun::backfill_snapshot_logical_bytes(
			&mut conn,
			group_id,
			&[("kopia-snap".into(), 99)],
		)
		.await
		.expect("backfill");

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let sref = refs::RECONCILE_SIZE_MISMATCH;
		let issue = machine_issue(&mut conn, machine_id, sref)
			.await
			.expect("size-mismatch issue filed");
		assert_eq!(
			issue.effective_result.as_deref(),
			Some("warning"),
			"size-mismatch is a warning (non-paging)",
		);
		assert!(issue.active);
		assert_eq!(
			machine_issue_open_links(&mut conn, machine_id, sref).await,
			0,
			"Warning size-mismatch does not open an incident by itself",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn reconcile_clears_size_mismatch_when_latest_sizes_agree() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		// An older run whose reported size disagrees with the repo → raises.
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(5),
		)
		.await;
		BackupRun::backfill_snapshot_logical_bytes(
			&mut conn,
			group_id,
			&[("kopia-snap".into(), 99)],
		)
		.await
		.expect("backfill mismatch");
		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("first reconcile");
		let sref = refs::RECONCILE_SIZE_MISMATCH;
		assert!(
			machine_issue(&mut conn, machine_id, sref)
				.await
				.expect("size-mismatch open")
				.active,
			"precondition: size-mismatch is active",
		);

		// A newer run whose reported and observed sizes agree becomes the latest
		// comparable run → clears. (Write-once backfill leaves the older run's
		// recorded size untouched; only the new null row is filled.)
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&pg,
			SignedDuration::from_hours(1),
		)
		.await;
		BackupRun::backfill_snapshot_logical_bytes(
			&mut conn,
			group_id,
			&[("kopia-snap".into(), 42)],
		)
		.await
		.expect("backfill match");
		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan2");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("second reconcile");

		let issue = machine_issue(&mut conn, machine_id, sref)
			.await
			.expect("size-mismatch still present (now cleared)");
		assert!(
			!issue.active,
			"size-mismatch cleared once latest sizes agree"
		);
		assert_eq!(issue.effective_result.as_deref(), Some("passed"));
	})
	.await;
}

// ===========================================================================
// Case 5 — raise_group_event bypasses the is_monitored gate (NON-NEGOTIABLE)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn group_event_pages_even_when_all_members_unmonitored() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		// Every member is UNMONITORED — the per-server path would never page.
		let _s1 = insert_server(&mut conn, group_id, false).await;
		let _s2 = insert_server(&mut conn, group_id, false).await;

		let stamp = database::issues::CheckStateStamp {
			check: refs::CORRUPTION.into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: true,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let issue = database::issues::raise_group_event_with_state(
			&mut conn,
			group_id,
			refs::CORRUPTION,
			None,
			"repo corruption detected",
			true,
			Some(&stamp),
		)
		.await
		.expect("raise group event");

		assert_eq!(
			issue.application_id, None,
			"group-scoped issue has no machine_id"
		);
		assert_eq!(issue.server_group_id, Some(group_id));
		assert!(issue.active);

		// It opens an incident on the group despite all members being unmonitored.
		assert_eq!(
			group_issue_open_links(&mut conn, group_id, refs::CORRUPTION).await,
			1,
			"group-level event opens an incident regardless of is_monitored",
		);
		let open = database::issues::Incident::list_for_group(&mut conn, group_id, false, 10)
			.await
			.expect("list incidents");
		assert_eq!(open.len(), 1, "exactly one open incident on the group");

		// Recovery: same (source, ref) with active=false and a passed
		// result leaves the incident and closes it.
		let stamp = database::issues::CheckStateStamp {
			check: refs::CORRUPTION.into(),
			observed: CheckResult::Passed,
			effective: CheckResult::Passed,
			escalates: true,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		database::issues::raise_group_event_with_state(
			&mut conn,
			group_id,
			refs::CORRUPTION,
			None,
			"repo corruption cleared",
			false,
			Some(&stamp),
		)
		.await
		.expect("clear group event");
		assert_eq!(
			group_issue_open_links(&mut conn, group_id, refs::CORRUPTION).await,
			0,
			"recovery removes the issue from its incident",
		);
		// The incident lingers (close-side grace) rather than closing on
		// the spot; once the window elapses the sweep closes it.
		let still_open = database::issues::Incident::list_for_group(&mut conn, group_id, false, 10)
			.await
			.expect("list incidents 2");
		assert_eq!(
			still_open.len(),
			1,
			"incident lingers after its only contributor recovers",
		);
		sql_query(
			"UPDATE incidents SET closing_at = closing_at - INTERVAL '1 hour' \
			 WHERE server_group_id = $1",
		)
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(&mut conn)
		.await
		.expect("expire linger");
		database::issues::sweep_lingering_incidents(&mut conn)
			.await
			.expect("linger sweep");
		let still_open = database::issues::Incident::list_for_group(&mut conn, group_id, false, 10)
			.await
			.expect("list incidents 3");
		assert!(
			still_open.is_empty(),
			"linger sweep closes the incident once the window elapses",
		);
	})
	.await;
}

// ===========================================================================
// Case 6 — maintenance failure (backup-maintenance-error)
// ===========================================================================

#[tokio::test(flavor = "multi_thread")]
async fn sweep_files_maintenance_error_when_latest_run_failed_then_clears_on_success() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		// Freshly-created config so maintenance-STALE does NOT also fire — this
		// isolates the failure signal from absence-of-success.
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(1)).await;

		// The most recently finished run failed an hour ago.
		insert_maintenance_run(
			&mut conn,
			group_id,
			"full",
			"failure",
			Some("kopia maintenance: connection refused"),
			SignedDuration::from_hours(1),
		)
		.await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("sweep");

		let issue = group_issue(&mut conn, group_id, refs::MAINTENANCE_ERROR)
			.await
			.expect("maintenance-error issue filed");
		assert_eq!(issue.observed_result.as_deref(), Some("failed"));
		assert_eq!(
			issue.effective_result.as_deref(),
			Some("warning"),
			"a repository that misses maintenance stays fully restorable",
		);
		assert!(issue.active, "the issue is still active");
		assert_eq!(
			group_issue_open_links(&mut conn, group_id, refs::MAINTENANCE_ERROR).await,
			0,
			"a warning does not open an incident on its own",
		);
		// Staleness must NOT fire for a freshly-created group.
		assert!(
			group_issue(&mut conn, group_id, refs::MAINTENANCE_STALE)
				.await
				.is_none(),
			"a recent config is not maintenance-stale",
		);

		// A newer successful run is now the latest finished run → clears it.
		insert_maintenance_run(
			&mut conn,
			group_id,
			"full",
			"success",
			None,
			SignedDuration::from_secs(0),
		)
		.await;
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("re-sweep");

		let cleared = group_issue(&mut conn, group_id, refs::MAINTENANCE_ERROR)
			.await
			.expect("issue row persists");
		assert!(
			!cleared.active,
			"failure issue cleared after a successful run",
		);
		assert_eq!(
			group_issue_open_links(&mut conn, group_id, refs::MAINTENANCE_ERROR).await,
			0,
			"recovery removes the failure issue from its incident",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn in_flight_run_does_not_clear_an_open_maintenance_error() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(1)).await;
		insert_maintenance_run(
			&mut conn,
			group_id,
			"full",
			"failure",
			Some("boom"),
			SignedDuration::from_hours(1),
		)
		.await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("sweep");
		assert!(
			group_issue(&mut conn, group_id, refs::MAINTENANCE_ERROR)
				.await
				.expect("error filed")
				.active
		);

		// A run that has started but not finished (outcome NULL) must be ignored:
		// it is not evidence that the failure recovered.
		database::BackupMaintenanceRun::start(
			&mut conn,
			group_id,
			commons_types::backup::MaintenanceKind::Full,
		)
		.await
		.expect("start in-flight run");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("re-sweep");

		assert!(
			group_issue(&mut conn, group_id, refs::MAINTENANCE_ERROR)
				.await
				.expect("error still present")
				.active,
			"an in-flight run must not clear the failure issue",
		);
	})
	.await;
}

/// The severity rule, checked against what the sweeps actually seed into the
/// catalog rather than against any single assertion: no backup check ships as
/// a failure, and none escalates. A check that ships as a failure sends tech
/// support looking for a live-service outage that isn't happening.
///
/// Only the checks reachable from this crate's sweeps are covered — corruption,
/// rotation-broken, and object-lock are filed from the `jobs` crate and are the
/// three deliberate exceptions, so they are listed here and exempted rather
/// than asserted on.
// spec: BKJ#alerting
#[tokio::test(flavor = "multi_thread")]
async fn backup_sweeps_seed_no_check_that_defaults_to_a_failure() {
	#[derive(QueryableByName, Debug)]
	struct PolicyRow {
		#[diesel(sql_type = sql_types::Text)]
		check_name: String,
		#[diesel(sql_type = sql_types::Text)]
		ceiling: String,
		#[diesel(sql_type = sql_types::Bool)]
		escalates: bool,
	}

	/// The only backup-sphere checks allowed to default to an escalating
	/// failure: the backups are already gone, unrecoverable, or unprotected,
	/// rather than merely late. Adding to this list is a decision to make with
	/// the people who answer the alerts, not a default to ship.
	const MAY_FAIL: [&str; 3] = [
		refs::CORRUPTION,
		refs::ROTATION_BROKEN,
		refs::PREFLIGHT_OBJECT_LOCK,
	];

	/// Checks that ship lower than a warning, at a ceiling of `passed`:
	/// recorded and visible, never alerting, because their evidence supports
	/// "something looks off" and nothing firmer.
	const RECORDED_ONLY: [&str; 1] = [refs::RECONCILE_RECENCY];

	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let interval = SignedDuration::from_hours(12);
		let group_id = insert_group(&mut conn, "g").await;
		let stale_server = insert_server(&mut conn, group_id, true).await;
		let never_server = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;

		// Nine days: past MAINTENANCE_STALE_AFTER, so the group with no
		// maintenance run at all also files maintenance-stale.
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(9 * 24)).await;
		insert_schedule(&mut conn, group_id, &pg, interval).await;
		enable_capability(&mut conn, stale_server, &pg).await;
		enable_capability(&mut conn, never_server, &pg).await;

		// One server with an old success (staleness), one that never succeeded
		// (never), and a group with no maintenance run (maintenance-stale).
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			stale_server,
			&pg,
			SignedDuration::from_hours(72),
		)
		.await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("staleness sweep");
		database::backup::reconcile::sweep(&mut conn, &rows)
			.await
			.expect("reconcile sweep");

		let catalog: Vec<PolicyRow> = sql_query(
			"SELECT check_name, ceiling, escalates FROM check_policies \
			 WHERE source = $1 ORDER BY check_name",
		)
		.bind::<sql_types::Text, _>(refs::CANOPY_SOURCE)
		.load(&mut conn)
		.await
		.expect("load catalog");

		let covered: Vec<&PolicyRow> = catalog
			.iter()
			.filter(|r| !MAY_FAIL.contains(&r.check_name.as_str()))
			.collect();
		assert!(
			covered.len() >= 3,
			"the sweeps should have seeded the staleness, never, and \
			 maintenance checks; got {:?}",
			catalog.iter().map(|r| &r.check_name).collect::<Vec<_>>(),
		);
		for row in covered {
			let expected = if RECORDED_ONLY.contains(&row.check_name.as_str()) {
				("passed", false)
			} else {
				("warning", false)
			};
			assert_eq!(
				(row.ceiling.as_str(), row.escalates),
				expected,
				"{} must ship no more urgently than a non-escalating warning: a \
				 failure means a live service is down, and a backup problem is \
				 not that",
				row.check_name,
			);
		}
	})
	.await;
}

/// One check per server, whatever it backs up. A server stale on three of its
/// four types holds a single `backup-staleness` check naming all three, not
/// three checks — and the catalog has one entry to configure, not three.
// spec: CHK#names
#[tokio::test(flavor = "multi_thread")]
async fn staleness_is_one_check_per_server_with_the_types_as_instances() {
	TestDb::run(|mut conn, _url| async move {
		let interval = SignedDuration::from_hours(12);
		let stale_types = [
			BackupType::TamanuPostgres,
			BackupType::Custom("tamanu-config".into()),
			BackupType::Custom("caddy-config".into()),
		];
		let fresh_type = BackupType::Custom("postgres-config".into());

		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;

		for ty in stale_types.iter().chain(std::iter::once(&fresh_type)) {
			insert_schedule(&mut conn, group_id, ty, interval).await;
			enable_capability(&mut conn, machine_id, ty).await;
		}
		// Three types last succeeded three days ago (stale), one an hour ago.
		for ty in &stale_types {
			insert_backup_success_aged(
				&mut conn,
				device_id,
				group_id,
				machine_id,
				ty,
				SignedDuration::from_hours(72),
			)
			.await;
		}
		insert_backup_success_aged(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			&fresh_type,
			SignedDuration::from_hours(1),
		)
		.await;

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		assert_eq!(rows.len(), 4, "four scanned (server, type) pairs");
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.expect("sweep");

		// Exactly one staleness issue for the server, and no parameterised
		// variants alongside it.
		let issues = issues_matching(&mut conn, machine_id, "backup-staleness%").await;
		assert_eq!(
			issues,
			vec![refs::STALENESS.to_string()],
			"one staleness check per server, named for the condition only",
		);

		let issue = machine_issue(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("staleness issue filed");
		assert_eq!(issue.observed_result.as_deref(), Some("failed"));
		assert_eq!(issue.effective_result.as_deref(), Some("warning"));

		// The message names the types it is stale for, and the detail carries
		// them with their own results — the thing a name per type used to say.
		let message = issue_message(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("message");
		for ty in &stale_types {
			assert!(
				message.contains(&ty.to_string()),
				"message names {ty}: {message}",
			);
		}
		assert!(
			!message.contains(&fresh_type.to_string()),
			"the fresh type is not named: {message}",
		);

		let detail = issue_instances(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("instances");
		let instances = detail.as_object().expect("instances by key");
		assert_eq!(instances.len(), 4, "four instances were considered");
		assert_eq!(
			instances
				.values()
				.filter(|i| i["effective"] != "passed")
				.count(),
			3,
			"three of them are stale",
		);
		for ty in &stale_types {
			assert_eq!(
				instances[&ty.to_string()]["effective"],
				"warning",
				"detail holds {ty} as degraded",
			);
		}
		assert_eq!(
			instances[&fresh_type.to_string()]["effective"],
			"passed",
			"detail holds the healthy type as passing",
		);

		// And the catalog gained one entry, not one per type.
		let catalog = catalog_names(&mut conn, "backup-staleness%").await;
		assert_eq!(
			catalog,
			vec![refs::STALENESS.to_string()],
			"one catalog entry to configure, not one per backup type",
		);
	})
	.await;
}

/// A backup type is an instance of the staleness check, keyed by the type, so
/// silencing one type on a machine quiets that type alone: its siblings still
/// grade, the message stops naming it, and the check passes only once every
/// stale type is quiet.
// spec: CHK#silencing-one-instance
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silence_on_one_backup_type_quiets_that_type_only() {
	TestDb::run(|mut conn, _url| async move {
		let interval = SignedDuration::from_hours(12);
		let pg = BackupType::TamanuPostgres;
		let config = BackupType::Custom("tamanu-config".into());

		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		let device_id = insert_device(&mut conn).await;
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(72)).await;
		for ty in [&pg, &config] {
			insert_schedule(&mut conn, group_id, ty, interval).await;
			enable_capability(&mut conn, machine_id, ty).await;
			insert_backup_success_aged(
				&mut conn,
				device_id,
				group_id,
				machine_id,
				ty,
				SignedDuration::from_hours(72),
			)
			.await;
		}

		let sweep = async |conn: &mut AsyncPgConnection| {
			let rows = database::backup::staleness::scan_rows(conn)
				.await
				.expect("scan");
			database::backup::staleness::sweep(conn, &rows)
				.await
				.expect("sweep");
		};
		sweep(&mut conn).await;
		let instances = issue_instances(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("instances");
		assert_eq!(
			instances
				.as_object()
				.unwrap()
				.keys()
				.map(String::as_str)
				.collect::<Vec<_>>(),
			[config.as_str(), pg.as_str()],
			"keyed by backup type",
		);

		database::silenced_refs::MachineSilencedRef::add(
			&mut conn,
			machine_id,
			refs::CANOPY_SOURCE,
			refs::STALENESS,
			Some(pg.as_str()),
			Some("op@example.com"),
		)
		.await
		.expect("silence one type");
		sweep(&mut conn).await;

		let instances = issue_instances(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("instances");
		assert_eq!(instances[pg.as_str()]["observed"], "failed");
		assert_eq!(
			instances[pg.as_str()]["effective"],
			"skipped",
			"the silenced type is quiet",
		);
		assert_eq!(
			instances[config.as_str()]["effective"],
			"warning",
			"its sibling still grades",
		);
		let issue = machine_issue(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("staleness issue");
		assert!(issue.active);
		assert_eq!(issue.effective_result.as_deref(), Some("warning"));
		let message = issue_message(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("message");
		assert!(message.contains(config.as_str()), "{message}");
		assert!(
			!message.contains(pg.as_str()),
			"the silenced type is not named: {message}",
		);

		database::silenced_refs::MachineSilencedRef::add(
			&mut conn,
			machine_id,
			refs::CANOPY_SOURCE,
			refs::STALENESS,
			Some(config.as_str()),
			Some("op@example.com"),
		)
		.await
		.expect("silence the other type");
		sweep(&mut conn).await;
		let issue = machine_issue(&mut conn, machine_id, refs::STALENESS)
			.await
			.expect("staleness issue");
		assert!(!issue.active, "every stale type is quiet");
		assert_eq!(issue.effective_result.as_deref(), Some("skipped"));
	})
	.await;
}

/// The staleness anchor is when the BOX was enrolled, not when its backup
/// configuration was created. A machine onboarded into a configuration that
/// predates it is not stale on arrival: it is given its grace from the moment
/// it joined.
// spec: BKJ
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_onboarded_into_an_existing_config_is_not_stale_on_arrival() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let group_id = insert_group(&mut conn, "long-running").await;
		let machine_id = insert_server(&mut conn, group_id, true).await;
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(12)).await;
		enable_capability(&mut conn, machine_id, &pg).await;

		// The configuration has existed for a month; the box joined an hour ago
		// and has never backed up.
		sql_query(
			"UPDATE server_group_backup_config SET created_at = NOW() - interval '30 days' \
			 WHERE group_id = $1",
		)
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(&mut conn)
		.await
		.expect("age the config");
		sql_query(
			"UPDATE machines SET registered_at = NOW() - interval '1 hour' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(machine_id)
		.execute(&mut conn)
		.await
		.expect("enrol the machine");

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		let row = rows
			.iter()
			.find(|r| r.machine_id == machine_id && r.r#type == pg)
			.expect("the pair is in the scan set");
		assert_eq!(
			row.classify(Timestamp::now(), false),
			StalenessVerdict::Ok,
			"a freshly-onboarded box is inside its grace, not overdue since the config was made",
		);
	})
	.await;
}

/// Anchored on the machine rather than the application, so deploying a second
/// workload onto a box does not restart the box's backup deadline. Both
/// applications answer with the same anchor, and a machine that has been
/// failing to back up stays overdue.
// spec: BKJ
#[tokio::test(flavor = "multi_thread")]
async fn a_second_application_does_not_restart_the_machines_anchor() {
	TestDb::run(|mut conn, _url| async move {
		let pg = BackupType::TamanuPostgres;
		let group_id = insert_group(&mut conn, "two-workloads").await;
		let first = insert_server(&mut conn, group_id, true).await;
		insert_ready_config(&mut conn, group_id, SignedDuration::from_hours(12)).await;
		enable_capability(&mut conn, first, &pg).await;

		// The box was enrolled a month ago and has never backed up.
		sql_query(
			"UPDATE machines SET registered_at = NOW() - interval '30 days' \
			 WHERE id = $1",
		)
		.bind::<sql_types::Uuid, _>(first)
		.execute(&mut conn)
		.await
		.expect("enrol the machine");

		// A second workload lands on the same box today. It shares the box's
		// group, which the trigger corrects on update rather than on insert.
		let host = format!("http://test.invalid/{}", Uuid::new_v4());
		sql_query(
			"INSERT INTO applications (host, type, machine_id, group_id, rank) \
			 VALUES ($1, 'tamanu-central', $2, $3, 'production')",
		)
		.bind::<sql_types::Text, _>(host)
		.bind::<sql_types::Uuid, _>(first)
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(&mut conn)
		.await
		.expect("insert second application");

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.expect("scan");
		let now = Timestamp::now();

		// One row for the box, not one per workload: the capability is the
		// box's, and a run captures the box's data once.
		let for_box: Vec<_> = rows
			.iter()
			.filter(|r| r.machine_id == first && r.r#type == pg)
			.collect();
		assert_eq!(
			for_box.len(),
			1,
			"a box carrying two workloads owes one backup, not two",
		);
		assert_eq!(
			for_box[0].classify(now, false),
			StalenessVerdict::Never,
			"the box has been failing to back up for a month; a new workload does not reset that",
		);
	})
	.await;
}
