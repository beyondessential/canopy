//! Where a machine's backup schedule comes from and when it took effect.
//!
//! Spec: BKO (scheduling, when a backup is due) and BKJ (detection).

use commons_tests::db::TestDb;
use commons_types::backup::{
	BackupConfigStatus, BackupType,
	schedule::{EffectiveSchedule, Schedule, ScheduleLayer, ZoneSource},
};
use database::{
	BackupTypeDefault, MachineBackupCapability, NewBackupTypeDefault, ServerGroupBackupConfig,
	ServerGroupBackupSchedule,
	backup::{
		schedules::{self, LayerKey, MachineReportedTimezone, ScheduleBook, ScheduleChange},
		staleness::{Prior, ScanRow, StalenessVerdict},
	},
	diesel_async::AsyncPgConnection,
	pg_duration::PgDuration,
	reported_detail::MachineReportedDetail,
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::{SignedDuration, Timestamp};
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

fn pg() -> BackupType {
	BackupType::TamanuPostgres
}

fn ts(s: &str) -> Timestamp {
	s.parse().unwrap()
}

fn every(hours: i64) -> Schedule {
	Schedule::Interval {
		seconds: SignedDuration::from_hours(hours).as_secs(),
	}
}

fn nightly(zone: Option<&str>) -> Schedule {
	Schedule::Cron {
		expression: "0 2 * * *".into(),
		zone: zone.map(Into::into),
	}
}

async fn insert_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	sql_query("INSERT INTO server_groups (name) VALUES ($1) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.get_result::<RowId>(conn)
		.await
		.unwrap()
		.id
}

async fn insert_machine(conn: &mut AsyncPgConnection, group_id: Uuid, name: &str) -> Uuid {
	sql_query("INSERT INTO machines (name, group_id) VALUES ($1, $2) RETURNING id")
		.bind::<sql_types::Text, _>(name)
		.bind::<sql_types::Uuid, _>(group_id)
		.get_result::<RowId>(conn)
		.await
		.unwrap()
		.id
}

fn new_default(schedule: &Schedule) -> NewBackupTypeDefault {
	let (interval, cron, zone) = schedule.to_columns();
	NewBackupTypeDefault {
		r#type: pg(),
		default_interval: interval.map(|s| PgDuration(SignedDuration::from_secs(s))),
		default_cron: cron,
		default_zone: zone,
		default_retention: serde_json::json!({"keep_daily": 7, "keep_weekly": 4, "keep_monthly": 6}),
		auto_enable: false,
		allow_below_floor: false,
	}
}

async fn book(conn: &mut AsyncPgConnection) -> ScheduleBook {
	ScheduleBook::load(conn, None, None).await.unwrap()
}

/// A history entry at a chosen moment, which the API cannot do: it stamps now.
#[allow(clippy::too_many_arguments)]
async fn history_at(
	conn: &mut AsyncPgConnection,
	layer: &str,
	group: Option<Uuid>,
	machine: Option<Uuid>,
	schedule: Option<&Schedule>,
	at: &str,
) {
	let (kind, interval, cron, zone) = match schedule {
		None => (None, None, None, None),
		Some(s) => {
			let (i, c, z) = s.to_columns();
			let kind = match s {
				Schedule::Manual => "manual",
				Schedule::Interval { .. } => "interval",
				Schedule::Cron { .. } => "cron",
			};
			(Some(kind), i, c, z)
		}
	};
	sql_query(
		"INSERT INTO backup_schedule_history \
		 (layer, type, group_id, machine_id, kind, interval, cron, zone, changed_at) \
		 VALUES ($1, 'tamanu-postgres', $2, $3, $4, ($5 || ' seconds')::INTERVAL, $6, $7, $8::timestamptz)",
	)
	.bind::<sql_types::Text, _>(layer)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(group)
	.bind::<sql_types::Nullable<sql_types::Uuid>, _>(machine)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(kind)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(interval.map(|i| i.to_string()))
	.bind::<sql_types::Nullable<sql_types::Text>, _>(cron)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(zone)
	.bind::<sql_types::Text, _>(at)
	.execute(conn)
	.await
	.unwrap();
}

// ---------------------------------------------------------------------------
// Resolution
// ---------------------------------------------------------------------------

/// The machine's override wins over its group's, which wins over the fleet
/// default; clearing one falls back to the next, and the layer in force is
/// named.
// spec: BKO#scheduling
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_override_beats_a_group_override_beats_the_fleet_default() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;

		// A type with nothing set anywhere is manual-only, from no layer.
		let none = book(&mut conn)
			.await
			.resolve(machine, Some(group), &BackupType::from("files"));
		assert_eq!(none.schedule, Schedule::Manual);
		assert_eq!(none.layer, None);

		// The migration seeds the fleet default for this one.
		schedules::upsert_default(&mut conn, new_default(&every(6)), Some("a@x"))
			.await
			.unwrap();
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			(r.schedule, r.layer),
			(every(6), Some(ScheduleLayer::Fleet))
		);

		schedules::set_group_schedule(&mut conn, group, &pg(), &every(12), Some("a@x"))
			.await
			.unwrap();
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			(r.schedule, r.layer),
			(every(12), Some(ScheduleLayer::Group))
		);

		schedules::set_machine_schedule(&mut conn, machine, &pg(), &nightly(None), Some("a@x"))
			.await
			.unwrap();
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			(r.schedule, r.layer),
			(nightly(None), Some(ScheduleLayer::Machine))
		);

		schedules::clear_machine_schedule(&mut conn, machine, &pg(), Some("a@x"))
			.await
			.unwrap();
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			(r.schedule, r.layer),
			(every(12), Some(ScheduleLayer::Group))
		);

		schedules::clear_group_schedule(&mut conn, group, &pg(), Some("a@x"))
			.await
			.unwrap();
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			(r.schedule, r.layer),
			(every(6), Some(ScheduleLayer::Fleet))
		);
	})
	.await;
}

