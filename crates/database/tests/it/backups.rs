//! DB-layer tests for the backup-credentials models (`database::backups`).
//! Exercises the model helpers directly against a fresh migrated DB — no HTTP.

use commons_errors::AppError;
use commons_tests::db::TestDb;
use commons_types::backup::schedule::Schedule;
use database::diesel_async::AsyncPgConnection;
use database::pg_duration::PgDuration;
use database::{
	BackupConfigStatus, BackupCredentialIssuance, BackupMaintenanceRunFilters, BackupPurpose,
	BackupRepoSnapshot, BackupRepoStats, BackupRequest, BackupRun, BackupRunFilters,
	BackupRunProgress, BackupType, BackupTypeDefault, MachineBackupCapability, MaintenanceKind,
	MaintenanceOutcomeFilter, NewBackupCredentialIssuance, NewBackupRun, NewBackupRunProgress,
	NewBackupTypeDefault, NewServerGroupBackupConfig, RunOutcome, ServerGroupBackupConfig,
	ServerGroupBackupSchedule, backups::BackupMaintenanceRun,
};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

// --- seeding helpers (raw SQL, matching the existing test style) -----------

#[derive(diesel::QueryableByName)]
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

/// A box with one workload on it, returning the *machine*: a backup is the
/// box's, so that is what these tests key on.
// spec: BAK
async fn insert_server(conn: &mut AsyncPgConnection, group_id: Uuid) -> Uuid {
	let machine =
		sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
			.bind::<sql_types::Uuid, _>(group_id)
			.get_result::<RowId>(conn)
			.await
			.expect("insert machine")
			.id;
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	sql_query(
		"INSERT INTO applications (host, type, group_id, machine_id) \
		 VALUES ($1, 'tamanu-central', $2, $3) RETURNING id",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group_id)
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

fn new_config(group_id: Uuid, region: Option<&str>) -> NewServerGroupBackupConfig {
	NewServerGroupBackupConfig {
		group_id,
		bucket: "bes-kopia-backups-test".into(),
		prefix: String::new(),
		target_role_arn: "arn:aws:iam::123456789012:role/canopy-backups-test".into(),
		maintenance_role_arn: "arn:aws:iam::123456789012:role/canopy-maint-test".into(),
		region: region.map(Into::into),
		repo_password_ref: "kopia-repo-pw-test".into(),
		status: BackupConfigStatus::Provisioning,
		mode: commons_types::backup::BackupRepoMode::FromBirth,
		placement: commons_types::backup::BackupPlacement::External,
	}
}

// --- config -----------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn config_upsert_roundtrip_and_status() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;

		// Insert with NULL region.
		let cfg = ServerGroupBackupConfig::upsert(&mut conn, new_config(group_id, None))
			.await
			.expect("insert config");
		assert_eq!(cfg.region, None);
		assert_eq!(cfg.status, BackupConfigStatus::Provisioning);
		assert_eq!(
			cfg.created_at, cfg.updated_at,
			"fresh row: created == updated"
		);

		// Upsert replaces mutable fields (region now set).
		let cfg = ServerGroupBackupConfig::upsert(
			&mut conn,
			new_config(group_id, Some("ap-southeast-2")),
		)
		.await
		.expect("upsert config");
		assert_eq!(cfg.region.as_deref(), Some("ap-southeast-2"));

		// Status transition + updated_at auto-touch.
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		let cfg =
			ServerGroupBackupConfig::set_status(&mut conn, group_id, BackupConfigStatus::Ready)
				.await
				.expect("set status");
		assert_eq!(cfg.status, BackupConfigStatus::Ready);
		assert!(
			cfg.updated_at > cfg.created_at,
			"updated_at auto-touched on update"
		);

		assert!(
			ServerGroupBackupConfig::get(&mut conn, group_id)
				.await
				.unwrap()
				.is_some()
		);
		assert!(
			ServerGroupBackupConfig::get(&mut conn, Uuid::new_v4())
				.await
				.unwrap()
				.is_none()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn type_defaults_retention_must_be_object() {
	TestDb::run(|mut conn, _url| async move {
		// Valid object retention round-trips.
		let td = BackupTypeDefault::upsert(
			&mut conn,
			NewBackupTypeDefault {
				r#type: BackupType::TamanuPostgres,
				default_interval: Some(PgDuration(SignedDuration::from_hours(24))),
				default_cron: None,
				default_zone: None,
				default_retention: retention(),
				auto_enable: true,
				allow_below_floor: false,
			},
		)
		.await
		.expect("insert type default");
		assert_eq!(td.r#type, BackupType::TamanuPostgres);
		assert!(td.auto_enable);

		// A non-object retention is rejected by the jsonb_typeof CHECK.
		let err = BackupTypeDefault::upsert(
			&mut conn,
			NewBackupTypeDefault {
				r#type: BackupType::from("bad"),
				default_interval: None,
				default_cron: None,
				default_zone: None,
				default_retention: serde_json::json!([1, 2, 3]),
				auto_enable: false,
				allow_below_floor: false,
			},
		)
		.await;
		assert!(err.is_err(), "array retention must violate the CHECK");
	})
	.await;
}

// --- FK semantics: plain references, no cascade (archival model) ------------

#[tokio::test(flavor = "multi_thread")]
async fn hard_deleting_a_group_with_backup_rows_is_blocked() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;

		ServerGroupBackupConfig::upsert(&mut conn, new_config(group_id, None))
			.await
			.unwrap();
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.unwrap();

		// Groups/applications are archived, never hard-deleted; the FKs are plain
		// references (no cascade), so a hard DELETE is blocked rather than
		// silently cascading the config/audit away.
		let res = sql_query("DELETE FROM server_groups WHERE id = $1")
			.bind::<sql_types::Uuid, _>(group_id)
			.execute(&mut conn)
			.await;
		assert!(
			res.is_err(),
			"FK restrict must block deleting a referenced group"
		);
	})
	.await;
}

// --- backup_runs ------------------------------------------------------------

