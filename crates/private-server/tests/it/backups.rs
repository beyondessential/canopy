//! Endpoint tests for the operator-facing `/api/backups/*` fns.
//!
//! The test harness uses the in-memory backup Secret store, so onboarding
//! (Canopy generating/storing the repo passphrase) is exercised without a
//! cluster.

use commons_tests::diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

/// Seed a server group; returns its id.
async fn seed_group(conn: &mut impl SimpleAsyncConnection) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{id}', 'grp-{id}');"
	))
	.await
	.expect("seed group");
	id
}

fn retention_json() -> serde_json::Value {
	serde_json::json!({
		"keep_latest": 1,
		"keep_daily": 7,
		"keep_weekly": 4,
		"keep_monthly": 6,
		"keep_annual": 0
	})
}

/// Assert a group has no backup config — a rejected upsert must not partially
/// write. (A macro, not a fn, to avoid naming `axum_test::TestServer`.)
macro_rules! assert_no_config {
	($private:expr, $group:expr) => {{
		let resp = $private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": $group }))
			.await;
		resp.assert_status_ok();
		assert!(
			resp.json::<serde_json::Value>().is_null(),
			"expected no config to be written"
		);
	}};
}

#[tokio::test(flavor = "multi_thread")]
async fn create_get_zero_state_and_full_view() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;

		// Zero-state: unconfigured group → 200 null.
		let resp = private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		assert!(resp.json::<serde_json::Value>().is_null());

		// Create.
		let resp = private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-kopia-test",
				"target_role_arn": "arn:aws:iam::123:role/x",
				"maintenance_role_arn": "arn:aws:iam::123:role/x-maint",
				"mode": "from_birth",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["status"], "provisioning");
		assert_eq!(body["mode"], "from_birth");
		assert_eq!(body["bucket"], "bes-kopia-test");

		// Get full view now.
		let resp = private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["status"], "provisioning");

		// Duplicate create → 409.
		let resp = private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "other",
				"target_role_arn": "arn:aws:iam::123:role/y",
				"maintenance_role_arn": "arn:aws:iam::123:role/y-maint",
				"mode": "from_birth",
			}))
			.await;
		resp.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_missing_group_is_404() {
	commons_tests::server::run(async |_conn, _public, private| {
		let resp = private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": Uuid::new_v4(),
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await;
		resp.assert_status_not_found();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_retention_floor_rejected_and_accepted() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();

		// Below floor → 400.
		let resp = private
			.post("/api/backups/set_retention")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"type": "tamanu-postgres",
				"retention": {
					"keep_latest": 1, "keep_daily": 1, "keep_weekly": 1,
					"keep_monthly": 1, "keep_annual": 0
				},
			}))
			.await;
		resp.assert_status_bad_request();

		// At/above floor → ok, and the retention round-trips into the view
		// without becoming a schedule override.
		let resp = private
			.post("/api/backups/set_retention")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"type": "tamanu-postgres",
				"retention": retention_json(),
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let sched = &body["schedules"][0];
		assert_eq!(sched["type"], "tamanu-postgres");
		assert!(sched["schedule"].is_null(), "retention only: {sched}");
		assert_eq!(sched["retention"]["keep_daily"], 7);
		assert_eq!(sched["allow_below_floor"], false);

		// Below floor but allow_below_floor → accepted, and the dangerous flag
		// round-trips into the view.
		let resp = private
			.post("/api/backups/set_retention")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"type": "tamanu-postgres",
				"retention": {
					"keep_latest": 1, "keep_daily": 2, "keep_weekly": 0,
					"keep_monthly": 0, "keep_annual": 0
				},
				"allow_below_floor": true,
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let sched = &body["schedules"][0];
		assert_eq!(sched["retention"]["keep_daily"], 2);
		assert_eq!(sched["allow_below_floor"], true);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn manual_only_is_a_schedule_override() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();
		let resp = private
			.post("/api/backups/set_schedule")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"type": "tamanu-postgres",
				"schedule": { "kind": "manual" },
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["schedules"][0]["schedule"]["kind"], "manual");
		assert!(body["schedules"][0]["retention"].is_null());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn set_schedule_takes_cron_and_refuses_what_cannot_run() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();

		let set = async |schedule: serde_json::Value| {
			private
				.post("/api/backups/set_schedule")
				.json(&serde_json::json!({
					"server_group_id": group_id,
					"type": "tamanu-postgres",
					"schedule": schedule,
				}))
				.await
		};

		let resp = set(serde_json::json!({
			"kind": "cron", "expression": "0 2 * * *", "zone": "Pacific/Auckland",
		}))
		.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let schedule = &body["schedules"][0]["schedule"];
		assert_eq!(schedule["kind"], "cron");
		assert_eq!(schedule["expression"], "0 2 * * *");
		assert_eq!(schedule["zone"], "Pacific/Auckland");

		for refused in [
			serde_json::json!({"kind": "cron", "expression": "*/30 * * * *"}),
			serde_json::json!({"kind": "cron", "expression": "0 0 30 2 *"}),
			serde_json::json!({"kind": "cron", "expression": "0 2 * * * UTC"}),
			serde_json::json!({"kind": "cron", "expression": "H/15 * * * *"}),
			serde_json::json!({"kind": "cron", "expression": "0 2 * * *", "zone": "Mars/Olympus"}),
			serde_json::json!({"kind": "interval", "seconds": 1800}),
		] {
			set(refused.clone()).await.assert_status_bad_request();
		}
		// None of the refusals disturbed what was set.
		let resp = private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		assert_eq!(
			resp.json::<serde_json::Value>()["schedules"][0]["schedule"]["expression"],
			"0 2 * * *"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_repo_clears_error_and_is_idempotent() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();

		// Simulate an init-Job failure stamped on the row.
		conn.batch_execute(&format!(
			"UPDATE server_group_backup_config SET last_init_error = 'boom' WHERE group_id = '{group_id}';"
		))
		.await
		.expect("set error");

		let resp = private
			.post("/api/backups/create_repo")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["status"], "provisioning");
		assert!(body["last_init_error"].is_null(), "error cleared on retry");

		// Idempotent second call.
		private
			.post("/api/backups/create_repo")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await
			.assert_status_ok();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_repo_on_ready_is_409() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();
		conn.batch_execute(&format!(
			"UPDATE server_group_backup_config SET status = 'ready' WHERE group_id = '{group_id}';"
		))
		.await
		.expect("ready");
		let resp = private
			.post("/api/backups/create_repo")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn passphrase_mode_requires_a_passphrase() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		// No passphrase supplied → 400.
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "passphrase",
			}))
			.await
			.assert_status_bad_request();

		// With a passphrase → created (no escrow; provisioning until init → ready).
		let resp = private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "passphrase",
				"passphrase": "an-existing-repo-passphrase",
			}))
			.await;
		resp.assert_status_ok();
		assert_eq!(resp.json::<serde_json::Value>()["mode"], "passphrase");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn probe_reports_state_and_already_configured() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;

		// The harness uses the bucket-name-derived fake prober; "fresh" has no
		// marker, so it reads empty.
		let resp = private
			.post("/api/backups/probe")
			.json(&serde_json::json!({
				"bucket": "fresh",
				"prefix": "",
				"maintenance_role_arn": "maint",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["state"], "empty");
		assert!(body["already_configured"].is_null());

		// A config for this bucket+prefix → the probe reports its group.
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "taken",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();
		let resp = private
			.post("/api/backups/probe")
			.json(&serde_json::json!({
				"bucket": "taken",
				"prefix": "",
				"maintenance_role_arn": "maint",
			}))
			.await;
		resp.assert_status_ok();
		assert_eq!(
			resp.json::<serde_json::Value>()["already_configured"],
			group_id.to_string()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn request_now_upserts_and_cancel_deletes() {
	commons_tests::server::run(async |mut conn, _public, private| {
		// Application in a group so we can read the pending request back via stats.
		let group_id = seed_group(&mut conn).await;
		let machine_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine_id}', '{group_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');"
		))
		.await
		.expect("seed server");

		let req = serde_json::json!({
			"machine_id": machine_id,
			"type": "tamanu-postgres",
			"purpose": "backup",
		});

		private
			.post("/api/backups/request_now")
			.json(&req)
			.await
			.assert_status_ok();
		// Re-request is a no-op upsert, not an error.
		private
			.post("/api/backups/request_now")
			.json(&req)
			.await
			.assert_status_ok();

		let stats = private
			.post("/api/backups/stats")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		stats.assert_status_ok();
		let body: serde_json::Value = stats.json();
		assert_eq!(
			body["pending_requests"].as_array().unwrap().len(),
			1,
			"single upserted row"
		);

		private
			.post("/api/backups/cancel_request")
			.json(&req)
			.await
			.assert_status_ok();
		let stats = private
			.post("/api/backups/stats")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		let body: serde_json::Value = stats.json();
		assert_eq!(
			body["pending_requests"].as_array().unwrap().len(),
			0,
			"cancel deleted the row"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn allow_disallow_restore_window_round_trip() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let machine_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine_id}', '{group_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');"
		))
		.await
		.expect("seed server");

		let args = serde_json::json!({ "machine_id": machine_id });

		// Initially closed: no window, and stats lists no open windows.
		let win = private
			.post("/api/backups/restore_window")
			.json(&args)
			.await;
		win.assert_status_ok();
		assert!(
			win.json::<serde_json::Value>()["allowed_until"].is_null(),
			"restores start disallowed"
		);

		// Allow → the endpoint echoes a future expiry.
		let allowed = private.post("/api/backups/allow_restore").json(&args).await;
		allowed.assert_status_ok();
		assert!(
			allowed.json::<serde_json::Value>()["allowed_until"].is_string(),
			"allow_restore opens the window"
		);

		// The window is visible via both the per-server getter and group stats.
		let win = private
			.post("/api/backups/restore_window")
			.json(&args)
			.await;
		assert!(win.json::<serde_json::Value>()["allowed_until"].is_string());

		let stats = private
			.post("/api/backups/stats")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		let windows = stats.json::<serde_json::Value>()["restore_windows"]
			.as_array()
			.cloned()
			.unwrap();
		assert_eq!(windows.len(), 1, "one open window");
		assert_eq!(windows[0]["machine_id"], serde_json::json!(machine_id));

		// Disallow → closed again, and dropped from stats.
		private
			.post("/api/backups/disallow_restore")
			.json(&args)
			.await
			.assert_status_ok();
		let win = private
			.post("/api/backups/restore_window")
			.json(&args)
			.await;
		assert!(
			win.json::<serde_json::Value>()["allowed_until"].is_null(),
			"disallow_restore closes the window"
		);
		let stats = private
			.post("/api/backups/stats")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		assert_eq!(
			stats.json::<serde_json::Value>()["restore_windows"]
				.as_array()
				.unwrap()
				.len(),
			0,
			"closed window is not listed"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn request_maintenance_flags_and_cancel_clears() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();

		// Not ready yet (provisioning) → 409.
		private
			.post("/api/backups/request_maintenance")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await
			.assert_status_conflict();

		conn.batch_execute(&format!(
			"UPDATE server_group_backup_config SET status = 'ready' WHERE group_id = '{group_id}';"
		))
		.await
		.expect("ready");

		// Ready → flags the request and echoes it back on the view.
		let resp = private
			.post("/api/backups/request_maintenance")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert!(
			!body["force_full_maintenance_at"].is_null(),
			"request set the pending flag"
		);

		// Re-request is an idempotent refresh, not an error.
		private
			.post("/api/backups/request_maintenance")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await
			.assert_status_ok();

		// Cancel clears it.
		let resp = private
			.post("/api/backups/cancel_maintenance")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert!(
			body["force_full_maintenance_at"].is_null(),
			"cancel cleared the pending flag"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn request_maintenance_missing_config_is_404() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/request_maintenance")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await
			.assert_status_not_found();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stats_includes_runs_and_pending_requests() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let device_id = Uuid::new_v4();
		let consumer_id = Uuid::new_v4();
		let machine_id = Uuid::new_v4();
		let run_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{device_id}', 'machine');
			 WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{machine_id}', '{group_id}', '{device_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');
			 INSERT INTO backup_repo_stats (group_id, snapshot_count, source_count, logical_bytes, physical_bytes, bucket_bytes, bucket_bytes_observed_at) \
				VALUES ('{group_id}', 12, 3, 1000, 800, 2000, now() - interval '1 day');
			 INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, bytes_uploaded) \
				VALUES ('{run_id}', '{device_id}', '{group_id}', '{machine_id}', 'tamanu-postgres', 'backup', 'success', 500);
			 -- The issuance that started the backup 5 minutes before it reported, so
			 -- the run carries a Canopy-measured duration of ~300s.
			 INSERT INTO backup_credential_issuances \
				(device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix) \
				VALUES ('{device_id}', '{group_id}', 'tamanu-postgres', now() - interval '300 seconds', \
					now() + interval '3300 seconds', 'backup', 'arn:test', 'b', '');
			 -- A restore whose credentials are still valid but which never reported:
			 -- surfaces as an in-flight run (bestool's manual restore doesn't report).
			 INSERT INTO backup_credential_issuances \
				(device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix) \
				VALUES ('{device_id}', '{group_id}', 'tamanu-postgres', now() - interval '30 seconds', \
					now() + interval '3570 seconds', 'restore', 'arn:test', 'b', '');
			 -- A managed-restore *consumer* device's restore issuance belongs to the
			 -- restore-checks table, not this backup panel — it must be excluded.
			 INSERT INTO devices (id, role) VALUES ('{consumer_id}', 'backup-restore');
			 INSERT INTO backup_credential_issuances \
				(device_id, group_id, type, issued_at, expires_at, purpose, sts_assumed_role, bucket, prefix) \
				VALUES ('{consumer_id}', '{group_id}', 'tamanu-postgres', now() - interval '30 seconds', \
					now() + interval '3570 seconds', 'restore', 'arn:test', 'b', '');
			 INSERT INTO backup_requests (machine_id, type, purpose) VALUES \
				('{machine_id}', 'tamanu-postgres', 'backup');
			 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) VALUES \
				('{machine_id}', 'tamanu-postgres', true);"
		))
		.await
		.expect("seed stats");

		let resp = private
			.post("/api/backups/stats")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["stats"]["snapshot_count"], 12);
		assert_eq!(body["stats"]["bucket_bytes"], 2000);
		// The bucket figure carries its own measurement timestamp, separate
		// from the inspection-owned observed_at.
		assert!(body["stats"]["bucket_bytes_observed_at"].is_string());
		// The reported backup, plus an inferred in-flight restore from its issuance.
		// The consumer device's restore issuance is excluded (it's shown in the
		// restore-checks table instead).
		let runs = body["recent_runs"].as_array().unwrap();
		assert_eq!(runs.len(), 2);
		// Newest first: the reported backup (reported now) sorts above the restore
		// (started 30s ago).
		assert_eq!(runs[0]["outcome"], "success");
		assert_eq!(runs[0]["status"], "reported");
		let duration = runs[0]["duration_seconds"].as_i64().expect("duration");
		assert!(
			(250..=350).contains(&duration),
			"duration ~300s from issuance→report, got {duration}"
		);
		// The unreported restore is surfaced as in-flight with no outcome.
		let restore = runs
			.iter()
			.find(|r| r["purpose"] == "restore")
			.expect("restore row");
		assert_eq!(restore["status"], "in_progress");
		assert!(restore["outcome"].is_null());
		assert!(restore["duration_seconds"].is_null());
		assert_eq!(body["pending_requests"].as_array().unwrap().len(), 1);
		assert_eq!(body["pending_requests"][0]["purpose"], "backup");
		// The member's declared capabilities ride along so the "back up now"
		// panel knows which types to offer per server.
		assert_eq!(body["capabilities"].as_array().unwrap().len(), 1);
		assert_eq!(body["capabilities"][0]["machine_id"], machine_id.to_string());
		assert_eq!(body["capabilities"][0]["type"], "tamanu-postgres");
		assert_eq!(body["capabilities"][0]["enabled"], true);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn stats_restore_run_resolves_snapshot_size_from_the_producing_backup() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let device_id = Uuid::new_v4();
		let machine_id = Uuid::new_v4();
		let backup_run_id = Uuid::new_v4();
		let restore_run_id = Uuid::new_v4();
		// The backup that produced snapshot snap-1 was sized when it ran; the
		// restore that later used snap-1 has no size of its own (inspection has
		// not swept since it reported). Its snapshot size must resolve from the
		// producing backup immediately, not wait for the backfill.
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{device_id}', 'machine');
			 WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{machine_id}', '{group_id}', '{device_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');
			 INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, \
				bytes_uploaded, snapshot_id, reported_at) \
				VALUES ('{backup_run_id}', '{device_id}', '{group_id}', '{machine_id}', 'tamanu-postgres', \
					'backup', 'success', 8192, 'snap-1', now() - interval '1 hour');
			 INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, \
				snapshot_id, s3_received_payload_bytes) \
				VALUES ('{restore_run_id}', '{device_id}', '{group_id}', '{machine_id}', 'tamanu-postgres', \
					'restore', 'success', 'snap-1', 4096);"
		))
		.await
		.expect("seed runs");

		let resp = private
			.post("/api/backups/stats")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let runs = body["recent_runs"].as_array().unwrap();
		let restore = runs
			.iter()
			.find(|r| r["purpose"] == "restore")
			.expect("restore row");
		assert_eq!(
			restore["snapshot_logical_bytes"], 8192,
			"restore resolves its source snapshot's size from the backup that produced it"
		);
		assert!(restore["bytes_uploaded"].is_null());
		// The backup row keeps its own figures untouched.
		let backup = runs
			.iter()
			.find(|r| r["purpose"] == "backup")
			.expect("backup row");
		assert_eq!(backup["bytes_uploaded"], 8192);
		assert!(backup["snapshot_logical_bytes"].is_null());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn group_schedules_reports_the_groups_schedule_and_layer() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let device_id = Uuid::new_v4();
		let machine_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role) VALUES ('{device_id}', 'machine');
			 WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{machine_id}', '{group_id}', '{device_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');
			 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) VALUES \
				('{machine_id}', 'tamanu-postgres', true);
			 -- A 1h scheduled interval and a successful backup at a known time.
			 INSERT INTO server_group_backup_schedule (group_id, type, expected_interval) VALUES \
				('{group_id}', 'tamanu-postgres', INTERVAL '3600 seconds');
			 INSERT INTO backup_runs (id, device_id, group_id, machine_id, type, purpose, outcome, reported_at) VALUES \
				('{}', '{device_id}', '{group_id}', '{machine_id}', 'tamanu-postgres', 'backup', 'success', '2026-06-01T00:00:00Z');",
			Uuid::new_v4()
		))
		.await
		.expect("seed schedule + run");

		let resp = private
			.post("/api/backups/group_schedules")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let row = &body[0];
		assert_eq!(row["type"], "tamanu-postgres");
		assert_eq!(row["schedule"], serde_json::json!({"kind": "interval", "seconds": 3600}));
		assert_eq!(row["group_schedule"], row["schedule"]);
		assert_eq!(row["layer"], "group");
		// The seeded fleet default is what it would otherwise inherit.
		assert_eq!(
			row["fleet_schedule"],
			serde_json::json!({"kind": "interval", "seconds": 21600})
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn group_schedules_reports_manual_only_override_as_no_schedule() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let machine_id = Uuid::new_v4();
		// An override row with a NULL interval is manual-only, even though
		// `tamanu-postgres` has a canopy-wide 6h default to inherit from.
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine_id}', '{group_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');
			 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) VALUES \
				('{machine_id}', 'tamanu-postgres', true);
			 INSERT INTO server_group_backup_schedule (group_id, type, expected_interval) VALUES \
				('{group_id}', 'tamanu-postgres', NULL);"
		))
		.await
		.expect("seed manual-only override");

		let resp = private
			.post("/api/backups/group_schedules")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let row = body
			.as_array()
			.unwrap()
			.iter()
			.find(|r| r["type"] == "tamanu-postgres")
			.expect("the overridden type appears in schedule/retention");
		assert_eq!(
			row["schedule"]["kind"], "manual",
			"manual-only must not resurrect the type default",
		);
		assert_eq!(row["layer"], "group");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn group_schedules_includes_disabled_declared_types() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let machine_id = Uuid::new_v4();
		// A *declared but disabled* type (manual-only): retention still applies, so
		// it must surface in schedule/retention as a manual-only entry.
		conn.batch_execute(&format!(
			"WITH m AS (INSERT INTO machines (name, id, group_id) VALUES ('box', '{machine_id}', '{group_id}') RETURNING id) INSERT INTO applications (id, host, type, group_id, machine_id) VALUES \
				('{machine_id}', 'https://e.test', 'tamanu-central', '{group_id}', '{machine_id}');
			 INSERT INTO machine_backup_capabilities (machine_id, type, enabled) VALUES \
				('{machine_id}', 'files', false);"
		))
		.await
		.expect("seed disabled capability");

		let resp = private
			.post("/api/backups/group_schedules")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let row = body
			.as_array()
			.unwrap()
			.iter()
			.find(|r| r["type"] == "files")
			.expect("disabled declared type appears in schedule/retention");
		// Manual-only: no schedule, but a (floor) retention policy that applies to
		// manual backups of this type.
		assert_eq!(row["schedule"]["kind"], "manual");
		assert!(row["group_schedule"].is_null());
		assert!(row["effective_retention"]["keep_daily"].as_i64().unwrap() >= 7);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn update_region_and_delete() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
				"region": "ap-southeast-2",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();

		let resp = private
			.post("/api/backups/update")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"region": "us-east-1",
			}))
			.await;
		resp.assert_status_ok();
		assert_eq!(resp.json::<serde_json::Value>()["region"], "us-east-1");

		private
			.post("/api/backups/delete")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await
			.assert_status_ok();

		// Now a get → null again.
		let resp = private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		assert!(resp.json::<serde_json::Value>().is_null());
	})
	.await;
}