/// An override replaces the one beneath whole: a machine override of manual-only
/// is manual-only, not the group's schedule.
// spec: BKO#scheduling
#[tokio::test(flavor = "multi_thread")]
async fn an_override_of_manual_only_is_manual_only() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;
		schedules::set_group_schedule(&mut conn, group, &pg(), &every(6), None)
			.await
			.unwrap();
		schedules::set_machine_schedule(&mut conn, machine, &pg(), &Schedule::Manual, None)
			.await
			.unwrap();
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(r.schedule, Schedule::Manual);
		assert_eq!(r.layer, Some(ScheduleLayer::Machine));
	})
	.await;
}

/// A machine's override is the machine's: it follows the machine to another
/// group, and survives its capability being disabled and enabled again.
// spec: BKO#scheduling
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_override_survives_a_group_move_and_a_disabled_type() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let other = insert_group(&mut conn, "h").await;
		let machine = insert_machine(&mut conn, group, "box").await;
		MachineBackupCapability::register(&mut conn, machine, &pg(), true)
			.await
			.unwrap();
		schedules::set_machine_schedule(&mut conn, machine, &pg(), &nightly(None), None)
			.await
			.unwrap();

		sql_query("UPDATE machines SET group_id = $1 WHERE id = $2")
			.bind::<sql_types::Uuid, _>(other)
			.bind::<sql_types::Uuid, _>(machine)
			.execute(&mut conn)
			.await
			.unwrap();
		MachineBackupCapability::set_enabled(&mut conn, machine, &pg(), false)
			.await
			.unwrap();
		MachineBackupCapability::set_enabled(&mut conn, machine, &pg(), true)
			.await
			.unwrap();

		let r = book(&mut conn).await.resolve(machine, Some(other), &pg());
		assert_eq!(r.schedule, nightly(None));
		assert_eq!(r.layer, Some(ScheduleLayer::Machine));
	})
	.await;
}

