//! Recording where an environment is going, through the operator API, and
//! reading the fleet view that surfaces it.

use commons_tests::diesel_async::SimpleAsyncConnection;
use serde_json::{Value, json};

const GROUP: &str = "cccccccc-0000-0000-0000-000000000001";
const UNPLANNED: &str = "cccccccc-0000-0000-0000-000000000002";

#[tokio::test(flavor = "multi_thread")]
async fn record_then_the_fleet_view_shows_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published'),
				('cccccccc-0000-0000-0000-0000000000f2', 2, 63, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka'),
				('cccccccc-0000-0000-0000-000000000002', 'no-plan');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001'),
				('cccccccc-0000-0000-0000-0000000000a2', 'cccccccc-0000-0000-0000-000000000002');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1'),
				('cccccccc-0000-0000-0000-0000000000a2', 'https://no-plan.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000002', 'cccccccc-0000-0000-0000-0000000000a2');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0'),
				('cccccccc-0000-0000-0000-0000000000a2', 'test', '{}'::jsonb, '2.60.0');",
		)
		.await
		.unwrap();

		// Planning the older minor deliberately: the site can only absorb 2.61.
		let resp = private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
				// Long past, so `late` is stable however far in the future this
				// test runs. Date arithmetic itself is unit-tested with injected days.
				"planned_for": "2020-01-01",
				"note": "site can absorb 2.61 only",
			}))
			.await;
		resp.assert_status_ok();
		let plan: Value = resp.json();
		assert_eq!(plan["planned_for"], "2020-01-01");
		assert_eq!(plan["note"], "site can absorb 2.61 only");

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();

		let planned = fleet
			.iter()
			.find(|row| row["group_id"] == GROUP)
			.expect("the planned group");
		assert_eq!(planned["target_version"], "2.61.0");
		assert_eq!(planned["current_version"], "2.60.0");
		assert_eq!(planned["late"], true, "the planned day has passed unmet");

		// A group with no plan is still listed: an unplanned group several
		// minors behind is what the view exists to surface.
		let unplanned = fleet
			.iter()
			.find(|row| row["group_id"] == UNPLANNED)
			.expect("the unplanned group");
		assert!(unplanned["plan"].is_null());
		assert_eq!(unplanned["late"], false);
		assert_eq!(
			unplanned["behind"], 3,
			"2.60.0 against the newest published, 2.63.0"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_target_behind_the_group_is_refused() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'ahead');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://ahead.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.62.0');",
		)
		.await
		.unwrap();

		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
			}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

/// A plan on a group with nothing declared to migrate its data is never
/// dispatched, so the view says so rather than reporting it as merely untested.
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_nothing_will_test_says_so() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');
			INSERT INTO devices (id, role) VALUES
				('cccccccc-0000-0000-0000-0000000000d0', 'backup-restore');
			INSERT INTO restore_consumer_capabilities
				(consumer_device_id, intent, semantics)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0', 'analytics',
				'[\"check\", \"url\"]');
			INSERT INTO restore_replicas
				(consumer_device_id, group_id, type, intent, name)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'analytics',
				'kamaka-analytics');
			INSERT INTO upgrade_plans (group_id, rank, target_version_id) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'production',
				 'cccccccc-0000-0000-0000-0000000000f1');
			INSERT INTO backup_credential_issuances
				(device_id, group_id, type, purpose, issued_at, expires_at,
				 sts_assumed_role, bucket, prefix)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'restore',
				NOW() - INTERVAL '20 minutes', NOW() + INTERVAL '40 minutes',
				'arn:aws:iam::1:role/r', 'b', '')",
		)
		.await
		.unwrap();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = fleet.iter().find(|r| r["group_id"] == GROUP).unwrap();
		assert_eq!(
			row["testable"], false,
			"an intent that does not migrate tests nothing"
		);
		assert!(
			row["attempt"].is_null(),
			"and its restores are not a migration test under way"
		);

		// A declaration on a migrating intent is what turns the plan into work.
		conn.batch_execute(
			"INSERT INTO restore_consumer_capabilities
				(consumer_device_id, intent, semantics)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0', 'upgrade', '[\"migrate\"]');
			INSERT INTO restore_replicas
				(consumer_device_id, group_id, type, intent, name)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'upgrade',
				'kamaka-upgrade');",
		)
		.await
		.unwrap();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = fleet.iter().find(|r| r["group_id"] == GROUP).unwrap();
		assert_eq!(row["testable"], true);
		assert_eq!(row["attempt"], "in_flight");
	})
	.await;
}

