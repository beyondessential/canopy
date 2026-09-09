//! Endpoint tests for the environment inventory: the lease a run holds while
//! it runs, what it reads for one environment, and the refusals it has to be
//! told apart from Canopy being unreachable.
//!
//! spec: INV

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use serde_json::{Value, json};
use uuid::Uuid;

/// The login every test request authenticates as.
pub(crate) const ME: &str = "admin@localhost";

pub(crate) async fn insert_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{id}', '{name}')"
	))
	.await
	.expect("insert group");
	id
}

pub(crate) async fn insert_application(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	name: &str,
	r#type: &str,
	host: Option<&str>,
) -> Uuid {
	insert_ranked_application(conn, group, name, r#type, None, host).await
}

/// An application on a machine of its own, in a group. Seeded directly: a type
/// is reported rather than entered, so no operator flow creates one.
pub(crate) async fn insert_ranked_application(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	name: &str,
	r#type: &str,
	rank: Option<&str>,
	host: Option<&str>,
) -> Uuid {
	insert_application_on_its_own_machine(conn, group, name, r#type, rank, host)
		.await
		.0
}

/// An application on a machine of its own, in a group, answering both
/// identifiers. Seeded directly: a type is reported rather than entered, so no
/// operator flow creates one.
pub(crate) async fn insert_application_on_its_own_machine(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	name: &str,
	r#type: &str,
	rank: Option<&str>,
	host: Option<&str>,
) -> (Uuid, Uuid) {
	let id = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let ty = r#type;
	let host = host.map_or("NULL".to_string(), |h| format!("'{h}'"));
	let rank = rank.map_or("NULL".to_string(), |r| format!("'{r}'"));
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, name, group_id) VALUES ('{machine}', '{name}', '{group}');
		 INSERT INTO applications (id, name, type, rank, host, group_id, machine_id)
		 VALUES ('{id}', '{name}', '{ty}', {rank}, {host}, '{group}', '{machine}')"
	))
	.await
	.expect("insert application");
	(id, machine)
}

/// The identity speaks for the box, so the device binds to the machine the
/// application runs on.
pub(crate) async fn bind_device(
	conn: &mut AsyncPgConnection,
	application: Uuid,
	tailscale_name: &str,
) {
	let device = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role, tailscale_node_name)
		 VALUES ('{device}', 'server', '{tailscale_name}');
		 UPDATE machines SET device_id = '{device}'
		 WHERE id = (SELECT machine_id FROM applications WHERE id = '{application}')"
	))
	.await
	.expect("bind device");
}

/// Take the lease and answer its identifier, which the inventory read needs.
pub(crate) async fn take_lease(
	private: &commons_tests::axum_test::TestServer,
	args: Value,
) -> Uuid {
	let response = private.post("/api/inventory/take_lease").json(&args).await;
	response.assert_status_ok();
	let body: Value = response.json();
	body["id"]
		.as_str()
		.expect("lease id")
		.parse()
		.expect("lease id is a uuid")
}

/// Take the lease and read the inventory under it, the whole flow a run makes.
pub(crate) async fn read_inventory(
	private: &commons_tests::axum_test::TestServer,
	args: Value,
) -> Value {
	let lease = take_lease(private, args).await;
	let response = private
		.post("/api/inventory/for_group")
		.json(&json!({ "lease_id": lease }))
		.await;
	response.assert_status_ok();
	response.json()
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_an_environments_machines_and_applications() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-prod").await;
		let central = insert_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			None,
		)
		.await;
		bind_device(&mut conn, central, "kamaka-prod-central").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-prod-facility",
			"tamanu-facility",
			Some("https://facility.kamaka.example/"),
		)
		.await;

		let body = read_inventory(&private, json!({ "group": "kamaka-prod" })).await;

		assert_eq!(body["group"], "kamaka-prod");
		// Nothing here carries a rank, so the whole group is one environment at
		// the default rank.
		assert_eq!(body["rank"], "dev");

		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 2);

		// A bound device's tailnet name is the address, and the machine carries
		// the applications a run configures on it.
		assert_eq!(hosts[0]["name"], "kamaka-prod-central");
		assert_eq!(hosts[0]["address"], "kamaka-prod-central");
		assert_eq!(hosts[0]["applications"][0]["type"], "tamanu-central");
		assert_eq!(hosts[0]["applications"][0]["id"], central.to_string());

		// No device on the box, so the address is the recorded host of an
		// application on it, as a bare name.
		assert_eq!(hosts[1]["address"], "facility.kamaka.example");
	})
	.await
}