/// Every set and clear is recorded with who and when; setting what is already
/// there is not a change.
// spec: BKO#scheduling
#[tokio::test(flavor = "multi_thread")]
async fn every_change_to_a_layer_is_recorded_with_who() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;

		schedules::set_machine_schedule(&mut conn, machine, &pg(), &every(6), Some("ann@x"))
			.await
			.unwrap();
		schedules::set_machine_schedule(&mut conn, machine, &pg(), &every(6), Some("bob@x"))
			.await
			.unwrap();
		schedules::set_machine_schedule(&mut conn, machine, &pg(), &nightly(None), Some("bob@x"))
			.await
			.unwrap();
		schedules::clear_machine_schedule(&mut conn, machine, &pg(), Some("cat@x"))
			.await
			.unwrap();
		// Clearing what is not set changes nothing.
		schedules::clear_machine_schedule(&mut conn, machine, &pg(), Some("cat@x"))
			.await
			.unwrap();

		let changes = ScheduleChange::list_for_layer(&mut conn, LayerKey::Machine(machine), &pg())
			.await
			.unwrap();
		let summary: Vec<(Option<Schedule>, Option<String>)> = changes
			.iter()
			.map(|c| (c.schedule(), c.changed_by.clone()))
			.collect();
		assert_eq!(
			summary,
			vec![
				(None, Some("cat@x".into())),
				(Some(nightly(None)), Some("bob@x".into())),
				(Some(every(6)), Some("ann@x".into())),
			],
			"newest first"
		);

		// Another layer's changes are not this layer's.
		let group_changes =
			ScheduleChange::list_for_layer(&mut conn, LayerKey::Group(group), &pg())
				.await
				.unwrap();
		assert!(group_changes.is_empty());
	})
	.await;
}

/// The fleet default's history records a change to its schedule, but not an
/// edit that leaves the schedule alone, such as to its retention.
// spec: BKO#scheduling
#[tokio::test(flavor = "multi_thread")]
async fn the_fleet_default_records_schedule_changes_only() {
	TestDb::run(|mut conn, _url| async move {
		let before = ScheduleChange::list_for_layer(&mut conn, LayerKey::Fleet, &pg())
			.await
			.unwrap()
			.len();
		schedules::upsert_default(&mut conn, new_default(&nightly(Some("UTC"))), Some("ann@x"))
			.await
			.unwrap();
		let mut same = new_default(&nightly(Some("UTC")));
		same.auto_enable = true;
		schedules::upsert_default(&mut conn, same, Some("bob@x"))
			.await
			.unwrap();
		let after = ScheduleChange::list_for_layer(&mut conn, LayerKey::Fleet, &pg())
			.await
			.unwrap();
		assert_eq!(after.len(), before + 1);
		assert_eq!(after[0].changed_by.as_deref(), Some("ann@x"));
	})
	.await;
}

/// A refused schedule writes nothing.
// spec: BKO#cron-expressions
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_schedule_is_not_stored() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;
		for bad in [
			Schedule::Interval { seconds: 600 },
			Schedule::Cron {
				expression: "*/30 * * * *".into(),
				zone: None,
			},
			Schedule::Cron {
				expression: "0 2 * * *".into(),
				zone: Some("Nope/Nowhere".into()),
			},
		] {
			assert!(
				schedules::set_machine_schedule(&mut conn, machine, &pg(), &bad, None)
					.await
					.is_err(),
				"{bad:?}"
			);
		}
		assert!(
			ScheduleChange::list_for_layer(&mut conn, LayerKey::Machine(machine), &pg())
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await;
}

// ---------------------------------------------------------------------------
// When a schedule took effect
// ---------------------------------------------------------------------------

