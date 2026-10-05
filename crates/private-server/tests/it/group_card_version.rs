//! The status group card's headline version must come from the highest-ranked,
//! highest-kind member (e.g. the production central), not whichever member
//! reported most recently.

use commons_tests::diesel_async::SimpleAsyncConnection;
use serde_json::{Value, json};

#[tokio::test(flavor = "multi_thread")]
async fn group_card_version_is_from_highest_rank_kind_member() {
	commons_tests::server::run(async |mut conn, _, private| {
		let group_id = "aaaaaaaa-0000-0000-0000-000000000001".parse().unwrap();

		// Set up the group and its members, then recompute the cache through the
		// real path (raw INSERTs alone bypass `recompute_version`, leaving
		// `version_server_id` NULL and the trigger a no-op).
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status)
				VALUES (2, 10, 0, 'x', 'published');
			INSERT INTO server_groups (id, name)
				VALUES ('aaaaaaaa-0000-0000-0000-000000000001', 'Group');
			INSERT INTO machines (name, id, group_id) VALUES
				('box', 'aaaaaaaa-0000-0000-0000-0000000000c0',
				 'aaaaaaaa-0000-0000-0000-000000000001'),
				('box', 'aaaaaaaa-0000-0000-0000-0000000000d0',
				 'aaaaaaaa-0000-0000-0000-000000000001');
			INSERT INTO applications (id, name, host, type, rank, group_id, machine_id) VALUES
				('aaaaaaaa-0000-0000-0000-0000000000c0', 'prod-central',
				 'https://prod.example.com', 'tamanu-central', 'production',
				 'aaaaaaaa-0000-0000-0000-000000000001',
				 'aaaaaaaa-0000-0000-0000-0000000000c0'),
				('aaaaaaaa-0000-0000-0000-0000000000d0', 'dev-facility',
				 'https://dev.example.com', 'tamanu-facility', 'dev',
				 'aaaaaaaa-0000-0000-0000-000000000001',
				 'aaaaaaaa-0000-0000-0000-0000000000d0');",
		)
		.await
		.unwrap();

		database::server_groups::ServerGroup::recompute_version(&mut conn, group_id)
			.await
			.unwrap();

		// The dev/facility server reports MORE recently, with a different
		// version; the prod/central reports earlier. The card must show the
		// prod/central version regardless of recency: the trigger only updates
		// the cache for the cached canonical member (prod-central).
		conn.batch_execute(
			"INSERT INTO statuses (server_id, created_at, version, healthy, health) VALUES
				('aaaaaaaa-0000-0000-0000-0000000000c0', NOW() - INTERVAL '1 hour',
				 '2.10.0', true, '[]'::jsonb),
				('aaaaaaaa-0000-0000-0000-0000000000d0', NOW() - INTERVAL '1 minute',
				 '2.99.0', true, '[]'::jsonb);",
		)
		.await
		.unwrap();

		let resp = private
			.post("/api/statuses/group_details")
			.json(&json!({ "server_group_id": "aaaaaaaa-0000-0000-0000-000000000001" }))
			.await;
		resp.assert_status_ok();
		let card: Value = resp.json();
		assert_eq!(
			card["version"], "2.10.0",
			"card shows the production-central version, not the most-recent (dev) one",
		);
	})
	.await;
}

/// A fresh Canopy instance — or one where every version is still draft — has no
/// published version to measure against. That's an unknown distance, not a
/// missing group: 404ing here blanks the whole status board.
#[tokio::test(flavor = "multi_thread")]
async fn group_card_is_served_when_no_version_is_published() {
	commons_tests::server::run(async |mut conn, _, private| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status)
				VALUES (2, 10, 0, 'unreleased', 'draft');
			INSERT INTO server_groups (id, name)
				VALUES ('bbbbbbbb-0000-0000-0000-000000000001', 'Fresh');
			INSERT INTO machines (name, id, group_id) VALUES
				('box', 'bbbbbbbb-0000-0000-0000-0000000000c0',
				 'bbbbbbbb-0000-0000-0000-000000000001');
			INSERT INTO applications (id, name, host, type, rank, group_id, machine_id) VALUES
				('bbbbbbbb-0000-0000-0000-0000000000c0', 'central',
				 'https://fresh.example.com', 'tamanu-central', 'production',
				 'bbbbbbbb-0000-0000-0000-000000000001',
				 'bbbbbbbb-0000-0000-0000-0000000000c0');",
		)
		.await
		.unwrap();

		let resp = private
			.post("/api/statuses/group_details")
			.json(&json!({ "server_group_id": "bbbbbbbb-0000-0000-0000-000000000001" }))
			.await;
		resp.assert_status_ok();
		let card: Value = resp.json();
		assert_eq!(card["name"], "Fresh");
		assert!(
			card["version_distance"].is_null(),
			"no published version to compare against means unknown distance",
		);
	})
	.await;
}