/// Rank is an application's, so two workloads on one box sit in two
/// environments and are configured apart. The box is one machine in each.
#[tokio::test(flavor = "multi_thread")]
async fn splits_a_shared_box_by_the_rank_of_each_application() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-shared").await;
		let machine = Uuid::new_v4();
		let device = Uuid::new_v4();
		let production = Uuid::new_v4();
		let demo = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role, tailscale_node_name)
			 VALUES ('{device}', 'server', 'kamaka-shared-box');
			 INSERT INTO machines (id, name, group_id, device_id)
			 VALUES ('{machine}', 'kamaka-shared-box', '{group}', '{device}');
			 INSERT INTO applications (id, name, type, rank, group_id, machine_id)
			 VALUES ('{production}', 'kamaka-central', 'tamanu-central', 'production', '{group}', '{machine}'),
			        ('{demo}', 'kamaka-demo-central', 'tamanu-central', 'demo', '{group}', '{machine}')"
		))
		.await
		.expect("seed a shared box");

		let body = read_inventory(
			&private,
			json!({ "server_group_id": group, "rank": "production" }),
		)
		.await;
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["id"], machine.to_string());
		assert_eq!(hosts[0]["address"], "kamaka-shared-box");
		// Only the production workload, though both run on this box.
		let applications = hosts[0]["applications"].as_array().expect("applications");
		assert_eq!(applications.len(), 1);
		assert_eq!(applications[0]["id"], production.to_string());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn serves_the_environment_at_the_rank_asked_for() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;

		let body = read_inventory(&private, json!({ "group": "kamaka", "rank": "demo" })).await;
		assert_eq!(body["rank"], "demo");
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-demo");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_group_holding_several_environments_with_no_rank_named() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "group": "kamaka" }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let body: Value = response.json();
		assert!(
			body["detail"]
				.as_str()
				.expect("detail")
				.contains("name the rank"),
			"{body}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_an_archived_application() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let gone =
			insert_application(&mut conn, group, "kamaka-old", "tamanu-facility", None).await;
		conn.batch_execute(&format!(
			"UPDATE applications SET deleted_at = NOW() WHERE id = '{gone}'"
		))
		.await
		.expect("archive");

		let body = read_inventory(&private, json!({ "group": "kamaka" })).await;
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-central");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_group_canopy_does_not_have() {
	commons_tests::server::run(async move |_conn, _public, private| {
		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "group": "nowhere" }))
			.await;
		response.assert_status(axum::http::StatusCode::NOT_FOUND);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_archived_group() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-gone").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		conn.batch_execute(&format!(
			"UPDATE server_groups SET deleted_at = NOW() WHERE id = '{group}'"
		))
		.await
		.expect("archive");

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "group": "kamaka-gone" }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let body: Value = response.json();
		assert!(
			body["detail"]
				.as_str()
				.expect("detail")
				.contains("archived"),
			"{body}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_group_with_nothing_to_configure() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		insert_group(&mut conn, "kamaka-empty").await;
		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "group": "kamaka-empty" }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_name_that_answers_for_two_groups() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let first = insert_group(&mut conn, "kamaka").await;
		insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, first, "kamaka-central", "tamanu-central", None).await;

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "group": "kamaka" }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let body: Value = response.json();
		assert!(
			body["detail"]
				.as_str()
				.expect("detail")
				.contains("ask by identifier"),
			"{body}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_request_naming_neither_a_group_nor_an_identifier() {
	commons_tests::server::run(async move |_conn, _public, private| {
		private
			.post("/api/inventory/take_lease")
			.json(&json!({}))
			.await
			.assert_status(axum::http::StatusCode::BAD_REQUEST);
	})
	.await
}

// --- leases ---

/// The lease is the interlock: an environment holds one, and a second
/// operator's run is refused while it holds.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_lease_another_operator_holds() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		conn.batch_execute(&format!(
			"INSERT INTO inventory_leases (server_group_id, rank, intent, held_by, note, expires_at)
			 VALUES ('{group}', 'dev', 'configure', 'someone.else@bes.au', 'rolling the certs',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let detail = response.json::<Value>()["detail"]
			.as_str()
			.expect("detail")
			.to_owned();
		assert!(detail.contains("someone.else@bes.au"), "{detail}");
		assert!(detail.contains("rolling the certs"), "{detail}");
	})
	.await
}