/// Onboarding is all-or-nothing on both create paths. A config row left behind
/// by a failed Secret create is unrecoverable: it points its
/// `repo_password_ref` at nothing, and every retry now sees a config and takes
/// the *update* path, which never creates the Secret.
#[tokio::test(flavor = "multi_thread")]
async fn upsert_rolls_back_the_config_when_the_secret_cannot_be_created() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;

		// Onboard once so the group's passphrase Secret exists...
		let resp = private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-iac",
				"target_role_arn": "arn:aws:iam::123:role/dev",
				"maintenance_role_arn": "arn:aws:iam::123:role/maint",
				"mode": "from_birth",
			}))
			.await;
		resp.assert_status_ok();

		// ...then drop only the config row, leaving the Secret behind. `upsert`
		// now takes the create path, and its create-if-absent Secret write
		// fails — the shape of any transient secret-store failure.
		conn.batch_execute(&format!(
			"DELETE FROM server_group_backup_config WHERE group_id = '{group_id}'"
		))
		.await
		.expect("drop config row");

		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-iac",
				"target_role_arn": "arn:aws:iam::123:role/dev",
				"maintenance_role_arn": "arn:aws:iam::123:role/maint",
			}))
			.await;
		resp.assert_status(axum::http::StatusCode::BAD_GATEWAY);
		assert_no_config!(private, group_id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_creates_then_reapplies_idempotently() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;

		// First apply → creates from-birth (the fake prober reports empty) and
		// provisions. Schedule/retention are NOT part of this API — they inherit
		// the canopy-wide type defaults and are UI-managed.
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-iac",
				"prefix": "p/",
				"target_role_arn": "arn:aws:iam::123:role/dev",
				"maintenance_role_arn": "arn:aws:iam::123:role/maint",
				"region": "ap-southeast-2",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["status"], "provisioning");
		assert_eq!(body["mode"], "from_birth");
		assert_eq!(body["maintenance_role_arn"], "arn:aws:iam::123:role/maint");
		// No per-(group,type) schedule override is written by upsert.
		assert_eq!(body["schedules"].as_array().unwrap().len(), 0);

		// Re-apply with changed mutable fields → updates in place, no duplicate.
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-iac",
				"prefix": "p/",
				"target_role_arn": "arn:aws:iam::123:role/dev2",
				"maintenance_role_arn": "arn:aws:iam::123:role/maint2",
				"region": "us-east-1",
			}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["maintenance_role_arn"], "arn:aws:iam::123:role/maint2");
		assert_eq!(body["target_role_arn"], "arn:aws:iam::123:role/dev2");
		assert_eq!(body["region"], "us-east-1");

		// Changing the bucket (identity) is rejected.
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-iac-moved",
				"prefix": "p/",
				"target_role_arn": "arn:aws:iam::123:role/dev2",
				"maintenance_role_arn": "arn:aws:iam::123:role/maint2",
			}))
			.await;
		resp.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_missing_group_is_404() {
	commons_tests::server::run(async |_conn, _public, private| {
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": Uuid::new_v4(),
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint-arn",
			}))
			.await;
		resp.assert_status_not_found();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_rejects_prefix_change() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let base = serde_json::json!({
			"server_group_id": group_id,
			"bucket": "bes-iac",
			"prefix": "a/",
			"target_role_arn": "arn",
			"maintenance_role_arn": "maint",
		});
		private
			.post("/api/backups/upsert")
			.json(&base)
			.await
			.assert_status_ok();

		// Same bucket, different prefix → prefix is part of the immutable identity.
		let mut moved = base.clone();
		moved["prefix"] = serde_json::json!("b/");
		private
			.post("/api/backups/upsert")
			.json(&moved)
			.await
			.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_rejects_existing_repo() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		// `…existing…` → the fake prober reports an existing kopia repo. The
		// machine API never imports; that's an interactive wizard action.
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-existing-repo",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint",
			}))
			.await;
		resp.assert_status_conflict();
		assert_no_config!(private, group_id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_rejects_other_content() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-other-stuff",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint",
			}))
			.await;
		resp.assert_status_conflict();
		assert_no_config!(private, group_id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_rejects_inaccessible_bucket() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "bes-denied",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint",
			}))
			.await;
		// Inaccessible → AppError::Upstream → 502.
		assert_eq!(resp.status_code().as_u16(), 502);
		assert_no_config!(private, group_id);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_rejects_bucket_already_configured_for_another_group() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_a = seed_group(&mut conn).await;
		let group_b = seed_group(&mut conn).await;

		private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_a,
				"bucket": "bes-shared",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint",
			}))
			.await
			.assert_status_ok();

		// A different group can't claim the same bucket/prefix, and is told
		// which group has it.
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_b,
				"bucket": "bes-shared",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint",
			}))
			.await;
		resp.assert_status_conflict();
		assert!(
			resp.text()
				.contains(&format!("already configured for grp-{group_a}")),
			"{}",
			resp.text()
		);
		assert_no_config!(private, group_b);
	})
	.await;
}

