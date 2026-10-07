//! The operator endpoints for backup schedules and what they drive.
//!
//! Spec: BKO (scheduling, editing schedules), SAFE (grades).

use commons_servers::backup_jobs::backups_due_now_for_machine;
use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use jiff::Timestamp;
use serde_json::json;
use uuid::Uuid;

/// A group with a ready backup configuration and one box with a `tamanu-postgres`
/// capability enabled. Returns `(group, machine)`.
async fn ready_group_with_box(conn: &mut impl SimpleAsyncConnection) -> (Uuid, Uuid) {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group}', 'grp-{group}'); \
		 INSERT INTO machines (id, name, group_id) VALUES ('{machine}', 'box', '{group}'); \
		 INSERT INTO server_group_backup_config \
			(group_id, bucket, prefix, target_role_arn, maintenance_role_arn, region, repo_password_ref, status) \
			VALUES ('{group}', 'b', '', 'arn:aws:iam::123456789012:role/t', \
				'arn:aws:iam::123456789012:role/m', 'ap-southeast-2', 'ref', 'ready'); \
		 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) \
			VALUES ('{machine}', 'tamanu-postgres', true);"
	))
	.await
	.expect("seed");
	(group, machine)
}

fn ts(s: &str) -> Timestamp {
	s.parse().unwrap()
}

/// What the schedulers tell a machine to back up at `now`.
async fn due(conn: &mut AsyncPgConnection, group: Uuid, machine: Uuid, now: &str) -> Vec<String> {
	backups_due_now_for_machine(conn, machine, group, ts(now))
		.await
		.unwrap()
		.into_iter()
		.map(|t| t.to_string())
		.collect()
}

