//! Declaring maintenance over one of a group's environments through the
//! operator API, and how it reads back.
//!
//! spec: MNT

use commons_tests::diesel_async::SimpleAsyncConnection;
use serde_json::{Value, json};

const GROUP: &str = "dddddddd-0000-0000-0000-000000000001";
const PRODUCTION_BOX: &str = "dddddddd-0000-0000-0000-0000000000b1";
const CLONE_BOX: &str = "dddddddd-0000-0000-0000-0000000000b2";
const PRODUCTION: &str = "dddddddd-0000-0000-0000-0000000000a1";
const CLONE: &str = "dddddddd-0000-0000-0000-0000000000a2";

async fn seed(conn: &mut impl SimpleAsyncConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka');
		 INSERT INTO machines (id, group_id) VALUES
			('{PRODUCTION_BOX}', '{GROUP}'),
			('{CLONE_BOX}', '{GROUP}');
		 INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES
			('{PRODUCTION}', 'https://kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{PRODUCTION_BOX}'),
			('{CLONE}', 'https://clone.kamaka.example', 'tamanu-central', 'clone', '{GROUP}', '{CLONE_BOX}');"
	))
	.await
	.unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_window_is_named_and_kept_apart_from_the_groups() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		let in_an_hour = (jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1)).to_string();

		private
			.post("/api/maintenance/declare")
			.json(&json!({
				"server_group_id": GROUP,
				"rank": "clone",
				"expected_end": in_an_hour,
				"note": "rehearsing the upgrade",
			}))
			.await
			.assert_status_ok();

		let open: Vec<Value> = private
			.post("/api/maintenance/list_open")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(open.len(), 1);
		assert_eq!(open[0]["target"], "kamaka clone");
		assert_eq!(open[0]["window"]["rank"], "clone");

		// Only the clone reads as maintained; production is watched.
		let maintained = async |application: &str| -> bool {
			let detail: Value = private
				.post("/api/fleet/applications/get_detail")
				.json(&json!({ "server_id": application }))
				.await
				.json();
			detail["maintained"] == true
		};
		assert!(maintained(CLONE).await);
		assert!(!maintained(PRODUCTION).await);

		// The group's own window is a second target, not an amendment, and it
		// covers production too.
		private
			.post("/api/maintenance/declare")
			.json(&json!({ "server_group_id": GROUP, "expected_end": in_an_hour }))
			.await
			.assert_status_ok();
		let open: Vec<Value> = private
			.post("/api/maintenance/list_open")
			.json(&json!({}))
			.await
			.json();
		assert_eq!(open.len(), 2, "{open:?}");
		assert!(maintained(PRODUCTION).await);

		// A rank makes no sense for a window over one machine.
		private
			.post("/api/maintenance/declare")
			.json(&json!({ "machine_id": CLONE_BOX, "rank": "clone", "expected_end": in_an_hour }))
			.await
			.assert_status_bad_request();
	})
	.await;
}