/// A run that dies stops holding the environment once its lease expires.
#[tokio::test(flavor = "multi_thread")]
async fn takes_a_lease_over_one_that_has_expired() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		conn.batch_execute(&format!(
			"INSERT INTO inventory_leases (server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() - INTERVAL '1 minute')"
		))
		.await
		.expect("seed an expired lease");

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await
			.assert_status_ok();
	})
	.await
}

/// Taking over another operator's lease is deliberate, so it needs asking for.
#[tokio::test(flavor = "multi_thread")]
async fn takes_a_lease_over_when_asked_to() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		conn.batch_execute(&format!(
			"INSERT INTO inventory_leases (server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group, "take_over": true }))
			.await;
		response.assert_status_ok();
		assert_eq!(response.json::<Value>()["held_by"], ME);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_the_inventory_to_someone_who_holds_no_lease() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO inventory_leases (id, server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{lease}', '{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_the_inventory_under_an_expired_lease() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		conn.batch_execute(&format!(
			"UPDATE inventory_leases SET expires_at = NOW() - INTERVAL '1 minute'
			 WHERE id = '{lease}'"
		))
		.await
		.expect("expire the lease");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": lease }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("expired"),
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn extending_keeps_the_environment_and_releasing_gives_it_back() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;

		conn.batch_execute(&format!(
			"UPDATE inventory_leases SET expires_at = NOW() + INTERVAL '1 minute'
			 WHERE id = '{lease}'"
		))
		.await
		.expect("bring the expiry in");
		let extended = private
			.post("/api/inventory/extend_lease")
			.json(&json!({ "lease_id": lease }))
			.await;
		extended.assert_status_ok();

		private
			.post("/api/inventory/release_lease")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status_ok();
		// Released, so it is no longer the environment's open lease.
		private
			.post("/api/inventory/release_lease")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status(axum::http::StatusCode::NOT_FOUND);
	})
	.await
}

/// An expired lease is over: reviving it would skip every gate taking one
/// applies, so a run that has lapsed takes a new one.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_to_extend_a_lease_that_has_expired() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		conn.batch_execute(&format!(
			"UPDATE inventory_leases SET expires_at = NOW() - INTERVAL '1 minute'
			 WHERE id = '{lease}'"
		))
		.await
		.expect("expire the lease");

		let response = private
			.post("/api/inventory/extend_lease")
			.json(&json!({ "lease_id": lease }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("expired"),
		);
	})
	.await
}

/// The group page reads who holds an environment without taking it.
#[tokio::test(flavor = "multi_thread")]
async fn reports_the_lease_holding_an_environment() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		let before = read_run_state(&private, group).await;
		assert_eq!(before["lease"], Value::Null);
		assert_eq!(before["window"], Value::Null);
		assert_eq!(before["refuses"], false);

		take_lease(&private, json!({ "server_group_id": group })).await;
		let after = read_run_state(&private, group).await;
		assert_eq!(after["lease"]["held_by"], ME);
	})
	.await
}

/// The page reads the windows canopy refuses a lease for, which are those over
/// the environment's machines as well as those over the group.
#[tokio::test(flavor = "multi_thread")]
async fn reports_a_window_over_a_machine_in_the_environment() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let central =
			insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_machine_window(
			&mut conn,
			central,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;

		let state = read_run_state(&private, group).await;
		assert_eq!(state["window"]["declared_by"], "someone.else@bes.au");
		assert_eq!(state["refuses"], true);
	})
	.await
}

/// A window the reader declared themselves holds the page's answer without
/// refusing them, since taking the lease under it is the point of declaring it.
#[tokio::test(flavor = "multi_thread")]
async fn reads_the_readers_own_window_as_no_refusal() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(&mut conn, group, ME, "NOW() + INTERVAL '2 hours'").await;

		let state = read_run_state(&private, group).await;
		assert_eq!(state["window"]["declared_by"], ME);
		assert_eq!(state["refuses"], false);
	})
	.await
}