/// The moment a schedule took effect is the latest history entry on the
/// resolution path that changed what it resolves to. A change to a layer that
/// is shadowed doesn't count, and clearing the layer that shadows does.
// spec: BKO#when-a-backup-is-due
#[tokio::test(flavor = "multi_thread")]
async fn a_schedule_took_effect_when_its_resolution_last_changed() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;

		// Current rows, with history written at controlled moments.
		BackupTypeDefault::upsert(&mut conn, new_default(&every(6)))
			.await
			.unwrap();
		sql_query("DELETE FROM backup_schedule_history")
			.execute(&mut conn)
			.await
			.unwrap();
		history_at(
			&mut conn,
			"fleet",
			None,
			None,
			Some(&every(6)),
			"1970-01-01T00:00:00Z",
		)
		.await;

		// 1. The group sets a cron schedule: it took effect then.
		let nightly_nz = nightly(Some("Pacific/Auckland"));
		ServerGroupBackupSchedule::set_schedule(&mut conn, group, &pg(), &nightly_nz)
			.await
			.unwrap();
		history_at(
			&mut conn,
			"group",
			Some(group),
			None,
			Some(&nightly_nz),
			"2026-10-01T00:00:00Z",
		)
		.await;
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(r.since, Some(ts("2026-10-01T00:00:00Z")));

		// 2. The fleet default changes beneath it: shadowed, so no move.
		history_at(
			&mut conn,
			"fleet",
			None,
			None,
			Some(&every(8)),
			"2026-10-02T00:00:00Z",
		)
		.await;
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			r.since,
			Some(ts("2026-10-01T00:00:00Z")),
			"a shadowed layer"
		);

		// 3. The machine overrides with the very same schedule: it resolves to
		// the same thing, so it didn't take effect anew.
		sql_query(
			"INSERT INTO machine_backup_schedule (machine_id, type, expected_cron, schedule_zone) \
			 VALUES ($1, 'tamanu-postgres', '0 2 * * *', 'Pacific/Auckland')",
		)
		.bind::<sql_types::Uuid, _>(machine)
		.execute(&mut conn)
		.await
		.unwrap();
		history_at(
			&mut conn,
			"machine",
			None,
			Some(machine),
			Some(&nightly_nz),
			"2026-10-03T00:00:00Z",
		)
		.await;
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(
			r.since,
			Some(ts("2026-10-01T00:00:00Z")),
			"same resolved schedule"
		);

		// 4. The machine moves to another time: that is a change.
		let six = Schedule::Cron {
			expression: "0 6 * * *".into(),
			zone: Some("Pacific/Auckland".into()),
		};
		sql_query(
			"UPDATE machine_backup_schedule SET expected_cron = '0 6 * * *' WHERE machine_id = $1",
		)
		.bind::<sql_types::Uuid, _>(machine)
		.execute(&mut conn)
		.await
		.unwrap();
		history_at(
			&mut conn,
			"machine",
			None,
			Some(machine),
			Some(&six),
			"2026-10-04T00:00:00Z",
		)
		.await;
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(r.since, Some(ts("2026-10-04T00:00:00Z")));

		// 5. Clearing the machine override falls back to the group's: the
		// resolved schedule changed, so that counts.
		sql_query("DELETE FROM machine_backup_schedule WHERE machine_id = $1")
			.bind::<sql_types::Uuid, _>(machine)
			.execute(&mut conn)
			.await
			.unwrap();
		history_at(
			&mut conn,
			"machine",
			None,
			Some(machine),
			None,
			"2026-10-05T00:00:00Z",
		)
		.await;
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(r.since, Some(ts("2026-10-05T00:00:00Z")));
	})
	.await;
}

/// A schedule read in the machine's zone takes effect anew when the machine
/// reports a different zone; one with a zone of its own doesn't.
// spec: BKO#when-a-backup-is-due
#[tokio::test(flavor = "multi_thread")]
async fn a_new_reported_zone_moves_when_a_machine_zoned_schedule_took_effect() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;
		BackupTypeDefault::upsert(&mut conn, new_default(&nightly(None)))
			.await
			.unwrap();
		sql_query("DELETE FROM backup_schedule_history").execute(&mut conn).await.unwrap();
		history_at(&mut conn, "fleet", None, None, Some(&nightly(None)), "2026-09-01T00:00:00Z").await;

		sql_query("INSERT INTO machine_reported_timezone (machine_id, timezone, changed_at) VALUES ($1, 'Pacific/Auckland', '2026-10-06T00:00:00Z')")
			.bind::<sql_types::Uuid, _>(machine)
			.execute(&mut conn)
			.await
			.unwrap();

		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(r.zone.as_ref().unwrap().name, "Pacific/Auckland");
		assert_eq!(r.zone.as_ref().unwrap().source, ZoneSource::Machine);
		assert_eq!(
			r.since,
			Some(ts("2026-10-06T00:00:00Z")),
			"the zone changed after the schedule was set"
		);

		// So a machine reporting a new zone isn't due from a firing before the
		// change: nightly 2am in Auckland was 13:00Z on the 5th, but the zone
		// changed at 00:00Z on the 6th, so that firing opens no window.
		let seed = commons_types::backup::schedule::schedule_seed(machine, "tamanu-postgres");
		assert!(!r.is_due(seed, ts("2026-10-06T00:30:00Z"), None));
		assert!(r.is_due(seed, ts("2026-10-06T14:00:00Z"), None), "the 6th's own firing");

		// The same schedule with a zone of its own is unmoved by the machine.
		let pinned = nightly(Some("Asia/Tokyo"));
		BackupTypeDefault::upsert(&mut conn, new_default(&pinned))
			.await
			.unwrap();
		history_at(&mut conn, "fleet", None, None, Some(&pinned), "2026-09-02T00:00:00Z").await;
		let r = book(&mut conn).await.resolve(machine, Some(group), &pg());
		assert_eq!(r.zone.as_ref().unwrap().source, ZoneSource::Schedule);
		assert_eq!(r.since, Some(ts("2026-09-02T00:00:00Z")));
	})
	.await;
}