/// A machine override beats the group's and the fleet's, and both the group
/// view and the machine's own say which layer is in force; the change is
/// recorded with who made it.
// spec: BKO#scheduling
#[tokio::test(flavor = "multi_thread")]
async fn machine_overrides_are_set_cleared_and_recorded() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, machine) = ready_group_with_box(&mut conn).await;

		private
			.post("/api/backups/set_schedule")
			.json(&json!({
				"server_group_id": group,
				"type": "tamanu-postgres",
				"schedule": {"kind": "interval", "seconds": 7200},
			}))
			.await
			.assert_status_ok();
		private
			.post("/api/backups/set_machine_schedule")
			.json(&json!({
				"machine_id": machine,
				"type": "tamanu-postgres",
				"schedule": {"kind": "cron", "expression": "0 3 * * *", "zone": "Asia/Tokyo"},
			}))
			.await
			.assert_status_ok();

		let caps: serde_json::Value = private
			.post("/api/backups/capabilities")
			.json(&json!({ "machine_id": machine }))
			.await
			.json();
		let cap = &caps[0];
		assert_eq!(cap["schedule"]["layer"], "machine");
		assert_eq!(cap["schedule"]["schedule"]["expression"], "0 3 * * *");
		assert_eq!(cap["schedule"]["zone"]["name"], "Asia/Tokyo");
		assert!(cap["next_backup"]["kind"].is_string());

		// The group view carries the same row for the box.
		let stats: serde_json::Value = private
			.post("/api/backups/stats")
			.json(&json!({ "server_group_id": group }))
			.await
			.json();
		assert_eq!(stats["capabilities"][0]["schedule"]["layer"], "machine");

		// The change is in the machine layer's history, with who.
		let history: serde_json::Value = private
			.post("/api/backups/schedule_history")
			.json(&json!({"layer": "machine", "type": "tamanu-postgres", "machine_id": machine}))
			.await
			.json();
		assert_eq!(history.as_array().unwrap().len(), 1);
		assert_eq!(history[0]["schedule"]["kind"], "cron");
		assert!(history[0]["changed_by"].is_string());
		assert!(history[0]["changed_at"].is_string());

		// Clearing falls back to the group's schedule, and is recorded too.
		private
			.post("/api/backups/clear_machine_schedule")
			.json(&json!({"machine_id": machine, "type": "tamanu-postgres"}))
			.await
			.assert_status_ok();
		let caps: serde_json::Value = private
			.post("/api/backups/capabilities")
			.json(&json!({ "machine_id": machine }))
			.await
			.json();
		assert_eq!(caps[0]["schedule"]["layer"], "group");
		assert_eq!(caps[0]["schedule"]["schedule"]["seconds"], 7200);
		let history: serde_json::Value = private
			.post("/api/backups/schedule_history")
			.json(&json!({"layer": "machine", "type": "tamanu-postgres", "machine_id": machine}))
			.await
			.json();
		assert_eq!(history.as_array().unwrap().len(), 2);
		assert!(history[0]["schedule"].is_null(), "newest is the clear");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_override_that_cannot_run_is_refused_and_unknown_machines_are_404() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (_, machine) = ready_group_with_box(&mut conn).await;
		private
			.post("/api/backups/set_machine_schedule")
			.json(&json!({
				"machine_id": machine,
				"type": "tamanu-postgres",
				"schedule": {"kind": "cron", "expression": "*/5 * * * *"},
			}))
			.await
			.assert_status_bad_request();
		private
			.post("/api/backups/set_machine_schedule")
			.json(&json!({
				"machine_id": Uuid::new_v4(),
				"type": "tamanu-postgres",
				"schedule": {"kind": "manual"},
			}))
			.await
			.assert_status_not_found();
		private
			.post("/api/backups/schedule_history")
			.json(&json!({"layer": "group", "type": "tamanu-postgres"}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

/// Clearing a group's schedule leaves its retention override, and clearing
/// retention leaves the schedule.
// spec: BKO#scheduling, SAFE
#[tokio::test(flavor = "multi_thread")]
async fn a_groups_schedule_and_retention_overrides_are_cleared_apart() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, _) = ready_group_with_box(&mut conn).await;
		let retention = json!({
			"keep_latest": 1, "keep_daily": 9, "keep_weekly": 4, "keep_monthly": 6, "keep_annual": 0
		});
		private
			.post("/api/backups/set_schedule")
			.json(&json!({
				"server_group_id": group, "type": "tamanu-postgres",
				"schedule": {"kind": "interval", "seconds": 3600},
			}))
			.await
			.assert_status_ok();
		private
			.post("/api/backups/set_retention")
			.json(&json!({
				"server_group_id": group, "type": "tamanu-postgres", "retention": retention,
			}))
			.await
			.assert_status_ok();

		let view = |body: serde_json::Value| body["schedules"][0].clone();
		let body: serde_json::Value = private
			.post("/api/backups/clear_schedule")
			.json(&json!({"server_group_id": group, "type": "tamanu-postgres"}))
			.await
			.json();
		let row = view(body);
		assert!(row["schedule"].is_null());
		assert_eq!(row["retention"]["keep_daily"], 9, "retention stays");

		let sched: serde_json::Value = private
			.post("/api/backups/group_schedules")
			.json(&json!({"server_group_id": group}))
			.await
			.json();
		assert_eq!(
			sched[0]["layer"], "fleet",
			"the schedule is inherited again"
		);
		assert_eq!(sched[0]["has_retention_override"], true);

		private
			.post("/api/backups/set_schedule")
			.json(&json!({
				"server_group_id": group, "type": "tamanu-postgres",
				"schedule": {"kind": "manual"},
			}))
			.await
			.assert_status_ok();
		let body: serde_json::Value = private
			.post("/api/backups/clear_retention")
			.json(&json!({"server_group_id": group, "type": "tamanu-postgres"}))
			.await
			.json();
		let row = view(body);
		assert_eq!(row["schedule"]["kind"], "manual", "the schedule stays");
		assert!(row["retention"].is_null());
	})
	.await;
}

/// The preview says why an expression would be refused, as it is typed.
// spec: BKO#editing-schedules
#[tokio::test(flavor = "multi_thread")]
async fn the_preview_says_why_an_expression_is_refused() {
	commons_tests::server::run(async |_conn, _public, private| {
		for (expression, reason) in [
			("*/30 * * * *", "less than an hour"),
			("0 0 30 2 *", "never fires"),
			("0 2 * * * UTC", "timezone"),
			("H/15 * * * *", "`H`"),
			("not a cron", "five fields"),
		] {
			let preview: serde_json::Value = private
				.post("/api/backups/schedule_preview")
				.json(&json!({
					"type": "tamanu-postgres",
					"schedule": {"kind": "cron", "expression": expression},
				}))
				.await
				.json();
			let refusal = preview["refusal"]
				.as_str()
				.unwrap_or_else(|| panic!("{expression}: {preview}"));
			assert!(refusal.contains(reason), "{expression}: {refusal}");
			assert!(preview["machines"].as_array().unwrap().is_empty());
		}
	})
	.await;
}