/// Someone else's window refuses the take whatever else holds, so the page
/// reports theirs rather than the reader's own and agrees with the refusal.
#[tokio::test(flavor = "multi_thread")]
async fn reports_the_window_that_refuses_over_the_readers_own() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let central =
			insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(&mut conn, group, ME, "NOW() + INTERVAL '2 hours'").await;
		declare_machine_window(
			&mut conn,
			central,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;

		let state = read_run_state(&private, group).await;
		assert_eq!(state["window"]["declared_by"], "someone.else@bes.au");
		assert_eq!(state["refuses"], true);

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

/// A window past its expected end holds nothing, the same reading `take_lease`
/// takes, so the page offers the run rather than reporting work under way.
#[tokio::test(flavor = "multi_thread")]
async fn reads_a_window_past_its_end_as_over() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(
			&mut conn,
			group,
			"someone.else@bes.au",
			"NOW() - INTERVAL '1 minute'",
		)
		.await;

		let state = read_run_state(&private, group).await;
		assert_eq!(state["window"], Value::Null);
		assert_eq!(state["refuses"], false);
	})
	.await
}

async fn read_run_state(private: &commons_tests::axum_test::TestServer, group: Uuid) -> Value {
	let response = private
		.post("/api/inventory/run_state")
		.json(&json!({ "server_group_id": group, "rank": "dev" }))
		.await;
	response.assert_status_ok();
	response.json()
}

// --- work under way ---

async fn declare_group_window(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	declared_by: &str,
	expected_end: &str,
) {
	conn.batch_execute(&format!(
		"INSERT INTO maintenance_windows (server_group_id, expected_end, declared_by)
		 VALUES ('{group}', {expected_end}, '{declared_by}')"
	))
	.await
	.expect("declare window");
}

async fn declare_machine_window(
	conn: &mut AsyncPgConnection,
	application: Uuid,
	declared_by: &str,
	expected_end: &str,
) {
	conn.batch_execute(&format!(
		"INSERT INTO maintenance_windows (machine_id, expected_end, declared_by)
		 SELECT machine_id, {expected_end}, '{declared_by}'
		 FROM applications WHERE id = '{application}'"
	))
	.await
	.expect("declare window");
}

async fn amend_window(conn: &mut AsyncPgConnection, group: Uuid, amended_by: &str) {
	conn.batch_execute(&format!(
		"UPDATE maintenance_windows SET amended_by = '{amended_by}', amended_at = NOW()
		 WHERE server_group_id = '{group}' AND ended_at IS NULL"
	))
	.await
	.expect("amend window");
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_lease_under_a_window_someone_else_declared() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-busy").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(
			&mut conn,
			group,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let detail = response.json::<Value>()["detail"]
			.as_str()
			.expect("detail")
			.to_owned();
		assert!(detail.contains("someone.else@bes.au"), "{detail}");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn takes_a_lease_under_the_readers_own_window() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-mine").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(&mut conn, group, ME, "NOW() + INTERVAL '2 hours'").await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await
			.assert_status_ok();
	})
	.await
}

/// A second operator declaring over an open window amends it rather than
/// opening one of their own, so the amendment is their declaration of the same
/// work and the environment is theirs to run on.
#[tokio::test(flavor = "multi_thread")]
async fn takes_a_lease_under_a_window_the_reader_amended() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(
			&mut conn,
			group,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;
		amend_window(&mut conn, group, ME).await;

		let state = read_run_state(&private, group).await;
		assert_eq!(state["window"]["amended_by"], ME);
		assert_eq!(state["refuses"], false);

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await
			.assert_status_ok();
	})
	.await
}

/// The amendment that stands is the one that speaks for the window, so an
/// operator it does not belong to is refused and told who to wait for.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_lease_under_a_window_someone_else_amended() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(
			&mut conn,
			group,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;
		amend_window(&mut conn, group, "third.party@bes.au").await;

		let state = read_run_state(&private, group).await;
		assert_eq!(state["refuses"], true);

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let detail = response.json::<Value>()["detail"]
			.as_str()
			.expect("detail")
			.to_owned();
		assert!(detail.contains("third.party@bes.au"), "{detail}");
	})
	.await
}