/// Decrypt an `age` ciphertext with the given identity (test-side, proving the
/// vault format is standard and the ceremony round-trips).
fn age_decrypt(ciphertext: &[u8], identity: &age::x25519::Identity) -> Vec<u8> {
	use std::io::Read;
	let decryptor = age::Decryptor::new_buffered(ciphertext).unwrap();
	let mut reader = decryptor
		.decrypt(std::iter::once(identity as &dyn age::Identity))
		.unwrap();
	let mut out = Vec::new();
	reader.read_to_end(&mut out).unwrap();
	out
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_ceremony_roundtrip() {
	use base64::prelude::*;

	// Each nextest test runs in its own process, so this env set is isolated.
	let identity = age::x25519::Identity::generate();
	unsafe {
		std::env::set_var(
			"CANOPY_RECOVERY_VAULT_KEYS",
			identity.to_public().to_string(),
		);
	}

	commons_tests::server::run(async move |_conn, _public, private| {
		// Status: configured but never verified → due.
		let resp = private
			.post("/api/backups/recovery_status")
			.json(&serde_json::json!({}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert_eq!(body["configured"], true);
		assert_eq!(body["due"], true);
		assert!(body["last_verified_at"].is_null());

		// Challenge → decrypt offline → verify.
		let resp = private
			.post("/api/backups/recovery_challenge")
			.json(&serde_json::json!({}))
			.await;
		resp.assert_status_ok();
		let ct_b64 = resp.json::<serde_json::Value>()["ciphertext_base64"]
			.as_str()
			.unwrap()
			.to_string();
		let plaintext = age_decrypt(&BASE64_STANDARD.decode(ct_b64).unwrap(), &identity);
		let answer = String::from_utf8(plaintext).unwrap();

		let resp = private
			.post("/api/backups/recovery_verify")
			.json(&serde_json::json!({ "answer": answer }))
			.await;
		resp.assert_status_ok();

		// Status now reports verified (not due) for the current recipients.
		let resp = private
			.post("/api/backups/recovery_status")
			.json(&serde_json::json!({}))
			.await;
		let body: serde_json::Value = resp.json();
		assert_eq!(body["due"], false);
		assert!(!body["last_verified_at"].is_null());
		assert_eq!(
			body["last_verified_recipients"][0],
			identity.to_public().to_string()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_status_reports_last_write() {
	use database::RecoveryVaultWrite;

	commons_tests::server::run(async move |mut conn, _public, private| {
		// No writes yet → null.
		let resp = private
			.post("/api/backups/recovery_status")
			.json(&serde_json::json!({}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert!(body["last_write_at"].is_null());
		assert!(body["last_write_bytes"].is_null());

		RecoveryVaultWrite::record(&mut conn, 12_345)
			.await
			.expect("record vault write");

		let resp = private
			.post("/api/backups/recovery_status")
			.json(&serde_json::json!({}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		assert!(!body["last_write_at"].is_null());
		assert_eq!(body["last_write_bytes"], 12_345);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn recovery_verify_rejects_wrong_and_missing() {
	let identity = age::x25519::Identity::generate();
	unsafe {
		std::env::set_var(
			"CANOPY_RECOVERY_VAULT_KEYS",
			identity.to_public().to_string(),
		);
	}

	commons_tests::server::run(async move |_conn, _public, private| {
		// No outstanding challenge → 400.
		private
			.post("/api/backups/recovery_verify")
			.json(&serde_json::json!({ "answer": "anything" }))
			.await
			.assert_status_bad_request();

		// Issue a challenge, then answer wrong → 400 (and the challenge is spent).
		private
			.post("/api/backups/recovery_challenge")
			.json(&serde_json::json!({}))
			.await
			.assert_status_ok();
		private
			.post("/api/backups/recovery_verify")
			.json(&serde_json::json!({ "answer": "not-the-nonce" }))
			.await
			.assert_status_bad_request();
		// Spent: a second verify with no fresh challenge → 400.
		private
			.post("/api/backups/recovery_verify")
			.json(&serde_json::json!({ "answer": "not-the-nonce" }))
			.await
			.assert_status_bad_request();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn type_defaults_list_and_set_roundtrip() {
	commons_tests::server::run(async |_conn, _public, private| {
		// The migration seeds the tamanu-postgres default at 6h.
		let resp = private
			.post("/api/backups/type_defaults")
			.json(&serde_json::json!({}))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();
		let td = body
			.as_array()
			.unwrap()
			.iter()
			.find(|d| d["type"] == "tamanu-postgres")
			.expect("seeded default present");
		assert_eq!(
			td["default_schedule"],
			serde_json::json!({"kind": "interval", "seconds": 21600}),
		); // 6h
		assert_eq!(td["auto_enable"], false); // capabilities stay opt-in

		// Update it.
		private
			.post("/api/backups/set_type_default")
			.json(&serde_json::json!({
				"type": "tamanu-postgres",
				"default_schedule": {"kind": "interval", "seconds": 7200},
				"default_retention": retention_json(),
				"auto_enable": false,
			}))
			.await
			.assert_status_ok();
		let resp = private
			.post("/api/backups/type_defaults")
			.json(&serde_json::json!({}))
			.await;
		let body: serde_json::Value = resp.json();
		let td = body
			.as_array()
			.unwrap()
			.iter()
			.find(|d| d["type"] == "tamanu-postgres")
			.unwrap();
		assert_eq!(td["default_schedule"]["seconds"], 7200);
		assert_eq!(td["auto_enable"], false);

		assert_eq!(td["allow_below_floor"], false);

		// Below-floor retention → 400.
		private
			.post("/api/backups/set_type_default")
			.json(&serde_json::json!({
				"type": "tamanu-postgres",
				"default_schedule": {"kind": "manual"},
				"default_retention": {
					"keep_latest": 1, "keep_daily": 1, "keep_weekly": 1,
					"keep_monthly": 1, "keep_annual": 0
				},
			}))
			.await
			.assert_status_bad_request();

		// Below-floor retention with allow_below_floor → accepted, flag persisted.
		private
			.post("/api/backups/set_type_default")
			.json(&serde_json::json!({
				"type": "tamanu-postgres",
				"default_schedule": {"kind": "manual"},
				"default_retention": {
					"keep_latest": 1, "keep_daily": 1, "keep_weekly": 0,
					"keep_monthly": 0, "keep_annual": 0
				},
				"allow_below_floor": true,
			}))
			.await
			.assert_status_ok();
		let resp = private
			.post("/api/backups/type_defaults")
			.json(&serde_json::json!({}))
			.await;
		let body: serde_json::Value = resp.json();
		let td = body
			.as_array()
			.unwrap()
			.iter()
			.find(|d| d["type"] == "tamanu-postgres")
			.unwrap();
		assert_eq!(td["allow_below_floor"], true);
		assert_eq!(td["default_retention"]["keep_daily"], 1);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn clear_schedule_removes_override() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = seed_group(&mut conn).await;
		private
			.post("/api/backups/create")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": "b",
				"target_role_arn": "arn",
				"maintenance_role_arn": "maint",
				"mode": "from_birth",
			}))
			.await
			.assert_status_ok();

		// Set a per-(group,type) override → appears in the config view.
		private
			.post("/api/backups/set_schedule")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"type": "tamanu-postgres",
				"schedule": {"kind": "interval", "seconds": 3600},
			}))
			.await
			.assert_status_ok();
		let resp = private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		assert_eq!(
			resp.json::<serde_json::Value>()["schedules"]
				.as_array()
				.unwrap()
				.len(),
			1
		);

		// Clear it → back to inheriting the default (no override row).
		private
			.post("/api/backups/clear_schedule")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"type": "tamanu-postgres",
			}))
			.await
			.assert_status_ok();
		let resp = private
			.post("/api/backups/get")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		assert_eq!(
			resp.json::<serde_json::Value>()["schedules"]
				.as_array()
				.unwrap()
				.len(),
			0
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_shared_auto_names_bucket_with_blank_roles() {
	commons_tests::server::run(async |mut conn, _public, private| {
		// Custom group name so its sanitized form lands in the auto bucket name.
		let group_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'Acme Prod');"
		))
		.await
		.expect("seed group");

		let resp = private
			.post("/api/backups/create_shared")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let body: serde_json::Value = resp.json();

		assert_eq!(body["placement"], "shared");
		assert_eq!(body["status"], "provisioning");
		assert_eq!(body["mode"], "from_birth");
		// The role ARNs + region are left blank: the backups pod stamps them at
		// provision time from its own env, so private-server needs no shared config.
		assert_eq!(body["target_role_arn"], "");
		assert_eq!(body["maintenance_role_arn"], "");
		assert!(body["region"].is_null());
		// Auto-named from the group name + random suffix, whole name ≤ 63.
		let bucket = body["bucket"].as_str().expect("bucket string");
		assert!(
			bucket.starts_with("bes-canopy-backup-acme-prod-"),
			"unexpected bucket name: {bucket}"
		);
		assert!(bucket.len() <= 63, "bucket too long: {bucket}");

		// Duplicate onboarding for the same group → 409.
		let resp = private
			.post("/api/backups/create_shared")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_conflict();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn create_shared_missing_group_is_404() {
	commons_tests::server::run(async |_conn, _public, private| {
		let resp = private
			.post("/api/backups/create_shared")
			.json(&serde_json::json!({ "server_group_id": Uuid::new_v4() }))
			.await;
		resp.assert_status_not_found();
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn upsert_refuses_a_shared_placement_group() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let group_id = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'shared-grp');"
		))
		.await
		.expect("seed group");

		// Onboard onto shared-account backups, and learn the auto-named bucket.
		let resp = private
			.post("/api/backups/create_shared")
			.json(&serde_json::json!({ "server_group_id": group_id }))
			.await;
		resp.assert_status_ok();
		let shared_bucket = resp.json::<serde_json::Value>()["bucket"]
			.as_str()
			.expect("bucket")
			.to_string();

		// A pulumi upsert for the same group — using the *same* bucket, so the
		// bucket-immutability check would pass and (without the placement guard)
		// it would silently overwrite the shared roles. The guard makes it 409.
		let resp = private
			.post("/api/backups/upsert")
			.json(&serde_json::json!({
				"server_group_id": group_id,
				"bucket": shared_bucket,
				"prefix": "",
				"target_role_arn": "arn:aws:iam::123456789012:role/byo",
				"maintenance_role_arn": "arn:aws:iam::123456789012:role/byo-maint",
			}))
			.await;
		resp.assert_status_conflict();
		assert!(
			resp.text().contains("shared-account"),
			"expected the placement-guard message, got: {}",
			resp.text()
		);
	})
	.await;
}