fn new_run(
	id: Uuid,
	device_id: Uuid,
	group_id: Uuid,
	machine_id: Option<Uuid>,
	purpose: BackupPurpose,
	outcome: RunOutcome,
) -> NewBackupRun {
	NewBackupRun {
		id,
		device_id,
		group_id,
		machine_id,
		r#type: BackupType::TamanuPostgres,
		purpose,
		outcome,
		error: None,
		bytes_uploaded: Some(42),
		snapshot_id: Some("kopia-snap-1".into()),
		s3_sent_raw_bytes: None,
		s3_sent_payload_bytes: None,
		s3_received_raw_bytes: None,
		s3_received_payload_bytes: None,
		snapshot_taken_at: None,
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn backup_run_client_uuid_and_duplicate_is_conflict() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;

		let run_id = Uuid::new_v4();
		let run = BackupRun::record(
			&mut conn,
			new_run(
				run_id,
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.expect("first insert");
		assert_eq!(run.id, run_id, "client-supplied UUID is the PK");

		// Re-inserting the same UUID is a clean Conflict, not a panic.
		let err = BackupRun::record(
			&mut conn,
			new_run(
				run_id,
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Failure,
			),
		)
		.await
		.expect_err("duplicate must error");
		assert!(matches!(err, AppError::Conflict(_)), "got {err:?}");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn backfill_snapshot_logical_bytes_writes_once() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;

		let id = Uuid::new_v4();
		BackupRun::record(
			&mut conn,
			new_run(
				id,
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.expect("record");

		// Matched by snapshot id ("kopia-snap-1" from new_run); starts unset.
		let n = BackupRun::backfill_snapshot_logical_bytes(
			&mut conn,
			group_id,
			&[("kopia-snap-1".into(), 1490)],
		)
		.await
		.expect("backfill");
		assert_eq!(n, 1);
		let runs = BackupRun::list_for_group(&mut conn, group_id, 10)
			.await
			.unwrap();
		assert_eq!(runs[0].snapshot_logical_bytes, Some(1490));

		// Write-once: a later inspection with a different value doesn't clobber.
		let n2 = BackupRun::backfill_snapshot_logical_bytes(
			&mut conn,
			group_id,
			&[("kopia-snap-1".into(), 9999)],
		)
		.await
		.expect("backfill again");
		assert_eq!(n2, 0);
		let runs = BackupRun::list_for_group(&mut conn, group_id, 10)
			.await
			.unwrap();
		assert_eq!(runs[0].snapshot_logical_bytes, Some(1490));

		// An unknown snapshot id matches nothing.
		let n3 =
			BackupRun::backfill_snapshot_logical_bytes(&mut conn, group_id, &[("nope".into(), 1)])
				.await
				.expect("backfill unknown");
		assert_eq!(n3, 0);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn snapshot_sizes_by_id_resolves_from_producing_backups_only() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let other_group = insert_group(&mut conn, "other").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let other_server = insert_server(&mut conn, other_group).await;
		let device_id = insert_device(&mut conn).await;

		// The backup that produced kopia-snap-1, sized by the device (42 from
		// new_run's bytes_uploaded).
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.expect("record backup");
		// A restore of the same snapshot must not satisfy the lookup itself.
		BackupRun::record(
			&mut conn,
			NewBackupRun {
				bytes_uploaded: None,
				..new_run(
					Uuid::new_v4(),
					device_id,
					group_id,
					Some(machine_id),
					BackupPurpose::Restore,
					RunOutcome::Success,
				)
			},
		)
		.await
		.expect("record restore");
		// Same snapshot id in another group: out of scope.
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				other_group,
				Some(other_server),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.expect("record other-group backup");

		let sizes =
			BackupRun::snapshot_sizes_by_id(&mut conn, group_id, &["kopia-snap-1".to_string()])
				.await
				.expect("lookup");
		assert_eq!(sizes.get("kopia-snap-1"), Some(&42));
		assert_eq!(sizes.len(), 1);

		// Unknown ids resolve to nothing; the empty query short-circuits.
		let miss = BackupRun::snapshot_sizes_by_id(&mut conn, group_id, &["nope".to_string()])
			.await
			.expect("lookup miss");
		assert!(miss.is_empty());
		let none = BackupRun::snapshot_sizes_by_id(&mut conn, group_id, &[])
			.await
			.expect("empty lookup");
		assert!(none.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_sized_lists_only_runs_with_both_sizes() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;

		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.expect("record");

		// Reported size present but not yet inspected → not comparable.
		let map = BackupRun::latest_sized_by_machine_type_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert!(map.is_empty(), "no observed size yet → nothing to compare");

		// After inspection fills the observed size, the pair is comparable.
		BackupRun::backfill_snapshot_logical_bytes(
			&mut conn,
			group_id,
			&[("kopia-snap-1".into(), 1490)],
		)
		.await
		.expect("backfill");
		let map = BackupRun::latest_sized_by_machine_type_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(
			map.get(&(machine_id, BackupType::TamanuPostgres)),
			Some(&(42, 1490)),
			"reported 42 (new_run), observed 1490",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_success_ignores_restore_and_failure() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let pg = BackupType::TamanuPostgres;

		// Oldest: a real successful backup.
		let good = Uuid::new_v4();
		BackupRun::record(
			&mut conn,
			new_run(
				good,
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.unwrap();
		// Newer: a successful *restore* — must NOT reset backup staleness.
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Restore,
				RunOutcome::Success,
			),
		)
		.await
		.unwrap();
		// Newer still: a *failed* backup — also must not count.
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Failure,
			),
		)
		.await
		.unwrap();

		let latest = BackupRun::latest_success_for_machine(&mut conn, machine_id, &pg)
			.await
			.unwrap()
			.expect("a successful backup exists");
		assert_eq!(
			latest.id, good,
			"latest successful *backup* is the original, not the restore/failure"
		);

		let map = BackupRun::latest_success_by_machine_type_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(map.get(&(machine_id, pg)).map(|r| r.id), Some(good));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn s3_traffic_this_month_sums_raw_bytes_within_the_month_window() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let other_group_id = insert_group(&mut conn, "other").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;

		async fn insert_run_with_s3(
			conn: &mut AsyncPgConnection,
			group_id: Uuid,
			device_id: Uuid,
			machine_id: Uuid,
			sent: Option<i64>,
			received: Option<i64>,
			age: Option<SignedDuration>,
		) -> Uuid {
			let id = Uuid::new_v4();
			BackupRun::record(
				conn,
				NewBackupRun {
					id,
					device_id,
					group_id,
					machine_id: Some(machine_id),
					r#type: BackupType::TamanuPostgres,
					purpose: BackupPurpose::Backup,
					outcome: RunOutcome::Success,
					error: None,
					bytes_uploaded: Some(1),
					snapshot_id: None,
					s3_sent_raw_bytes: sent,
					s3_sent_payload_bytes: sent,
					s3_received_raw_bytes: received,
					s3_received_payload_bytes: received,
					snapshot_taken_at: None,
				},
			)
			.await
			.unwrap();
			if let Some(age) = age {
				sql_query(
					"UPDATE backup_runs SET reported_at = NOW() - ($2 || ' seconds')::INTERVAL \
					 WHERE id = $1",
				)
				.bind::<sql_types::Uuid, _>(id)
				.bind::<sql_types::Text, _>(age.as_secs().to_string())
				.execute(conn)
				.await
				.expect("backdate reported_at");
			}
			id
		}

		// Two runs this month → summed.
		insert_run_with_s3(
			&mut conn,
			group_id,
			device_id,
			machine_id,
			Some(100),
			Some(10),
			None,
		)
		.await;
		insert_run_with_s3(
			&mut conn,
			group_id,
			device_id,
			machine_id,
			Some(200),
			Some(20),
			None,
		)
		.await;
		// A run with no S3 tally at all (e.g. a backup type the proxy didn't
		// instrument) contributes nothing, not an error.
		insert_run_with_s3(&mut conn, group_id, device_id, machine_id, None, None, None).await;
		// A run reported last month is out of the window (40 days always crosses
		// a calendar-month boundary, regardless of today's date).
		insert_run_with_s3(
			&mut conn,
			group_id,
			device_id,
			machine_id,
			Some(9_999),
			Some(9_999),
			Some(SignedDuration::from_hours(24 * 40)),
		)
		.await;
		// A run in another group must not leak into this group's total.
		let other_server_id = insert_server(&mut conn, other_group_id).await;
		insert_run_with_s3(
			&mut conn,
			other_group_id,
			device_id,
			other_server_id,
			Some(5_000),
			Some(5_000),
			None,
		)
		.await;

		let (sent, received) = BackupRun::s3_traffic_this_month_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(sent, 300);
		assert_eq!(received, 30);

		// A group with no runs at all gets zeros, not an error.
		let empty_group_id = insert_group(&mut conn, "empty").await;
		let (sent, received) =
			BackupRun::s3_traffic_this_month_for_group(&mut conn, empty_group_id)
				.await
				.unwrap();
		assert_eq!(sent, 0);
		assert_eq!(received, 0);
	})
	.await;
}

// --- issuances --------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn issuance_snapshots_bucket_and_orders_newest_first() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let device_id = insert_device(&mut conn).await;

		let mk = |bucket: &str| NewBackupCredentialIssuance {
			device_id,
			group_id,
			r#type: BackupType::TamanuPostgres,
			expires_at: Timestamp::now() + SignedDuration::from_hours(1),
			purpose: BackupPurpose::Backup,
			sts_assumed_role: "arn:aws:iam::123456789012:role/canopy-backups-test".into(),
			sts_request_id: Some("req-1".into()),
			access_key_id: Some("ASIAEXAMPLE".into()),
			bucket: bucket.into(),
			prefix: String::new(),
			run_id: None,
		};

		let first = BackupCredentialIssuance::record(&mut conn, mk("bucket-at-issue"))
			.await
			.unwrap();
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		let _second = BackupCredentialIssuance::record(&mut conn, mk("later-bucket"))
			.await
			.unwrap();

		// The snapshot column holds what was passed, independent of any config.
		assert_eq!(first.bucket, "bucket-at-issue");

		let list = BackupCredentialIssuance::list_for_device(&mut conn, device_id, 10)
			.await
			.unwrap();
		assert_eq!(list.len(), 2);
		assert_eq!(list[0].bucket, "later-bucket", "newest first");
		assert_eq!(list[1].bucket, "bucket-at-issue");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_for_group_since_windows_and_orders() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let device_id = insert_device(&mut conn).await;

		let mk = || NewBackupCredentialIssuance {
			device_id,
			group_id,
			r#type: BackupType::TamanuPostgres,
			expires_at: Timestamp::now() + SignedDuration::from_hours(1),
			purpose: BackupPurpose::Restore,
			sts_assumed_role: "arn:aws:iam::123456789012:role/canopy-backups-test".into(),
			sts_request_id: None,
			access_key_id: Some("ASIAEXAMPLE".into()),
			bucket: "b".into(),
			prefix: String::new(),
			run_id: None,
		};
		BackupCredentialIssuance::record(&mut conn, mk())
			.await
			.unwrap();
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		BackupCredentialIssuance::record(&mut conn, mk())
			.await
			.unwrap();

		// A window that starts in the past captures both, newest-first.
		let recent = BackupCredentialIssuance::list_for_group_since(
			&mut conn,
			group_id,
			Timestamp::now() - SignedDuration::from_hours(1),
			10,
		)
		.await
		.unwrap();
		assert_eq!(recent.len(), 2);
		assert!(recent[0].issued_at >= recent[1].issued_at, "newest first");

		// A window that starts in the future excludes everything.
		let none = BackupCredentialIssuance::list_for_group_since(
			&mut conn,
			group_id,
			Timestamp::now() + SignedDuration::from_hours(1),
			10,
		)
		.await
		.unwrap();
		assert!(none.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn list_filtered_narrows_by_every_field() {
	TestDb::run(|mut conn, _url| async move {
		let group_a = insert_group(&mut conn, "a").await;
		let group_b = insert_group(&mut conn, "b").await;
		let server_a = insert_server(&mut conn, group_a).await;
		let server_b = insert_server(&mut conn, group_b).await;
		let device = insert_device(&mut conn).await;

		let pg_run = new_run(
			Uuid::new_v4(),
			device,
			group_a,
			Some(server_a),
			BackupPurpose::Backup,
			RunOutcome::Success,
		);
		BackupRun::record(&mut conn, pg_run).await.expect("pg run");

		let files_run_id = Uuid::new_v4();
		let mut files_run = new_run(
			files_run_id,
			device,
			group_a,
			Some(server_a),
			BackupPurpose::Backup,
			RunOutcome::Failure,
		);
		files_run.r#type = BackupType::from("files");
		BackupRun::record(&mut conn, files_run)
			.await
			.expect("files run");

		let other_group_run = new_run(
			Uuid::new_v4(),
			device,
			group_b,
			Some(server_b),
			BackupPurpose::Backup,
			RunOutcome::Success,
		);
		BackupRun::record(&mut conn, other_group_run)
			.await
			.expect("other group run");

		// No filters: all three.
		let all = BackupRun::list_filtered(&mut conn, BackupRunFilters::default(), 10)
			.await
			.unwrap();
		assert_eq!(all.len(), 3);

		// group_id.
		let for_a = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				group_id: Some(group_a),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(for_a.len(), 2);

		// machine_id.
		let for_server_b = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				machine_id: Some(server_b),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(for_server_b.len(), 1);
		assert_eq!(for_server_b[0].machine_id, Some(server_b));

		// type.
		let files_only = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				r#type: Some(BackupType::from("files")),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(files_only.len(), 1);
		assert_eq!(files_only[0].id, files_run_id);

		// outcome.
		let failures = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				outcome: Some(RunOutcome::Failure),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(failures.len(), 1);
		assert_eq!(failures[0].id, files_run_id);

		// since: a future cutoff excludes everything already recorded.
		let none = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				since: Some(Timestamp::now() + SignedDuration::from_hours(1)),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert!(none.is_empty());

		// since: a past cutoff keeps everything.
		let all_again = BackupRun::list_filtered(
			&mut conn,
			BackupRunFilters {
				since: Some(Timestamp::now() - SignedDuration::from_hours(1)),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(all_again.len(), 3);

		// limit caps the result set.
		let limited = BackupRun::list_filtered(&mut conn, BackupRunFilters::default(), 1)
			.await
			.unwrap();
		assert_eq!(limited.len(), 1);
	})
	.await;
}

// --- maintenance runs -------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn maintenance_start_finish_bracket() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;

		let id = BackupMaintenanceRun::start(&mut conn, group_id, MaintenanceKind::Full)
			.await
			.unwrap();
		let running = &BackupMaintenanceRun::list_for_group(&mut conn, group_id, 10)
			.await
			.unwrap()[0];
		assert_eq!(running.id, id);
		assert_eq!(running.kind, MaintenanceKind::Full);
		assert!(running.outcome.is_none(), "NULL outcome while running");
		assert!(running.finished_at.is_none());

		BackupMaintenanceRun::finish(&mut conn, id, RunOutcome::Success, None, Some(1024))
			.await
			.unwrap();
		let done = &BackupMaintenanceRun::list_for_group(&mut conn, group_id, 10)
			.await
			.unwrap()[0];
		assert_eq!(done.outcome, Some(RunOutcome::Success));
		assert_eq!(done.bytes_reclaimed, Some(1024));
		assert!(done.finished_at.is_some());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn maintenance_list_filtered_narrows_by_every_field() {
	TestDb::run(|mut conn, _url| async move {
		let group_a = insert_group(&mut conn, "a").await;
		let group_b = insert_group(&mut conn, "b").await;

		let running_id = BackupMaintenanceRun::start(&mut conn, group_a, MaintenanceKind::Quick)
			.await
			.unwrap();

		let failed_id = BackupMaintenanceRun::start(&mut conn, group_a, MaintenanceKind::Full)
			.await
			.unwrap();
		BackupMaintenanceRun::finish(
			&mut conn,
			failed_id,
			RunOutcome::Failure,
			Some("boom".into()),
			None,
		)
		.await
		.unwrap();

		let other_group_id =
			BackupMaintenanceRun::start(&mut conn, group_b, MaintenanceKind::Quick)
				.await
				.unwrap();
		BackupMaintenanceRun::finish(
			&mut conn,
			other_group_id,
			RunOutcome::Success,
			None,
			Some(512),
		)
		.await
		.unwrap();

		// No filters: all three.
		let all = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters::default(),
			10,
		)
		.await
		.unwrap();
		assert_eq!(all.len(), 3);

		// group_id.
		let for_a = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters {
				group_id: Some(group_a),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(for_a.len(), 2);

		// kind.
		let full_only = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters {
				kind: Some(MaintenanceKind::Full),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(full_only.len(), 1);
		assert_eq!(full_only[0].id, failed_id);

		// outcome=running.
		let running = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters {
				outcome: Some(MaintenanceOutcomeFilter::Running),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(running.len(), 1);
		assert_eq!(running[0].id, running_id);

		// outcome=failure.
		let failed = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters {
				outcome: Some(MaintenanceOutcomeFilter::Outcome(RunOutcome::Failure)),
				..Default::default()
			},
			10,
		)
		.await
		.unwrap();
		assert_eq!(failed.len(), 1);
		assert_eq!(failed[0].id, failed_id);

		// limit caps the result set.
		let limited = BackupMaintenanceRun::list_filtered(
			&mut conn,
			BackupMaintenanceRunFilters::default(),
			1,
		)
		.await
		.unwrap();
		assert_eq!(limited.len(), 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn maintenance_latest_successful_finished_at() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;

		// No runs at all → None.
		assert_eq!(
			BackupMaintenanceRun::latest_successful_finished_at_for_group(&mut conn, group_id)
				.await
				.unwrap(),
			None
		);

		// A failed run doesn't count.
		let failed = BackupMaintenanceRun::start(&mut conn, group_id, MaintenanceKind::Quick)
			.await
			.unwrap();
		BackupMaintenanceRun::finish(
			&mut conn,
			failed,
			RunOutcome::Failure,
			Some("boom".into()),
			None,
		)
		.await
		.unwrap();
		assert_eq!(
			BackupMaintenanceRun::latest_successful_finished_at_for_group(&mut conn, group_id)
				.await
				.unwrap(),
			None
		);

		// A successful run is picked up.
		let ok = BackupMaintenanceRun::start(&mut conn, group_id, MaintenanceKind::Full)
			.await
			.unwrap();
		BackupMaintenanceRun::finish(&mut conn, ok, RunOutcome::Success, None, Some(2048))
			.await
			.unwrap();
		let first_success =
			BackupMaintenanceRun::latest_successful_finished_at_for_group(&mut conn, group_id)
				.await
				.unwrap();
		assert!(first_success.is_some());

		// A later failure doesn't regress the "latest success" answer.
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		let failed_after = BackupMaintenanceRun::start(&mut conn, group_id, MaintenanceKind::Quick)
			.await
			.unwrap();
		BackupMaintenanceRun::finish(
			&mut conn,
			failed_after,
			RunOutcome::Failure,
			Some("boom".into()),
			None,
		)
		.await
		.unwrap();
		assert_eq!(
			BackupMaintenanceRun::latest_successful_finished_at_for_group(&mut conn, group_id)
				.await
				.unwrap(),
			first_success
		);

		// A newer success overtakes it.
		tokio::time::sleep(std::time::Duration::from_millis(10)).await;
		let ok2 = BackupMaintenanceRun::start(&mut conn, group_id, MaintenanceKind::Full)
			.await
			.unwrap();
		BackupMaintenanceRun::finish(&mut conn, ok2, RunOutcome::Success, None, Some(4096))
			.await
			.unwrap();
		let second_success =
			BackupMaintenanceRun::latest_successful_finished_at_for_group(&mut conn, group_id)
				.await
				.unwrap();
		assert!(second_success > first_success);
	})
	.await;
}

// --- repo snapshots ---------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn repo_snapshot_upsert_in_place() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let pg = BackupType::TamanuPostgres;
		let source = format!("canopy@{machine_id}:/data");

		// Fixed microsecond-precision timestamps: Postgres timestamptz truncates
		// to microseconds, so a nanosecond `Timestamp::now()` wouldn't round-trip
		// to an equal value.
		let t1: Timestamp = "2026-06-10T00:00:00.000001Z".parse().unwrap();
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&source,
			Some(machine_id),
			Some(&pg),
			Some(t1),
		)
		.await
		.unwrap();
		let rows = BackupRepoSnapshot::list_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(rows.len(), 1);
		assert_eq!(rows[0].latest_snapshot_at, Some(t1));
		assert_eq!(rows[0].r#type, Some(pg.clone()));

		// Second observation of the same (group, source) updates in place.
		let t2: Timestamp = "2026-06-11T12:34:56.654321Z".parse().unwrap();
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&source,
			Some(machine_id),
			Some(&pg),
			Some(t2),
		)
		.await
		.unwrap();
		let rows = BackupRepoSnapshot::list_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(rows.len(), 1, "still one row");
		assert_eq!(rows[0].latest_snapshot_at, Some(t2));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn latest_backup_and_last_inspected_for_group() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;

		// Nothing reported / inspected yet.
		assert!(
			BackupRun::latest_backup_at_for_group(&mut conn, group_id)
				.await
				.unwrap()
				.is_none()
		);
		assert!(
			BackupRepoSnapshot::last_inspected_at_for_group(&mut conn, group_id)
				.await
				.unwrap()
				.is_none()
		);

		// A failed backup and a successful restore don't count as a backup.
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Failure,
			),
		)
		.await
		.unwrap();
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Restore,
				RunOutcome::Success,
			),
		)
		.await
		.unwrap();
		assert!(
			BackupRun::latest_backup_at_for_group(&mut conn, group_id)
				.await
				.unwrap()
				.is_none(),
			"only successful backups count"
		);

		// A successful backup → its reported_at.
		BackupRun::record(
			&mut conn,
			new_run(
				Uuid::new_v4(),
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			),
		)
		.await
		.unwrap();
		let runs = BackupRun::list_for_group(&mut conn, group_id, 10)
			.await
			.unwrap();
		let success = runs
			.iter()
			.find(|r| r.purpose == BackupPurpose::Backup && r.outcome == RunOutcome::Success)
			.unwrap();
		assert_eq!(
			BackupRun::latest_backup_at_for_group(&mut conn, group_id)
				.await
				.unwrap(),
			Some(success.reported_at)
		);

		// last-inspected comes from the (inspection-only) snapshots table.
		let t: Timestamp = "2026-06-10T00:00:00.000001Z".parse().unwrap();
		BackupRepoSnapshot::upsert(
			&mut conn,
			group_id,
			&format!("canopy@{machine_id}:/data"),
			Some(machine_id),
			Some(&BackupType::TamanuPostgres),
			Some(t),
		)
		.await
		.unwrap();
		let snaps = BackupRepoSnapshot::list_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(
			BackupRepoSnapshot::last_inspected_at_for_group(&mut conn, group_id)
				.await
				.unwrap(),
			Some(snaps[0].observed_at)
		);
	})
	.await;
}

