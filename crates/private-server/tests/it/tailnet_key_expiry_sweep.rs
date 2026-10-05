//! Integration tests for the `sweep_key_expiry` Tailscale sweep.
//!
//! Lives under private-server because it's the only crate that already
//! depends on both `commons-applications` (the sweep + directory live there)
//! and `commons-tests` (`TestDb`).

use commons_servers::{
	tailnet_directory::{DirectoryEntry, TailnetDirectory},
	tailnet_sweeps::{KEY_EXPIRY_REF, TAILSCALE_SOURCE, sweep_key_expiry},
};
use commons_tests::db::TestDb;
use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use database::issues::Issue;
use uuid::Uuid;

const NODE_ID: &str = "nodekey:keyexpiry-sweep";

fn directory_with(key_expiry_disabled: bool) -> TailnetDirectory {
	let ip: std::net::IpAddr = "100.64.0.77".parse().unwrap();
	TailnetDirectory::for_test([(
		ip,
		DirectoryEntry {
			node_id: NODE_ID.into(),
			node_name: "expiry-test.example.ts.net".into(),
			tailnet: "example.ts.net".into(),
			tags: vec!["tag:canopy-server".into()],
			addresses: vec![ip],
			last_seen: None,
			key_expiry_disabled,
		},
	)])
}

async fn insert_tailnet_device(conn: &mut AsyncPgConnection) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role, tailscale_node_id) \
		 VALUES ('{id}', 'server', '{NODE_ID}');"
	))
	.await
	.expect("insert device");
	id
}

async fn insert_server_for(conn: &mut AsyncPgConnection, device_id: Uuid, host: &str) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO machines (name, id, device_id) VALUES ('box', '{id}', '{device_id}'); \
		 INSERT INTO applications (id, host, type, machine_id) \
		 VALUES ('{id}', '{host}', 'tamanu-central', '{id}');"
	))
	.await
	.expect("insert server");
	id
}

/// The sweep speaks for the box, so its issue files at machine scope.
async fn issue_for(conn: &mut AsyncPgConnection, machine_id: Uuid) -> Option<Issue> {
	Issue::list_by_source_ref_for_machines(conn, TAILSCALE_SOURCE, KEY_EXPIRY_REF, &[machine_id])
		.await
		.expect("list issues")
		.into_iter()
		.next()
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_opens_critical_issue_when_key_expiry_enabled() {
	TestDb::run(async |mut conn, _| {
		let device_id = insert_tailnet_device(&mut conn).await;
		let server_id = insert_server_for(&mut conn, device_id, "https://srv1.invalid/").await;

		let dir = directory_with(false);
		let filed = sweep_key_expiry(&mut conn, &dir).await.expect("sweep");
		assert_eq!(filed, 1);

		let issue = issue_for(&mut conn, server_id).await.expect("issue");
		assert_eq!(
			issue.effective_result,
			Some(commons_types::status::CheckResult::Failed)
		);
		assert!(issue.escalates);
		assert_eq!(issue.device_id, Some(device_id));
		assert!(issue.active);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_skips_when_key_expiry_disabled() {
	TestDb::run(async |mut conn, _| {
		let device_id = insert_tailnet_device(&mut conn).await;
		let server_id = insert_server_for(&mut conn, device_id, "https://srv2.invalid/").await;

		let dir = directory_with(true);
		let filed = sweep_key_expiry(&mut conn, &dir).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for(&mut conn, server_id).await.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_closes_issue_when_operator_pins_key() {
	TestDb::run(async |mut conn, _| {
		let device_id = insert_tailnet_device(&mut conn).await;
		let server_id = insert_server_for(&mut conn, device_id, "https://srv3.invalid/").await;

		// First pass: expiry enabled, issue opens.
		sweep_key_expiry(&mut conn, &directory_with(false))
			.await
			.expect("first sweep");
		assert!(issue_for(&mut conn, server_id).await.unwrap().active);

		// Operator pins the key on the Tailscale side; next sweep should close.
		sweep_key_expiry(&mut conn, &directory_with(true))
			.await
			.expect("second sweep");
		assert!(!issue_for(&mut conn, server_id).await.unwrap().active);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_ignores_tailnet_device_with_no_server() {
	TestDb::run(async |mut conn, _| {
		// Tailnet-attached but no server: the tailnet has lots of these
		// (operator laptops, other infra) and we deliberately don't
		// touch them.
		let _device_id = insert_tailnet_device(&mut conn).await;

		let filed = sweep_key_expiry(&mut conn, &directory_with(false))
			.await
			.expect("sweep");
		assert_eq!(filed, 0);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn sweep_ignores_node_not_in_directory() {
	TestDb::run(async |mut conn, _| {
		// Device with an attached server but the directory snapshot
		// doesn't contain its node id (node left the tailnet or hasn't
		// been refreshed yet). The sweep must leave state alone — only
		// a real detach action should clear the attachment.
		let device_id = insert_tailnet_device(&mut conn).await;
		let server_id = insert_server_for(&mut conn, device_id, "https://srv4.invalid/").await;

		let empty = TailnetDirectory::for_test([]);
		let filed = sweep_key_expiry(&mut conn, &empty).await.expect("sweep");
		assert_eq!(filed, 0);
		assert!(issue_for(&mut conn, server_id).await.is_none());
	})
	.await
}