/// A machine that reports no timezone, or one nobody recognises, reads a cron
/// schedule in UTC and says so; a Windows zone name is read as its IANA one.
// spec: BKO#cron-expressions
#[tokio::test(flavor = "multi_thread")]
async fn the_zone_falls_back_to_utc_and_says_why() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let silent = insert_machine(&mut conn, group, "silent").await;
		let odd = insert_machine(&mut conn, group, "odd").await;
		let windows = insert_machine(&mut conn, group, "windows").await;
		BackupTypeDefault::upsert(&mut conn, new_default(&nightly(None)))
			.await
			.unwrap();
		MachineReportedTimezone::observe(&mut conn, odd, "Middle/Earth")
			.await
			.unwrap();
		MachineReportedTimezone::observe(&mut conn, windows, "New Zealand Standard Time")
			.await
			.unwrap();

		let book = book(&mut conn).await;
		let zone = |m| book.resolve(m, Some(group), &pg()).zone.unwrap();
		assert_eq!(zone(silent).source, ZoneSource::UnreportedUtc);
		assert_eq!(zone(silent).name, "UTC");
		assert_eq!(zone(odd).source, ZoneSource::UnrecognisedUtc);
		assert_eq!(zone(windows).name, "Pacific/Auckland");
		assert_eq!(zone(windows).source, ZoneSource::Machine);
	})
	.await;
}

/// The moment a machine's timezone changed moves only when it is a different
/// zone, and is recorded as the report comes in.
// spec: BKO#when-a-backup-is-due
#[tokio::test(flavor = "multi_thread")]
async fn a_report_moves_the_zone_change_only_when_the_zone_is_different() {
	TestDb::run(|mut conn, _url| async move {
		let group = insert_group(&mut conn, "g").await;
		let machine = insert_machine(&mut conn, group, "box").await;

		MachineReportedDetail::record(
			&mut conn,
			machine,
			"alertd",
			&serde_json::json!({"osTimezone": "Pacific/Auckland", "hostname": "h"}),
		)
		.await
		.unwrap();
		let first = MachineReportedTimezone::get(&mut conn, machine)
			.await
			.unwrap()
			.expect("recorded");
		assert_eq!(first.timezone, "Pacific/Auckland");

		// Repeating itself, or reporting without the field, moves nothing.
		sql_query("UPDATE machine_reported_timezone SET changed_at = '2020-01-01T00:00:00Z'")
			.execute(&mut conn)
			.await
			.unwrap();
		for body in [
			serde_json::json!({"osTimezone": "Pacific/Auckland"}),
			serde_json::json!({"hostname": "h"}),
		] {
			MachineReportedDetail::record(&mut conn, machine, "alertd", &body)
				.await
				.unwrap();
		}
		let held = MachineReportedTimezone::get(&mut conn, machine)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(held.changed_at, ts("2020-01-01T00:00:00Z"));
		assert_eq!(held.timezone, "Pacific/Auckland");

		// A different zone does.
		MachineReportedDetail::record(
			&mut conn,
			machine,
			"alertd",
			&serde_json::json!({"osTimezone": "Australia/Sydney"}),
		)
		.await
		.unwrap();
		let moved = MachineReportedTimezone::get(&mut conn, machine)
			.await
			.unwrap()
			.unwrap();
		assert_eq!(moved.timezone, "Australia/Sydney");
		assert!(moved.changed_at > ts("2020-01-01T00:00:00Z"));
	})
	.await;
}

// ---------------------------------------------------------------------------
// The scan and staleness
// ---------------------------------------------------------------------------