/// A restore takes hours, so a group mid-test must not read as untested: an
/// issuance with no report yet is an attempt in flight.
#[tokio::test(flavor = "multi_thread")]
async fn an_attempt_in_flight_shows_beside_the_verdict() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');
			INSERT INTO devices (id, role) VALUES
				('cccccccc-0000-0000-0000-0000000000d0', 'backup-restore');
			INSERT INTO restore_consumer_capabilities
				(consumer_device_id, intent, semantics)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0', 'upgrade', '[\"migrate\"]');
			INSERT INTO restore_replicas
				(consumer_device_id, group_id, type, intent, name)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'upgrade',
				'kamaka-upgrade');
			INSERT INTO upgrade_plans (group_id, rank, target_version_id) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'production',
				 'cccccccc-0000-0000-0000-0000000000f1');",
		)
		.await
		.unwrap();

		let before: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = before.iter().find(|r| r["group_id"] == GROUP).unwrap();
		assert!(row["attempt"].is_null(), "nothing has started");

		// Credentials issued, still valid, nothing reported: a restore is running.
		conn.batch_execute(
			"INSERT INTO backup_credential_issuances
				(device_id, group_id, type, purpose, issued_at, expires_at,
				 sts_assumed_role, bucket, prefix)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'restore',
				NOW() - INTERVAL '20 minutes', NOW() + INTERVAL '40 minutes',
				'arn:aws:iam::1:role/r', 'b', '')",
		)
		.await
		.unwrap();

		let during: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = during.iter().find(|r| r["group_id"] == GROUP).unwrap();
		assert_eq!(
			row["attempt"], "in_flight",
			"a group mid-test does not read as merely untested"
		);
		assert_eq!(
			row["verdict"], "nottested",
			"and the verdict is untouched by the activity"
		);
	})
	.await;
}