// --- repo stats: two independent writers ------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn repo_stats_split_writers_do_not_clobber() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;

		// Inspection writer first, then bucket-bytes writer.
		BackupRepoStats::upsert_repo_fields(
			&mut conn,
			group_id,
			Some(10),
			Some(3),
			Some(1000),
			Some(500),
		)
		.await
		.unwrap();
		BackupRepoStats::upsert_bucket_bytes(&mut conn, group_id, Some(777))
			.await
			.unwrap();
		let s = BackupRepoStats::get(&mut conn, group_id)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(
			(
				s.snapshot_count,
				s.source_count,
				s.logical_bytes,
				s.physical_bytes,
				s.bucket_bytes
			),
			(Some(10), Some(3), Some(1000), Some(500), Some(777))
		);

		let repo_observed_at = s.observed_at;
		let bucket_observed_at = s.bucket_bytes_observed_at.unwrap();

		// Bucket-bytes writer must not clobber the repo fields, and must bump
		// only its own timestamp — not the inspection one.
		BackupRepoStats::upsert_bucket_bytes(&mut conn, group_id, Some(888))
			.await
			.unwrap();
		let s = BackupRepoStats::get(&mut conn, group_id)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(s.bucket_bytes, Some(888));
		assert_eq!(s.snapshot_count, Some(10), "repo fields preserved");
		assert_eq!(s.observed_at, repo_observed_at, "observed_at untouched");
		assert!(
			s.bucket_bytes_observed_at.unwrap() >= bucket_observed_at,
			"bucket_bytes_observed_at bumped"
		);
		let bucket_observed_at = s.bucket_bytes_observed_at.unwrap();

		// And the inspection writer must not clobber bucket_bytes or its
		// timestamp.
		BackupRepoStats::upsert_repo_fields(
			&mut conn,
			group_id,
			Some(11),
			Some(3),
			Some(1100),
			Some(550),
		)
		.await
		.unwrap();
		let s = BackupRepoStats::get(&mut conn, group_id)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(s.snapshot_count, Some(11));
		assert_eq!(s.bucket_bytes, Some(888), "bucket_bytes preserved");
		assert_eq!(
			s.bucket_bytes_observed_at,
			Some(bucket_observed_at),
			"bucket_bytes_observed_at preserved"
		);
	})
	.await;
}