async fn ready_group_with_box(conn: &mut AsyncPgConnection) -> (Uuid, Uuid) {
	let group = insert_group(conn, &format!("g-{}", Uuid::new_v4())).await;
	let machine = insert_machine(conn, group, "box").await;
	ServerGroupBackupConfig::upsert(
		conn,
		database::NewServerGroupBackupConfig {
			group_id: group,
			bucket: "b".into(),
			prefix: String::new(),
			target_role_arn: "arn:aws:iam::123456789012:role/t".into(),
			maintenance_role_arn: "arn:aws:iam::123456789012:role/m".into(),
			region: None,
			repo_password_ref: "ref".into(),
			status: BackupConfigStatus::Provisioning,
			mode: commons_types::backup::BackupRepoMode::FromBirth,
			placement: commons_types::backup::BackupPlacement::External,
		},
	)
	.await
	.unwrap();
	ServerGroupBackupConfig::set_status(conn, group, BackupConfigStatus::Ready)
		.await
		.unwrap();
	MachineBackupCapability::register(conn, machine, &pg(), true)
		.await
		.unwrap();
	MachineBackupCapability::set_enabled(conn, machine, &pg(), true)
		.await
		.unwrap();
	(group, machine)
}

/// A machine's own override decides what the scan expects of it: cron is
/// scanned, and a machine override of manual-only takes it out, whatever its
/// group does.
// spec: BKJ#detection
#[tokio::test(flavor = "multi_thread")]
async fn the_scan_follows_the_machines_own_schedule() {
	TestDb::run(|mut conn, _url| async move {
		let (group, machine) = ready_group_with_box(&mut conn).await;
		let (_, quiet) = {
			let m = insert_machine(&mut conn, group, "quiet").await;
			MachineBackupCapability::register(&mut conn, m, &pg(), true)
				.await
				.unwrap();
			MachineBackupCapability::set_enabled(&mut conn, m, &pg(), true)
				.await
				.unwrap();
			(group, m)
		};
		schedules::set_group_schedule(&mut conn, group, &pg(), &nightly(Some("UTC")), None)
			.await
			.unwrap();
		schedules::set_machine_schedule(&mut conn, quiet, &pg(), &Schedule::Manual, None)
			.await
			.unwrap();

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.unwrap();
		let mine = rows
			.iter()
			.find(|r| r.machine_id == machine)
			.expect("scanned");
		assert_eq!(mine.schedule.schedule, nightly(Some("UTC")));
		assert_eq!(mine.schedule.layer, Some(ScheduleLayer::Group));
		assert!(
			!rows.iter().any(|r| r.machine_id == quiet),
			"a machine override of manual-only is not expected to back up"
		);
	})
	.await;
}

fn cron_row(since: Option<&str>, last_success: Option<&str>) -> ScanRow {
	ScanRow {
		machine_id: Uuid::from_u128(7),
		group_id: Uuid::nil(),
		device_id: None,
		r#type: pg(),
		is_monitored: true,
		schedule: EffectiveSchedule {
			schedule: nightly(Some("UTC")),
			layer: Some(ScheduleLayer::Fleet),
			zone: Some(commons_types::backup::schedule::resolve_zone(
				Some("UTC"),
				None,
			)),
			since: since.map(ts),
		},
		config_created_at: ts("2026-09-01T00:00:00Z"),
		machine_registered_at: None,
		last_success_at: last_success.map(ts),
	}
}

/// Under cron, one missed firing is not stale and two consecutive ones are.
// spec: BKJ#detection
#[test]
fn under_cron_one_missed_firing_is_not_stale_and_two_are() {
	let row = cron_row(None, Some("2026-10-03T02:10:00Z"));
	// By 03:00 on the 6th the 4th and 5th windows have closed unmet.
	assert_eq!(
		row.classify(ts("2026-10-06T03:00:00Z"), false),
		StalenessVerdict::Stale
	);
	// By 03:00 on the 5th only the 4th has.
	assert_eq!(
		row.classify(ts("2026-10-05T03:00:00Z"), false),
		StalenessVerdict::Ok
	);
}

/// Firings from before the schedule took effect don't count.
// spec: BKJ#detection
#[test]
fn firings_before_the_schedule_took_effect_do_not_count() {
	let row = cron_row(Some("2026-10-05T12:00:00Z"), Some("2026-10-01T02:10:00Z"));
	// The 5th's firing predates the change; only the 6th's has closed.
	assert_eq!(
		row.classify(ts("2026-10-06T20:00:00Z"), false),
		StalenessVerdict::Ok
	);
	assert_eq!(
		row.classify(ts("2026-10-07T20:00:00Z"), false),
		StalenessVerdict::Stale
	);
}