/// A member server taking restore credentials (a clone refresh, a manual
/// restore) never reports, so its expired issuances must not read as a test run
/// that ended without reporting.
#[tokio::test(flavor = "multi_thread")]
async fn a_member_servers_own_restore_is_not_an_attempt() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');
			INSERT INTO devices (id, role) VALUES
				('cccccccc-0000-0000-0000-0000000000d0', 'backup-restore'),
				('cccccccc-0000-0000-0000-0000000000d1', 'server');
			INSERT INTO machines (id, group_id, device_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a0',
				 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000d1');
			INSERT INTO applications (id, name, host, type, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a0', 'clone',
				 'https://clone.example.com', 'tamanu-central',
				 'cccccccc-0000-0000-0000-000000000001',
				 'cccccccc-0000-0000-0000-0000000000a0');
			INSERT INTO restore_consumer_capabilities
				(consumer_device_id, intent, semantics)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0', 'upgrade', '[\"migrate\"]');
			INSERT INTO restore_replicas
				(consumer_device_id, group_id, type, intent, name)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'upgrade',
				'kamaka-upgrade');
			INSERT INTO upgrade_plans (group_id, rank, target_version_id) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'production',
				 'cccccccc-0000-0000-0000-0000000000f1');
			INSERT INTO backup_credential_issuances
				(device_id, group_id, type, purpose, run_id, issued_at, expires_at,
				 sts_assumed_role, bucket, prefix)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d1',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'restore',
				'cccccccc-0000-0000-0000-0000000000e1',
				NOW() - INTERVAL '3 hours', NOW() - INTERVAL '2 hours',
				'arn:aws:iam::1:role/r', 'b', '')",
		)
		.await
		.unwrap();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = fleet.iter().find(|r| r["group_id"] == GROUP).unwrap();
		assert!(
			row["attempt"].is_null(),
			"a member server's own restore is not the pipeline"
		);

		// The same expired-unreported issuance from the consumer is the signal.
		conn.batch_execute(
			"INSERT INTO backup_credential_issuances
				(device_id, group_id, type, purpose, issued_at, expires_at,
				 sts_assumed_role, bucket, prefix)
			 VALUES ('cccccccc-0000-0000-0000-0000000000d0',
				'cccccccc-0000-0000-0000-000000000001', 'tamanu-postgres', 'restore',
				NOW() - INTERVAL '3 hours', NOW() - INTERVAL '2 hours',
				'arn:aws:iam::1:role/r', 'b', '')",
		)
		.await
		.unwrap();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = fleet.iter().find(|r| r["group_id"] == GROUP).unwrap();
		assert_eq!(row["attempt"], "ended_without_report");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn amend_changes_the_date_and_note_without_replacing_the_plan() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');",
		)
		.await
		.unwrap();

		let recorded: Value = private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
				"planned_for": "2020-01-01",
				"note": "waiting on the site",
			}))
			.await
			.json();

		let resp = private
			.post("/api/upgrade_plans/amend")
			.json(&json!({
				"id": recorded["id"],
				"planned_for": "2020-02-02",
				"note": "site confirmed the window",
			}))
			.await;
		resp.assert_status_ok();
		let amended: Value = resp.json();
		assert_eq!(amended["id"], recorded["id"], "the same plan");
		assert_eq!(amended["planned_for"], "2020-02-02");
		assert_eq!(amended["note"], "site confirmed the window");
		assert_eq!(
			amended["target_version_id"], recorded["target_version_id"],
			"amending does not move the group somewhere else"
		);
		assert!(amended["superseded_at"].is_null());
		assert!(!amended["amended_at"].is_null());

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let row = fleet
			.iter()
			.find(|row| row["group_id"] == GROUP)
			.expect("the planned group");
		assert_eq!(row["plan"]["note"], "site confirmed the window");
		assert_eq!(
			row["target_version"], "2.61.0",
			"still going to the same place"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn amending_a_withdrawn_plan_is_refused() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published'),
				('cccccccc-0000-0000-0000-0000000000f2', 2, 63, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');",
		)
		.await
		.unwrap();

		let first: Value = private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
			}))
			.await
			.json();

		// Recording a second plan replaces the first, which is then history.
		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f2",
			}))
			.await
			.assert_status_ok();

		private
			.post("/api/upgrade_plans/amend")
			.json(&json!({ "id": first["id"], "note": "too late" }))
			.await
			.assert_status_bad_request();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_history_view_shows_a_withdrawn_plan_beside_a_replaced_one() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published'),
				('cccccccc-0000-0000-0000-0000000000f2', 2, 63, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');",
		)
		.await
		.unwrap();

		let replaced: Value = private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
				"planned_for": "2020-01-01",
				"note": "site can absorb 2.61 only",
			}))
			.await
			.json();

		let withdrawn: Value = private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f2",
			}))
			.await
			.json();
		private
			.post("/api/upgrade_plans/withdraw")
			.json(&json!({ "id": withdrawn["id"] }))
			.await
			.assert_status_ok();

		let history: Vec<Value> = private
			.post("/api/upgrade_plans/history")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(history.len(), 2, "both closed plans are readable");

		// Most recently closed first: the withdrawal happened after the
		// replacement it followed.
		assert_eq!(history[0]["plan"]["id"], withdrawn["id"]);
		assert_eq!(history[0]["outcome"], "withdrawn");
		assert_eq!(history[0]["target_version"], "2.63.0");
		assert_eq!(history[0]["group_name"], "kamaka");
		assert!(
			!history[0]["plan"]["withdrawn_by"].is_null(),
			"who withdrew it is part of the record"
		);
		assert_eq!(history[0]["ended_at"], history[0]["plan"]["withdrawn_at"]);

		assert_eq!(history[1]["plan"]["id"], replaced["id"]);
		assert_eq!(history[1]["outcome"], "replaced");
		assert_eq!(history[1]["plan"]["planned_for"], "2020-01-01");
		assert_eq!(history[1]["plan"]["note"], "site can absorb 2.61 only");

		// An open plan is not history.
		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
			}))
			.await
			.assert_status_ok();
		let history: Vec<Value> = private
			.post("/api/upgrade_plans/history")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(history.len(), 2);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn every_version_ahead_is_offered_however_far_behind_the_group_is() {
	commons_tests::server::run(async |mut conn, _, private| {
		// Ten minors of patch releases sit between the group and the newest, so
		// the version it is actually going to is a long way down the list.
		let mut rows = vec!["(2, 54, 0, 'x', 'published')".to_owned()];
		for minor in 56..=65 {
			for patch in 0..=5 {
				rows.push(format!("(2, {minor}, {patch}, 'x', 'published')"));
			}
		}
		conn.batch_execute(&format!(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES {};
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{{}}'::jsonb, '2.53.0');",
			rows.join(", ")
		))
		.await
		.unwrap();

		let targets: Vec<Value> = private
			.post("/api/upgrade_plans/targets")
			.json(&json!({ "group_id": GROUP, "rank": "production" }))
			.await
			.json();

		assert!(
			targets.iter().any(|v| v["version"] == "2.54.0"),
			"a group moving to an older minor cannot pick it: offered {:?}",
			targets
				.iter()
				.map(|v| v["version"].as_str().unwrap_or(""))
				.collect::<Vec<_>>()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_target_under_an_open_known_issue_is_offered_but_flagged() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES
				(2, 61, 0, 'x', 'published'),
				(2, 61, 1, 'x', 'published'),
				(2, 62, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0');
			INSERT INTO version_known_issues (author, description, min_major, min_minor, min_patch)
				VALUES ('someone@example.com', 'breaks on upgrade', 2, 61, 1);",
		)
		.await
		.unwrap();

		let targets: Vec<Value> = private
			.post("/api/upgrade_plans/targets")
			.json(&json!({ "group_id": GROUP, "rank": "production" }))
			.await
			.json();

		let ready = |version: &str| {
			targets
				.iter()
				.find(|v| v["version"] == version)
				.unwrap_or_else(|| panic!("{version} is not offered: {targets:?}"))["ready"]
				.as_bool()
				.expect("ready")
		};

		// The issue covers 2.61.1 and every later patch in that minor; a
		// group may still plan for it, so it is offered rather than hidden.
		assert!(!ready("2.61.1"));
		// Earlier patches and other minors are untouched by it.
		assert!(ready("2.61.0"));
		assert!(ready("2.62.0"));
	})
	.await;
}

/// A site's clone is often moved ahead of its production, so the two are
/// planned, listed, and offered targets apart.
#[tokio::test(flavor = "multi_thread")]
async fn each_environment_is_planned_apart() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published'),
				('cccccccc-0000-0000-0000-0000000000f2', 2, 63, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001'),
				('cccccccc-0000-0000-0000-0000000000a2', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1'),
				('cccccccc-0000-0000-0000-0000000000a2', 'https://clone.kamaka.example', 'tamanu-central', 'clone', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a2');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0'),
				('cccccccc-0000-0000-0000-0000000000a2', 'test', '{}'::jsonb, '2.62.0');",
		)
		.await
		.unwrap();

		// The clone is already past 2.61, so it is offered only what is ahead of it.
		let clone_targets: Vec<Value> = private
			.post("/api/upgrade_plans/targets")
			.json(&json!({ "group_id": GROUP, "rank": "clone" }))
			.await
			.json();
		assert_eq!(
			clone_targets.iter().map(|v| &v["version"]).collect::<Vec<_>>(),
			vec!["2.63.0"]
		);
		let production_targets: Vec<Value> = private
			.post("/api/upgrade_plans/targets")
			.json(&json!({ "group_id": GROUP, "rank": "production" }))
			.await
			.json();
		assert_eq!(production_targets.len(), 2, "production is behind both");

		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "clone",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f2",
			}))
			.await
			.assert_status_ok();
		// Where the clone already is cannot be planned for production's benefit
		// either: the check is against the environment named.
		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "clone",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
			}))
			.await
			.assert_status_bad_request();
		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "production",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
			}))
			.await
			.assert_status_ok();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let rows: Vec<&Value> = fleet.iter().filter(|r| r["group_id"] == GROUP).collect();
		assert_eq!(rows.len(), 2, "one row per environment: {fleet:?}");
		let production = rows.iter().find(|r| r["rank"] == "production").unwrap();
		let clone = rows.iter().find(|r| r["rank"] == "clone").unwrap();
		assert_eq!(production["headline"], true);
		assert_eq!(production["current_version"], "2.60.0");
		assert_eq!(production["target_version"], "2.61.0");
		assert_eq!(clone["headline"], false);
		assert_eq!(clone["current_version"], "2.62.0");
		assert_eq!(
			clone["target_version"], "2.63.0",
			"recording for production left the clone's plan standing"
		);
	})
	.await;
}