// --- capabilities -----------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn capability_register_seeds_once_then_operator_controls() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let pg = BackupType::TamanuPostgres;

		// First registration seeds enabled from the type default.
		let cap = MachineBackupCapability::register(&mut conn, machine_id, &pg, true)
			.await
			.unwrap();
		assert!(cap.enabled);

		// Operator turns it off.
		MachineBackupCapability::set_enabled(&mut conn, machine_id, &pg, false)
			.await
			.unwrap();

		// Re-registration must NOT re-seed enabled — operator's choice sticks.
		let cap = MachineBackupCapability::register(&mut conn, machine_id, &pg, true)
			.await
			.unwrap();
		assert!(!cap.enabled, "re-register keeps the operator-set enabled");

		assert!(
			MachineBackupCapability::list_enabled(&mut conn)
				.await
				.unwrap()
				.is_empty()
		);
		MachineBackupCapability::set_enabled(&mut conn, machine_id, &pg, true)
			.await
			.unwrap();
		assert_eq!(
			MachineBackupCapability::list_enabled(&mut conn)
				.await
				.unwrap()
				.len(),
			1
		);
	})
	.await;
}

// --- schedule ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn schedule_and_retention_are_set_and_cleared_apart() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let pg = BackupType::TamanuPostgres;
		let every_12h = Schedule::Interval {
			seconds: SignedDuration::from_hours(12).as_secs(),
		};

		ServerGroupBackupSchedule::set_schedule(&mut conn, group_id, &pg, &every_12h)
			.await
			.unwrap();
		let got = ServerGroupBackupSchedule::get(&mut conn, group_id, &pg)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(got.schedule(), Some(every_12h.clone()));
		assert!(got.retention.is_none(), "retention is inherited");

		// Setting retention leaves the schedule alone.
		ServerGroupBackupSchedule::set_retention(&mut conn, group_id, &pg, &retention(), false)
			.await
			.unwrap();
		let got = ServerGroupBackupSchedule::get(&mut conn, group_id, &pg)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(got.schedule(), Some(every_12h.clone()));
		assert!(got.retention.is_some());

		// Clearing the schedule leaves the retention override on the row.
		ServerGroupBackupSchedule::clear_schedule(&mut conn, group_id, &pg)
			.await
			.unwrap();
		let got = ServerGroupBackupSchedule::get(&mut conn, group_id, &pg)
			.await
			.unwrap()
			.expect("a retention override keeps the row");
		assert_eq!(got.schedule(), None);
		assert!(got.retention.is_some());

		// Clearing that too leaves nothing, so the row goes.
		ServerGroupBackupSchedule::clear_retention(&mut conn, group_id, &pg)
			.await
			.unwrap();
		assert!(
			ServerGroupBackupSchedule::get(&mut conn, group_id, &pg)
				.await
				.unwrap()
				.is_none()
		);

		// A retention-only row doesn't override the schedule, and clearing the
		// retention removes it.
		ServerGroupBackupSchedule::set_retention(&mut conn, group_id, &pg, &retention(), false)
			.await
			.unwrap();
		let got = ServerGroupBackupSchedule::get(&mut conn, group_id, &pg)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(got.schedule(), None);
		ServerGroupBackupSchedule::clear_retention(&mut conn, group_id, &pg)
			.await
			.unwrap();
		assert!(
			ServerGroupBackupSchedule::list_for_group(&mut conn, group_id)
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await;
}