/// A machine already stale when a cron schedule takes effect stays stale until
/// it backs up, whatever the new schedule's firings say.
// spec: BKJ#detection
#[test]
fn a_machine_already_stale_when_a_cron_schedule_takes_effect_stays_stale() {
	let took_effect = "2026-10-06T00:00:00Z";
	let row = cron_row(Some(took_effect), Some("2026-10-01T02:10:00Z"));
	let now = ts("2026-10-06T01:00:00Z");
	assert_eq!(
		row.classify(now, false),
		StalenessVerdict::Ok,
		"no firing to judge it by"
	);
	let carried = Prior {
		stale: true,
		never: false,
	};
	assert_eq!(
		row.classify_after(now, true, carried),
		StalenessVerdict::Stale
	);

	// A backup since the schedule took effect ends the carry-over.
	let backed_up = cron_row(Some(took_effect), Some("2026-10-06T00:30:00Z"));
	assert_eq!(
		backed_up.classify_after(now, true, carried),
		StalenessVerdict::Recovered
	);

	// The same holds for one that never backed up.
	let never = cron_row(Some(took_effect), None);
	assert_eq!(
		never.classify_after(
			now,
			true,
			Prior {
				stale: false,
				never: true
			}
		),
		StalenessVerdict::Never
	);
}

/// A machine that has never backed up is stale once two firings have closed
/// since its expectation began.
// spec: BKJ#detection
#[test]
fn a_machine_that_never_backed_up_is_stale_after_two_closed_firings() {
	let mut row = cron_row(None, None);
	row.config_created_at = ts("2026-10-05T12:00:00Z");
	assert_eq!(
		row.classify(ts("2026-10-06T20:00:00Z"), false),
		StalenessVerdict::Ok
	);
	assert_eq!(
		row.classify(ts("2026-10-07T20:00:00Z"), false),
		StalenessVerdict::Never
	);
}

/// An interval is judged as it always was: stale at twice the interval.
// spec: BKJ#detection
#[test]
fn an_interval_is_stale_at_twice_the_interval() {
	let mut row = cron_row(None, Some("2026-10-06T00:00:00Z"));
	row.schedule.schedule = every(12);
	assert_eq!(
		row.classify(ts("2026-10-06T23:00:00Z"), false),
		StalenessVerdict::Ok
	);
	assert_eq!(
		row.classify(ts("2026-10-07T01:00:00Z"), false),
		StalenessVerdict::Stale
	);
}

async fn open_never_findings(conn: &mut AsyncPgConnection, machine: Uuid) -> i64 {
	#[derive(QueryableByName)]
	struct N {
		#[diesel(sql_type = sql_types::BigInt)]
		n: i64,
	}
	sql_query(
		"SELECT count(*) AS n FROM issues WHERE machine_id = $1 \
		 AND ref = 'backup-never' AND active AND resolved_at IS NULL",
	)
	.bind::<sql_types::Uuid, _>(machine)
	.get_result::<N>(conn)
	.await
	.unwrap()
	.n
}

/// Setting a type to manual-only ends an open staleness finding: a manual-only
/// type is never stale.
// spec: BKJ#detection
#[tokio::test(flavor = "multi_thread")]
async fn going_manual_only_resolves_an_open_staleness_finding() {
	TestDb::run(|mut conn, _url| async move {
		let (group, machine) = ready_group_with_box(&mut conn).await;
		schedules::set_group_schedule(&mut conn, group, &pg(), &every(6), None)
			.await
			.unwrap();
		sql_query("UPDATE server_group_backup_config SET created_at = now() - interval '3 days'")
			.execute(&mut conn)
			.await
			.unwrap();

		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.unwrap();
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.unwrap();
		assert_eq!(
			open_never_findings(&mut conn, machine).await,
			1,
			"never backed up for three days"
		);

		schedules::set_group_schedule(&mut conn, group, &pg(), &Schedule::Manual, None)
			.await
			.unwrap();
		let rows = database::backup::staleness::scan_rows(&mut conn)
			.await
			.unwrap();
		assert!(rows.is_empty());
		database::backup::staleness::sweep(&mut conn, &rows)
			.await
			.unwrap();
		assert_eq!(
			open_never_findings(&mut conn, machine).await,
			0,
			"manual-only is never stale"
		);
	})
	.await;
}