/// A window over one machine refuses the whole environment, a run acting on it
/// as a whole.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_lease_under_a_window_over_one_machine() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let facility =
			insert_application(&mut conn, group, "kamaka-facility", "tamanu-facility", None).await;
		declare_machine_window(
			&mut conn,
			facility,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

/// A window over a machine none of the environment's applications run on
/// refuses nothing.
#[tokio::test(flavor = "multi_thread")]
async fn ignores_a_window_over_a_machine_at_another_rank() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		let demo = insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;
		declare_machine_window(
			&mut conn,
			demo,
			"someone.else@bes.au",
			"NOW() + INTERVAL '2 hours'",
		)
		.await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group, "rank": "production" }))
			.await
			.assert_status_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn takes_a_lease_once_a_window_has_passed_its_end() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		declare_group_window(
			&mut conn,
			group,
			"someone.else@bes.au",
			"NOW() - INTERVAL '5 minutes'",
		)
		.await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await
			.assert_status_ok();
	})
	.await
}

// --- planned upgrades ---

async fn plan_upgrade(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	extra_columns: &str,
	extra_values: &str,
) {
	let version = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status)
		 VALUES ('{version}', 2, 63, 0, '', 'published');
		 INSERT INTO upgrade_plans (group_id, target_version_id{extra_columns})
		 VALUES ('{group}', '{version}'{extra_values})"
	))
	.await
	.expect("plan upgrade");
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_unplanned_upgrade_of_production() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-unplanned").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group, "intent": "upgrade" }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		assert!(
			response.json::<Value>()["detail"]
				.as_str()
				.expect("detail")
				.contains("no upgrade plan"),
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn takes_a_planned_upgrade_of_production() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-planned").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		plan_upgrade(&mut conn, group, "", "").await;

		let response = private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group, "intent": "upgrade" }))
			.await;
		response.assert_status_ok();
		assert_eq!(response.json::<Value>()["intent"], "upgrade");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_an_upgrade_whose_plan_was_withdrawn() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-withdrawn").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		plan_upgrade(&mut conn, group, ", withdrawn_at", ", NOW()").await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group, "intent": "upgrade" }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn takes_an_unplanned_configuration_run_on_production() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-configure").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await
			.assert_status_ok();
	})
	.await
}

/// A plan is for where a deployment's real users are, so another rank needs
/// none.
#[tokio::test(flavor = "multi_thread")]
async fn takes_an_unplanned_upgrade_at_another_rank() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-demo").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-demo-central",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;

		private
			.post("/api/inventory/take_lease")
			.json(&json!({ "server_group_id": group, "intent": "upgrade" }))
			.await
			.assert_status_ok();
	})
	.await
}

/// An operator whose lease was taken over is walking into work somebody else
/// has started, which the refusal has to say rather than reading as an expiry.
#[tokio::test(flavor = "multi_thread")]
async fn says_who_took_a_lease_over_rather_than_calling_it_expired() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let mine = take_lease(&private, json!({ "server_group_id": group })).await;
		conn.batch_execute(&format!(
			"UPDATE inventory_leases SET released_at = NOW(), released_by = 'someone.else@bes.au'
			 WHERE id = '{mine}'"
		))
		.await
		.expect("take the lease over");

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "lease_id": mine }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let detail = response.json::<Value>()["detail"]
			.as_str()
			.expect("detail")
			.to_owned();
		assert!(
			detail.contains("taken over by someone.else@bes.au"),
			"{detail}"
		);
	})
	.await
}

/// Canopy's own nil application is not a machine a run configures.
#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_the_meta_application() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		conn.batch_execute(&format!(
			"UPDATE machines SET group_id = '{group}'
			 WHERE id = '00000000-0000-0000-0000-000000000000';
			 UPDATE applications SET group_id = '{group}'
			 WHERE id = '00000000-0000-0000-0000-000000000000'"
		))
		.await
		.expect("put the meta application in the group");

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["name"], "kamaka-central");
	})
	.await
}

/// Canopy holding no address is served as none rather than as a guess, which
/// is what a variable has to supply.
#[tokio::test(flavor = "multi_thread")]
async fn serves_no_address_where_canopy_holds_none() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		let body = read_inventory(&private, json!({ "server_group_id": group })).await;
		assert_eq!(body["hosts"][0]["address"], Value::Null);
	})
	.await
}

/// Extending is the holder's, so a lease cannot be kept alive by whoever is
/// waiting for it.
#[tokio::test(flavor = "multi_thread")]
async fn refuses_to_extend_a_lease_someone_else_holds() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO inventory_leases (id, server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{lease}', '{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		private
			.post("/api/inventory/extend_lease")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}