// --- requests ---------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn requests_enqueue_clear_list() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let pg = BackupType::TamanuPostgres;

		BackupRequest::enqueue(
			&mut conn,
			machine_id,
			&pg,
			BackupPurpose::Backup,
			Some("op@bes"),
		)
		.await
		.unwrap();
		// Re-enqueue is an upsert on (server, type, purpose) — still one row.
		BackupRequest::enqueue(
			&mut conn,
			machine_id,
			&pg,
			BackupPurpose::Backup,
			Some("op2@bes"),
		)
		.await
		.unwrap();
		let pending = BackupRequest::pending_for_machine(&mut conn, machine_id)
			.await
			.unwrap();
		assert_eq!(pending.len(), 1);
		assert_eq!(pending[0].requested_by.as_deref(), Some("op2@bes"));

		// A different purpose is a distinct row.
		BackupRequest::enqueue(&mut conn, machine_id, &pg, BackupPurpose::Restore, None)
			.await
			.unwrap();
		assert_eq!(
			BackupRequest::pending_for_machine(&mut conn, machine_id)
				.await
				.unwrap()
				.len(),
			2
		);

		BackupRequest::clear(&mut conn, machine_id, &pg, BackupPurpose::Backup)
			.await
			.unwrap();
		let pending = BackupRequest::pending_for_machine(&mut conn, machine_id)
			.await
			.unwrap();
		assert_eq!(pending.len(), 1);
		assert_eq!(pending[0].purpose, BackupPurpose::Restore);
	})
	.await;
}