/// A plan outlives the environment it was recorded for. Archiving the last
/// application at a rank leaves the group with a plan and nowhere to read or
/// withdraw it, so the view carries a row with no running version rather than
/// dropping it.
// spec: UPG#the-fleet-view
#[tokio::test(flavor = "multi_thread")]
async fn a_plan_whose_environment_has_no_live_application_is_still_listed() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f1', 2, 61, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000001', 'kamaka');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'cccccccc-0000-0000-0000-000000000001'),
				('cccccccc-0000-0000-0000-0000000000a2', 'cccccccc-0000-0000-0000-000000000001');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'https://kamaka.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a1'),
				('cccccccc-0000-0000-0000-0000000000a2', 'https://kamaka-clone.example', 'tamanu-central', 'clone', 'cccccccc-0000-0000-0000-000000000001', 'cccccccc-0000-0000-0000-0000000000a2');
			INSERT INTO application_reported_detail (application_id, source, extra, version) VALUES
				('cccccccc-0000-0000-0000-0000000000a1', 'test', '{}'::jsonb, '2.60.0'),
				('cccccccc-0000-0000-0000-0000000000a2', 'test', '{}'::jsonb, '2.60.0');",
		)
		.await
		.unwrap();

		private
			.post("/api/upgrade_plans/record")
			.json(&json!({
				"group_id": GROUP,
				"rank": "clone",
				"target_version_id": "cccccccc-0000-0000-0000-0000000000f1",
			}))
			.await
			.assert_status_ok();

		conn.batch_execute(
			"UPDATE applications SET deleted_at = NOW() \
			 WHERE id = 'cccccccc-0000-0000-0000-0000000000a2';",
		)
		.await
		.unwrap();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let clone = fleet
			.iter()
			.find(|row| row["rank"] == "clone")
			.expect("the clone's row survives its last application");
		assert!(
			clone["plan"]["id"].is_string(),
			"and it still carries the plan, which is the only way to withdraw it"
		);
		assert!(
			clone["current_version"].is_null(),
			"with no running version, since nothing is running"
		);
		assert!(
			clone["behind"].is_null(),
			"and no distance, rather than a distance from nothing"
		);
		assert_eq!(
			clone["headline"], false,
			"a synthetic row is never the group's headline"
		);
	})
	.await;
}