/// The preview lists the next firings, one machine to a distinct zone for a
/// group's schedule, with `H` resolved per machine and a UTC fallback flagged.
// spec: BKO#editing-schedules
#[tokio::test(flavor = "multi_thread")]
async fn the_preview_lists_firings_per_distinct_zone() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, a) = ready_group_with_box(&mut conn).await;
		let b = Uuid::new_v4();
		let c = Uuid::new_v4();
		let d = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id) VALUES \
				('{b}', 'b-box', '{group}'), ('{c}', 'c-box', '{group}'), ('{d}', 'd-box', '{group}'); \
			 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) VALUES \
				('{b}', 'tamanu-postgres', true), ('{c}', 'tamanu-postgres', true), \
				('{d}', 'tamanu-postgres', true); \
			 INSERT INTO machine_reported_timezone (machine_id, timezone) VALUES \
				('{a}', 'Pacific/Auckland'), ('{b}', 'Pacific/Auckland'), ('{c}', 'Australia/Sydney'); \
			 INSERT INTO machine_backup_schedule (machine_id, type, expected_interval) \
				VALUES ('{d}', 'tamanu-postgres', INTERVAL '6 hours');"
		))
		.await
		.unwrap();
		// `d` has its own override, so the group's schedule doesn't apply to it.

		let preview: serde_json::Value = private
			.post("/api/backups/schedule_preview")
			.json(&json!({
				"type": "tamanu-postgres",
				"schedule": {"kind": "cron", "expression": "H H * * *"},
				"group_id": group,
				"count": 3,
			}))
			.await
			.json();
		assert!(preview["refusal"].is_null());
		let machines = preview["machines"].as_array().unwrap();
		let zones: Vec<&str> = machines
			.iter()
			.map(|m| m["zone"]["name"].as_str().unwrap())
			.collect();
		assert_eq!(
			zones,
			["Australia/Sydney", "Pacific/Auckland"],
			"one per distinct zone"
		);
		for m in machines {
			assert_eq!(m["firings"].as_array().unwrap().len(), 3);
			assert_eq!(m["zone"]["source"], "machine");
		}
		// Auckland's is the first of its machines by name ("b-box" before "box").
		let auckland = machines
			.iter()
			.find(|m| m["zone"]["name"] == "Pacific/Auckland")
			.unwrap();
		assert_eq!(auckland["machine_id"], b.to_string());
		assert_eq!(auckland["machine_name"], "b-box");
		let _ = a;

		// A machine override is previewed for that machine alone; one with no
		// reported zone reads in UTC and is flagged.
		let preview: serde_json::Value = private
			.post("/api/backups/schedule_preview")
			.json(&json!({
				"type": "tamanu-postgres",
				"schedule": {"kind": "cron", "expression": "0 2 * * *"},
				"machine_id": d,
			}))
			.await
			.json();
		let machines = preview["machines"].as_array().unwrap();
		assert_eq!(machines.len(), 1);
		assert_eq!(machines[0]["machine_id"], d.to_string());
		assert_eq!(machines[0]["zone"]["name"], "UTC");
		assert_eq!(machines[0]["zone"]["source"], "unreported_utc");

		// A schedule with a zone of its own is that zone for everyone.
		let preview: serde_json::Value = private
			.post("/api/backups/schedule_preview")
			.json(&json!({
				"type": "tamanu-postgres",
				"schedule": {"kind": "cron", "expression": "0 2 * * *", "zone": "Asia/Tokyo"},
				"group_id": group,
			}))
			.await
			.json();
		let machines = preview["machines"].as_array().unwrap();
		assert_eq!(machines.len(), 1);
		assert_eq!(machines[0]["zone"]["source"], "schedule");
		assert_eq!(machines[0]["firings"].as_array().unwrap().len(), 5);
	})
	.await;
}

// ---------------------------------------------------------------------------
// What the schedulers are told
// ---------------------------------------------------------------------------