// --- backup_run_progress ----------------------------------------------------

fn new_progress(
	run_id: Uuid,
	device_id: Uuid,
	group_id: Uuid,
	machine_id: Uuid,
	bytes_uploaded: Option<i64>,
) -> NewBackupRunProgress {
	NewBackupRunProgress {
		run_id,
		device_id,
		group_id,
		machine_id: Some(machine_id),
		r#type: BackupType::TamanuPostgres,
		purpose: BackupPurpose::Backup,
		snapshot_taken_at: None,
		bytes_read: None,
		bytes_hashed: None,
		bytes_uploaded,
		bytes_cached: None,
		bytes_estimated: None,
		files_done: None,
		files_estimated: None,
		errors: None,
		ignored_errors: None,
		current_path: None,
		s3_sent_raw_bytes: None,
		s3_sent_payload_bytes: None,
		s3_received_raw_bytes: None,
		s3_received_payload_bytes: None,
		extra: serde_json::json!({}),
	}
}

/// A sample can be recorded for a run that has no `backup_runs` row — which is
/// the normal case, since the run row only exists once the run finishes.
#[tokio::test(flavor = "multi_thread")]
async fn progress_records_without_a_run_row() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let run_id = Uuid::new_v4();

		BackupRunProgress::record(
			&mut conn,
			new_progress(run_id, device_id, group_id, machine_id, Some(10)),
		)
		.await
		.expect("a sample for an unknown run must be accepted");

		let latest = BackupRunProgress::latest_for_run(&mut conn, run_id)
			.await
			.unwrap()
			.expect("sample stored");
		assert_eq!(latest.bytes_uploaded, Some(10));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_series_is_oldest_first_and_latest_is_newest() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let run_id = Uuid::new_v4();

		for uploaded in [1_i64, 2, 3] {
			BackupRunProgress::record(
				&mut conn,
				new_progress(run_id, device_id, group_id, machine_id, Some(uploaded)),
			)
			.await
			.unwrap();
		}

		let series = BackupRunProgress::series_for_run(&mut conn, run_id)
			.await
			.unwrap();
		assert_eq!(
			series.iter().map(|p| p.bytes_uploaded).collect::<Vec<_>>(),
			vec![Some(1), Some(2), Some(3)],
		);

		let latest = BackupRunProgress::latest_for_run(&mut conn, run_id)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(latest.bytes_uploaded, Some(3));
	})
	.await;
}