/// An environment that has reported no version cannot be placed against the
/// newest one, so its distance is null. The dashboard's no-plan list keys off
/// that distance, so a null is what keeps an unreporting environment out of a
/// list of things to plan.
// spec: UPG#the-fleet-view
#[tokio::test(flavor = "multi_thread")]
async fn an_environment_that_has_reported_no_version_has_no_distance() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
				('cccccccc-0000-0000-0000-0000000000f2', 2, 63, 0, 'x', 'published');
			INSERT INTO server_groups (id, name) VALUES
				('cccccccc-0000-0000-0000-000000000002', 'silent');
			INSERT INTO machines (id, group_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a2', 'cccccccc-0000-0000-0000-000000000002');
			INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
				('cccccccc-0000-0000-0000-0000000000a2', 'https://silent.example', 'tamanu-central', 'production', 'cccccccc-0000-0000-0000-000000000002', 'cccccccc-0000-0000-0000-0000000000a2');",
		)
		.await
		.unwrap();

		let fleet: Vec<Value> = private
			.post("/api/upgrade_plans/fleet")
			.json(&json!({}))
			.await
			.json();
		let silent = fleet
			.iter()
			.find(|row| row["group_id"] == UNPLANNED)
			.expect("the environment is listed");
		assert!(
			silent["current_version"].is_null(),
			"nothing has been reported for it"
		);
		assert!(
			silent["behind"].is_null(),
			"so there is no distance to the newest published version"
		);
		assert!(silent["plan"].is_null());
	})
	.await;
}