/// A fleet default of nightly at 2am backs each machine up at its own 2am, from
/// when the window opens until halfway to the next firing, and a window that
/// closes unmet is not caught up.
// spec: BKO#when-a-backup-is-due
#[tokio::test(flavor = "multi_thread")]
async fn a_cron_default_is_due_in_each_machines_own_window() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, auckland) = ready_group_with_box(&mut conn).await;
		let sydney = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id) VALUES ('{sydney}', 'syd', '{group}'); \
			 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) \
				VALUES ('{sydney}', 'tamanu-postgres', true); \
			 INSERT INTO machine_reported_timezone (machine_id, timezone, changed_at) VALUES \
				('{auckland}', 'Pacific/Auckland', '2020-01-01T00:00:00Z'), \
				('{sydney}', 'Australia/Sydney', '2020-01-01T00:00:00Z');"
		))
		.await
		.unwrap();
		// A fleet default, hashed so each box's window opens on the firing.
		private
			.post("/api/backups/set_type_default")
			.json(&json!({
				"type": "tamanu-postgres",
				"default_schedule": {"kind": "cron", "expression": "0 2 * * *"},
				"default_retention": {
					"keep_latest": 1, "keep_daily": 7, "keep_weekly": 4, "keep_monthly": 6, "keep_annual": 0
				},
			}))
			.await
			.assert_status_ok();
		// Whatever the clock said when it was set, it took effect long ago.
		conn.batch_execute("UPDATE backup_schedule_history SET changed_at = '2020-01-01T00:00:00Z'")
			.await
			.unwrap();

		// Mid-January: Auckland is on NZDT (+13), Sydney on AEDT (+11). 02:00 in
		// Auckland is 13:00Z the day before; in Sydney, 15:00Z.
		// 14:00Z: Auckland's 03:00, inside its window; Sydney's is 01:00, before.
		assert_eq!(due(&mut conn, group, auckland, "2030-01-14T14:00:00Z").await, ["tamanu-postgres"]);
		assert!(due(&mut conn, group, sydney, "2030-01-14T14:00:00Z").await.is_empty());
		// 16:00Z: Sydney's 03:00 now, and Auckland's 05:00 is still in window.
		assert_eq!(due(&mut conn, group, sydney, "2030-01-14T16:00:00Z").await, ["tamanu-postgres"]);
		assert_eq!(due(&mut conn, group, auckland, "2030-01-14T16:00:00Z").await, ["tamanu-postgres"]);
		// 22:00Z is 11:00 in Auckland (window closes halfway to the next, at
		// 14:00 local): still open. 03:00Z the next day is 16:00 local: closed,
		// and not caught up later.
		assert_eq!(due(&mut conn, group, auckland, "2030-01-14T22:00:00Z").await, ["tamanu-postgres"]);
		assert!(due(&mut conn, group, auckland, "2030-01-15T03:00:00Z").await.is_empty());

		// A success since the firing ends the window.
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{0}', 'machine'); \
			 INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, reported_at) \
				VALUES ('{1}', '{0}', '{group}', '{auckland}', 'tamanu-postgres', 'backup', 'success', '2030-01-14T14:30:00Z');",
			Uuid::new_v4(),
			Uuid::new_v4()
		))
		.await
		.unwrap();
		assert!(due(&mut conn, group, auckland, "2030-01-14T16:00:00Z").await.is_empty());
	})
	.await;
}

/// Changing a schedule must not make a backup due at a time neither schedule
/// chose: from nightly 2am to 6am at 14:00, nothing is due until 06:00 tomorrow.
// spec: BKO#when-a-backup-is-due
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_schedule_is_not_due_for_firings_it_never_had() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, machine) = ready_group_with_box(&mut conn).await;
		private
			.post("/api/backups/set_schedule")
			.json(&json!({
				"server_group_id": group, "type": "tamanu-postgres",
				"schedule": {"kind": "cron", "expression": "0 6 * * *", "zone": "UTC"},
			}))
			.await
			.assert_status_ok();
		// The change was made at 14:00 on the 14th, from the seeded 6 hourly
		// interval.
		conn.batch_execute(
			"UPDATE backup_schedule_history SET changed_at = '2030-01-14T14:00:00Z' WHERE layer = 'group'",
		)
		.await
		.unwrap();

		// 06:00 today has passed but predates the change, so it opens no window.
		assert!(
			due(&mut conn, group, machine, "2030-01-14T14:30:00Z")
				.await
				.is_empty()
		);
		assert!(
			due(&mut conn, group, machine, "2030-01-15T05:59:00Z")
				.await
				.is_empty()
		);
		// The next 06:00 is the schedule's own.
		assert_eq!(
			due(&mut conn, group, machine, "2030-01-15T06:30:00Z").await,
			["tamanu-postgres"]
		);
	})
	.await;
}

/// An interval is due once the interval has passed since the last success, as
/// it was before cron, and manual-only never is.
// spec: BKO#when-a-backup-is-due
#[tokio::test(flavor = "multi_thread")]
async fn interval_and_manual_only_are_due_as_they_were() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let (group, machine) = ready_group_with_box(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{0}', 'machine'); \
			 INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, reported_at) \
				VALUES ('{1}', '{0}', '{group}', '{machine}', 'tamanu-postgres', 'backup', 'success', '2030-01-14T00:00:00Z');",
			Uuid::new_v4(),
			Uuid::new_v4()
		))
		.await
		.unwrap();
		// The seeded fleet default is every six hours.
		assert!(due(&mut conn, group, machine, "2030-01-14T05:00:00Z").await.is_empty());
		assert_eq!(due(&mut conn, group, machine, "2030-01-14T06:00:00Z").await, ["tamanu-postgres"]);

		private
			.post("/api/backups/set_machine_schedule")
			.json(&json!({"machine_id": machine, "type": "tamanu-postgres", "schedule": {"kind": "manual"}}))
			.await
			.assert_status_ok();
		assert!(due(&mut conn, group, machine, "2030-01-20T00:00:00Z").await.is_empty());
	})
	.await;
}