/// The write-once subtlety: a device announces the freeze moment once, early, and
/// omits it thereafter — so it must not be read off the latest sample.
#[tokio::test(flavor = "multi_thread")]
async fn earliest_snapshot_moment_ignores_later_null_samples() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let run_id = Uuid::new_v4();
		let taken: Timestamp = "2026-07-01T04:12:00Z".parse().unwrap();

		BackupRunProgress::record(
			&mut conn,
			NewBackupRunProgress {
				snapshot_taken_at: Some(taken),
				..new_progress(run_id, device_id, group_id, machine_id, Some(1))
			},
		)
		.await
		.unwrap();
		BackupRunProgress::record(
			&mut conn,
			new_progress(run_id, device_id, group_id, machine_id, Some(2)),
		)
		.await
		.unwrap();

		assert_eq!(
			BackupRunProgress::latest_for_run(&mut conn, run_id)
				.await
				.unwrap()
				.unwrap()
				.snapshot_taken_at,
			None,
			"the latest sample carries no moment — that's the trap",
		);
		assert_eq!(
			BackupRunProgress::earliest_snapshot_taken_at_for_run(&mut conn, run_id)
				.await
				.unwrap(),
			Some(taken),
		);

		let batch = BackupRunProgress::earliest_snapshot_taken_at_by_run(&mut conn, &[run_id])
			.await
			.unwrap();
		assert_eq!(batch.get(&run_id).copied(), Some(taken));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_batch_loaders_key_by_run_and_short_circuit_empty() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let (run_a, run_b) = (Uuid::new_v4(), Uuid::new_v4());

		for (run, uploaded) in [(run_a, 5_i64), (run_a, 9), (run_b, 100)] {
			BackupRunProgress::record(
				&mut conn,
				new_progress(run, device_id, group_id, machine_id, Some(uploaded)),
			)
			.await
			.unwrap();
		}

		let latest = BackupRunProgress::latest_by_run(&mut conn, &[run_a, run_b])
			.await
			.unwrap();
		assert_eq!(latest.get(&run_a).and_then(|p| p.bytes_uploaded), Some(9));
		assert_eq!(latest.get(&run_b).and_then(|p| p.bytes_uploaded), Some(100));

		let windows = BackupRunProgress::for_runs_since(
			&mut conn,
			&[run_a, run_b],
			Timestamp::now() - SignedDuration::from_hours(1),
		)
		.await
		.unwrap();
		assert_eq!(windows.get(&run_a).map(Vec::len), Some(2));
		assert_eq!(windows.get(&run_b).map(Vec::len), Some(1));

		// No ids → no query, empty result.
		assert!(
			BackupRunProgress::latest_by_run(&mut conn, &[])
				.await
				.unwrap()
				.is_empty()
		);
		assert!(
			BackupRunProgress::for_runs_since(&mut conn, &[], Timestamp::now())
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn progress_prune_deletes_only_past_the_cutoff() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let (old_run, fresh_run) = (Uuid::new_v4(), Uuid::new_v4());

		let old = BackupRunProgress::record(
			&mut conn,
			new_progress(old_run, device_id, group_id, machine_id, Some(1)),
		)
		.await
		.unwrap();
		BackupRunProgress::record(
			&mut conn,
			new_progress(fresh_run, device_id, group_id, machine_id, Some(2)),
		)
		.await
		.unwrap();

		// Backdate the first sample well past any plausible retention.
		sql_query(
			"UPDATE backup_run_progress SET observed_at = now() - interval '30 days' WHERE id = $1",
		)
		.bind::<sql_types::BigInt, _>(old.id)
		.execute(&mut conn)
		.await
		.unwrap();

		let deleted = BackupRunProgress::prune_before(
			&mut conn,
			Timestamp::now() - SignedDuration::from_hours(14 * 24),
		)
		.await
		.unwrap();
		assert_eq!(deleted, 1);

		assert!(
			BackupRunProgress::latest_for_run(&mut conn, old_run)
				.await
				.unwrap()
				.is_none()
		);
		assert!(
			BackupRunProgress::latest_for_run(&mut conn, fresh_run)
				.await
				.unwrap()
				.is_some(),
			"a fresh sample must survive pruning",
		);
	})
	.await;
}

// --- staleness anchor -------------------------------------------------------

/// Record a success whose report is `reported_age` old and whose data was frozen
/// `taken_age` before now (when given).
async fn insert_success_with_moments(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
	group_id: Uuid,
	machine_id: Uuid,
	reported_age: SignedDuration,
	taken_age: Option<SignedDuration>,
) -> Uuid {
	let id = Uuid::new_v4();
	BackupRun::record(
		conn,
		NewBackupRun {
			snapshot_taken_at: taken_age.map(|a| Timestamp::now() - a),
			..new_run(
				id,
				device_id,
				group_id,
				Some(machine_id),
				BackupPurpose::Backup,
				RunOutcome::Success,
			)
		},
	)
	.await
	.unwrap();
	sql_query(
		"UPDATE backup_runs SET reported_at = now() - ($2 || ' seconds')::INTERVAL WHERE id = $1",
	)
	.bind::<sql_types::Uuid, _>(id)
	.bind::<sql_types::Text, _>(reported_age.as_secs().to_string())
	.execute(conn)
	.await
	.unwrap();
	id
}

/// `anchor()` is the data's own moment, so a backup that took hours to upload is
/// as old as what it captured — not as young as its report.
#[tokio::test(flavor = "multi_thread")]
async fn anchor_prefers_the_freeze_moment_over_the_report() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let pg = BackupType::TamanuPostgres;

		// Reported an hour ago, but the data was frozen 22 hours ago — a long run.
		insert_success_with_moments(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			SignedDuration::from_hours(1),
			Some(SignedDuration::from_hours(22)),
		)
		.await;

		let run = BackupRun::latest_success_for_machine(&mut conn, machine_id, &pg)
			.await
			.unwrap()
			.unwrap();
		let age = Timestamp::now().duration_since(run.anchor());
		assert!(
			age > SignedDuration::from_hours(21),
			"anchor must reflect the freeze moment (age was {age:?})",
		);
	})
	.await;
}

/// A run that reports no freeze moment behaves exactly as before — this is what
/// makes the change a no-op for clients that don't send it.
#[tokio::test(flavor = "multi_thread")]
async fn anchor_falls_back_to_the_report_time() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let pg = BackupType::TamanuPostgres;

		insert_success_with_moments(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			SignedDuration::from_hours(3),
			None,
		)
		.await;

		let run = BackupRun::latest_success_for_machine(&mut conn, machine_id, &pg)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(run.anchor(), run.reported_at);
	})
	.await;
}

/// The reason selection and measure must use the same expression: a newer report
/// carrying *older* data must not become the anchor, or a server's freshness
/// would travel backwards as runs arrive.
#[tokio::test(flavor = "multi_thread")]
async fn latest_success_selects_by_data_age_not_report_order() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "g").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		let device_id = insert_device(&mut conn).await;
		let pg = BackupType::TamanuPostgres;

		// A: reported 2h ago, data frozen 3h ago — the fresher *data*.
		let a = insert_success_with_moments(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			SignedDuration::from_hours(2),
			Some(SignedDuration::from_hours(3)),
		)
		.await;
		// B: reported 1h ago (later), but data frozen 20h ago — staler data.
		let b = insert_success_with_moments(
			&mut conn,
			device_id,
			group_id,
			machine_id,
			SignedDuration::from_hours(1),
			Some(SignedDuration::from_hours(20)),
		)
		.await;

		let picked = BackupRun::latest_success_for_machine(&mut conn, machine_id, &pg)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(
			picked.id, a,
			"must pick the run with the newest data, not the newest report (b={b})",
		);

		let map = BackupRun::latest_success_by_machine_type_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(
			map.get(&(machine_id, pg)).map(|r| r.id),
			Some(a),
			"the batch loader must agree with the single-server query",
		);
	})
	.await;
}

// --- participation follows the machine --------------------------------------

/// Add another workload to a box that already carries one.
async fn add_application(conn: &mut AsyncPgConnection, group_id: Uuid, machine_id: Uuid) {
	let host = format!("http://test.invalid/{}", Uuid::new_v4());
	sql_query(
		"INSERT INTO applications (host, type, group_id, machine_id) \
		 VALUES ($1, 'tamanu-facility', $2, $3)",
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group_id)
	.bind::<sql_types::Uuid, _>(machine_id)
	.execute(conn)
	.await
	.expect("insert application");
}

#[tokio::test(flavor = "multi_thread")]
async fn capability_and_participation_follow_the_machine() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "kiritimati").await;
		let machine_id = insert_server(&mut conn, group_id).await;
		add_application(&mut conn, group_id, machine_id).await;

		let pg = BackupType::TamanuPostgres;
		let files = BackupType::Custom("tamanu-files".into());
		MachineBackupCapability::register(&mut conn, machine_id, &pg, true)
			.await
			.unwrap();
		MachineBackupCapability::register(&mut conn, machine_id, &files, false)
			.await
			.unwrap();

		let caps = MachineBackupCapability::list_for_machine(&mut conn, machine_id)
			.await
			.unwrap();
		assert_eq!(
			caps.len(),
			2,
			"the box declares its own types, not one set per workload",
		);

		let enabled = MachineBackupCapability::enabled_types_for_group(&mut conn, group_id)
			.await
			.unwrap();
		assert_eq!(
			enabled,
			vec![pg.clone()],
			"a two-application box is one participant, so its enabled type appears once",
		);

		let mut declared = MachineBackupCapability::declared_types_for_group(&mut conn, group_id)
			.await
			.unwrap();
		declared.sort_by(|a, b| a.as_str().cmp(b.as_str()));
		assert_eq!(
			declared,
			vec![files, pg],
			"every declared type reaches the group exactly once",
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_device_request_resolves_identity_to_machine_to_group() {
	TestDb::run(|mut conn, _url| async move {
		let group_id = insert_group(&mut conn, "rarotonga").await;

		// A box carrying no workload at all: resolution must still land on it.
		let machine_id =
			sql_query("INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id")
				.bind::<sql_types::Uuid, _>(group_id)
				.get_result::<RowId>(&mut conn)
				.await
				.expect("insert machine")
				.id;
		let device_id = insert_device(&mut conn).await;
		database::Machine::bind_device(&mut conn, machine_id, device_id)
			.await
			.unwrap();

		let resolved = database::Machine::get_by_device_id(&mut conn, device_id)
			.await
			.unwrap()
			.expect("the identity resolves to the box it is enrolled as");
		assert_eq!(resolved.id, machine_id);
		assert_eq!(
			resolved.group_id,
			Some(group_id),
			"the group comes from the machine, not from an application on it",
		);

		// The request enqueued against that machine is the one the box sees.
		let pg = BackupType::TamanuPostgres;
		BackupRequest::enqueue(&mut conn, machine_id, &pg, BackupPurpose::Backup, None)
			.await
			.unwrap();
		let pending = BackupRequest::pending_for_machine(&mut conn, resolved.id)
			.await
			.unwrap();
		assert_eq!(pending.len(), 1);

		// An identity bound to no machine resolves to nothing, rather than to
		// some application's box.
		let stray = insert_device(&mut conn).await;
		assert!(
			database::Machine::get_by_device_id(&mut conn, stray)
				.await
				.unwrap()
				.is_none(),
		);
	})
	.await;
}
