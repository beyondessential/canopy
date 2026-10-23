//! Endpoint tests for what an environment serves Ansible: the lease a run holds while
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
	// A rank is held by every application on a box, and a pending one is in no
	// environment, so these fixtures give an application no asked-for rank dev,
	// the lowest environment.
	let rank = format!("'{}'", rank.unwrap_or("dev"));
	conn.batch_execute(&format!(
		"INSERT INTO machines (id, name, group_id) VALUES ('{machine}', '{name}', '{group}');
		 INSERT INTO applications (id, name, type, rank, host, group_id, machine_id, registered_at)
		 VALUES ('{id}', '{name}', '{ty}', {rank}, {host}, '{group}', '{machine}', NOW())"
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

/// Take the lease and answer its identifier, which the read needs.
pub(crate) async fn take_lease(
	private: &commons_tests::axum_test::TestServer,
	args: Value,
) -> Uuid {
	let response = private.post("/api/ansible/take_lease").json(&args).await;
	response.assert_status_ok();
	let body: Value = response.json();
	body["id"]
		.as_str()
		.expect("lease id")
		.parse()
		.expect("lease id is a uuid")
}

/// Take the lease and read what is served under it, the whole flow a run makes.
pub(crate) async fn read_served(
	private: &commons_tests::axum_test::TestServer,
	args: Value,
) -> Value {
	let lease = take_lease(private, args).await;
	let response = private
		.post("/api/ansible/for_group")
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

		let body = read_served(&private, json!({ "group": "kamaka-prod" })).await;

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

/// A box serves one environment, so both workloads on a shared box sit in it and
/// the box is one host carrying both.
#[tokio::test(flavor = "multi_thread")]
async fn serves_a_shared_box_whole() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-shared").await;
		let machine = Uuid::new_v4();
		let device = Uuid::new_v4();
		let central = Uuid::new_v4();
		let database = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role, tailscale_node_name)
			 VALUES ('{device}', 'server', 'kamaka-shared-box');
			 INSERT INTO machines (id, name, group_id, device_id)
			 VALUES ('{machine}', 'kamaka-shared-box', '{group}', '{device}');
			 INSERT INTO applications (id, name, type, rank, group_id, machine_id)
			 VALUES ('{central}', 'kamaka-central', 'tamanu-central', 'production', '{group}', '{machine}'),
			        ('{database}', 'kamaka-database', 'postgres', 'production', '{group}', '{machine}')"
		))
		.await
		.expect("seed a shared box");

		let body = read_served(
			&private,
			json!({ "server_group_id": group, "rank": "production" }),
		)
		.await;
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1);
		assert_eq!(hosts[0]["id"], machine.to_string());
		assert_eq!(hosts[0]["address"], "kamaka-shared-box");
		let applications = hosts[0]["applications"].as_array().expect("applications");
		assert_eq!(applications.len(), 2, "the whole box is in the environment");
	})
	.await
}

/// A group with nothing ranked has no environment, so there is nothing to serve
/// and no run to take a lease for.
// spec: GRP#environments
#[tokio::test(flavor = "multi_thread")]
async fn a_group_with_nothing_ranked_has_no_environment_to_serve() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka-pending").await;
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id) VALUES ('{machine}', 'kamaka-box', '{group}');
			 INSERT INTO applications (id, name, type, group_id, machine_id)
			 VALUES (gen_random_uuid(), 'kamaka-central', 'tamanu-central', '{group}', '{machine}')"
		))
		.await
		.expect("seed a pending box");

		let refused = private
			.post("/api/ansible/take_lease")
			.json(&json!({ "server_group_id": group }))
			.await;
		assert!(
			refused.status_code().is_client_error(),
			"{}",
			refused.text()
		);
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

		let body = read_served(&private, json!({ "group": "kamaka", "rank": "demo" })).await;
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
			.post("/api/ansible/take_lease")
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

		let body = read_served(&private, json!({ "group": "kamaka" })).await;
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			"INSERT INTO ansible_leases (server_group_id, rank, intent, held_by, note, expires_at)
			 VALUES ('{group}', 'dev', 'configure', 'someone.else@bes.au', 'rolling the certs',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		let response = private
			.post("/api/ansible/take_lease")
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
			"INSERT INTO ansible_leases (server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() - INTERVAL '1 minute')"
		))
		.await
		.expect("seed an expired lease");

		private
			.post("/api/ansible/take_lease")
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
			"INSERT INTO ansible_leases (server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		let response = private
			.post("/api/ansible/take_lease")
			.json(&json!({ "server_group_id": group, "take_over": true }))
			.await;
		response.assert_status_ok();
		assert_eq!(response.json::<Value>()["held_by"], ME);
	})
	.await
}

/// Every row canopy writes against a group holds `FOR KEY SHARE` on it until
/// that transaction commits, and the backup work writes them continuously, so
/// a take must not wait on a lock those conflict with.
#[tokio::test(flavor = "multi_thread")]
async fn takes_a_lease_while_another_transaction_writes_against_the_group() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;

		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"BEGIN;
			 INSERT INTO machines (id, name, group_id)
			 VALUES ('{machine}', 'kamaka-facility', '{group}')"
		))
		.await
		.expect("open a writer against the group");

		let response = tokio::time::timeout(std::time::Duration::from_secs(5), async {
			private
				.post("/api/ansible/take_lease")
				.json(&json!({ "server_group_id": group }))
				.await
		})
		.await
		.expect("take_lease queued behind the open writer");
		response.assert_status_ok();

		conn.batch_execute("ROLLBACK").await.expect("rollback");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_the_environment_to_someone_who_holds_no_lease() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO ansible_leases (id, server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{lease}', '{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		private
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn refuses_the_environment_under_an_expired_lease() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let lease = take_lease(&private, json!({ "server_group_id": group })).await;
		conn.batch_execute(&format!(
			"UPDATE ansible_leases SET expires_at = NOW() - INTERVAL '1 minute'
			 WHERE id = '{lease}'"
		))
		.await
		.expect("expire the lease");

		let response = private
			.post("/api/ansible/for_group")
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
			"UPDATE ansible_leases SET expires_at = NOW() + INTERVAL '1 minute'
			 WHERE id = '{lease}'"
		))
		.await
		.expect("bring the expiry in");
		let extended = private
			.post("/api/ansible/extend_lease")
			.json(&json!({ "lease_id": lease }))
			.await;
		extended.assert_status_ok();

		private
			.post("/api/ansible/release_lease")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status_ok();
		// Released, so it is no longer the environment's open lease.
		private
			.post("/api/ansible/release_lease")
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
			"UPDATE ansible_leases SET expires_at = NOW() - INTERVAL '1 minute'
			 WHERE id = '{lease}'"
		))
		.await
		.expect("expire the lease");

		let response = private
			.post("/api/ansible/extend_lease")
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

async fn read_run_state(private: &commons_tests::axum_test::TestServer, group: Uuid) -> Value {
	let response = private
		.post("/api/ansible/run_state")
		.json(&json!({ "server_group_id": group, "rank": "dev" }))
		.await;
	response.assert_status_ok();
	response.json()
}

// --- work under way ---

const OTHER: &str = "someone.else@bes.au";
const THIRD: &str = "third.party@bes.au";
const IN_TWO_HOURS: &str = "NOW() + INTERVAL '2 hours'";
const FIVE_MINUTES_AGO: &str = "NOW() - INTERVAL '5 minutes'";

enum Over {
	Group,
	Rank(&'static str),
	Machine(Uuid),
}

struct Declared {
	over: Over,
	by: &'static str,
	ends: &'static str,
	amended_by: Option<&'static str>,
}

impl Declared {
	fn new(over: Over, by: &'static str, ends: &'static str) -> Self {
		Self {
			over,
			by,
			ends,
			amended_by: None,
		}
	}

	fn amended_by(mut self, amended_by: &'static str) -> Self {
		self.amended_by = Some(amended_by);
		self
	}
}

async fn declare(conn: &mut AsyncPgConnection, group: Uuid, window: &Declared) {
	let (columns, values) = match window.over {
		Over::Group => ("server_group_id", format!("'{group}'")),
		Over::Rank(rank) => ("server_group_id, rank", format!("'{group}', '{rank}'")),
		Over::Machine(machine) => ("machine_id", format!("'{machine}'")),
	};
	let (ends, by) = (window.ends, window.by);
	conn.batch_execute(&format!(
		"INSERT INTO maintenance_windows ({columns}, expected_end, declared_by)
		 VALUES ({values}, {ends}, '{by}')"
	))
	.await
	.expect("declare window");
	if let Some(amended_by) = window.amended_by {
		conn.batch_execute(&format!(
			"UPDATE maintenance_windows SET amended_by = '{amended_by}', amended_at = NOW()
			 WHERE declared_by = '{by}' AND ended_at IS NULL"
		))
		.await
		.expect("amend window");
	}
}

enum Take {
	Takes,
	Refused(&'static [&'static str]),
}

/// The window the page reports for an environment: who declared it, and who
/// amended it where someone did.
type Reported = Option<(&'static str, Option<&'static str>)>;

/// A window refuses a lease when it holds over the environment, for anyone but
/// the operator it belongs to, and the page that reads the refusal says so under
/// the same reading: over the group, the environment's rank, or any machine at
/// the rank, whether or not a run can reach that machine yet, and not one over
/// another of the group's environments or past its end. An amendment that
/// stands is the one that speaks for the window. Each row seeds its windows over
/// a fixed group and probes both environments.
// spec: ANSRUN#work-under-way
#[tokio::test(flavor = "multi_thread")]
async fn a_window_refuses_a_lease_exactly_where_the_page_reports_it() {
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
		let (_, facility) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-facility",
			"tamanu-facility",
			Some("production"),
			None,
		)
		.await;
		let (_, demo) = insert_application_on_its_own_machine(
			&mut conn,
			group,
			"kamaka-demo",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;
		let spare = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (name, id, group_id, rank)
			 VALUES ('kamaka-spare', '{spare}', '{group}', 'production')"
		))
		.await
		.expect("insert spare machine");

		let rows: Vec<(&str, Vec<Declared>, Vec<(&str, Take, Reported)>)> = vec![
			(
				"no window",
				vec![],
				vec![
					("production", Take::Takes, None),
					("demo", Take::Takes, None),
				],
			),
			(
				"another operator's window over the group",
				vec![Declared::new(Over::Group, OTHER, IN_TWO_HOURS)],
				vec![
					("production", Take::Refused(&[OTHER]), Some((OTHER, None))),
					("demo", Take::Refused(&[OTHER]), Some((OTHER, None))),
				],
			),
			(
				"the reader's own window over the group",
				vec![Declared::new(Over::Group, ME, IN_TWO_HOURS)],
				vec![("production", Take::Takes, Some((ME, None)))],
			),
			(
				"another operator's window the reader amended",
				vec![Declared::new(Over::Group, OTHER, IN_TWO_HOURS).amended_by(ME)],
				vec![("production", Take::Takes, Some((OTHER, Some(ME))))],
			),
			(
				"a window a third operator amended",
				vec![Declared::new(Over::Group, OTHER, IN_TWO_HOURS).amended_by(THIRD)],
				vec![(
					"production",
					Take::Refused(&[THIRD]),
					Some((OTHER, Some(THIRD))),
				)],
			),
			(
				"a window over one machine of the environment",
				vec![Declared::new(
					Over::Machine(facility),
					OTHER,
					IN_TWO_HOURS,
				)],
				vec![
					(
						"production",
						Take::Refused(&[OTHER, "kamaka-facility"]),
						Some((OTHER, None)),
					),
					("demo", Take::Takes, None),
				],
			),
			(
				"a window over a machine no run reaches yet",
				vec![Declared::new(Over::Machine(spare), OTHER, IN_TWO_HOURS)],
				vec![
					(
						"production",
						Take::Refused(&[OTHER, "kamaka-spare"]),
						Some((OTHER, None)),
					),
					("demo", Take::Takes, None),
				],
			),
			(
				"a window over a machine at another rank",
				vec![Declared::new(Over::Machine(demo), OTHER, IN_TWO_HOURS)],
				vec![
					("production", Take::Takes, None),
					("demo", Take::Refused(&[OTHER]), Some((OTHER, None))),
				],
			),
			(
				"a window over another environment of the group",
				vec![Declared::new(Over::Rank("demo"), OTHER, IN_TWO_HOURS)],
				vec![
					("production", Take::Takes, None),
					("demo", Take::Refused(&[OTHER]), Some((OTHER, None))),
				],
			),
			(
				"a window past its end",
				vec![Declared::new(Over::Group, OTHER, FIVE_MINUTES_AGO)],
				vec![("production", Take::Takes, None)],
			),
			(
				"the reader's own window over the group and another's over a machine",
				vec![
					Declared::new(Over::Group, ME, IN_TWO_HOURS),
					Declared::new(Over::Machine(facility), OTHER, IN_TWO_HOURS),
				],
				vec![("production", Take::Refused(&[OTHER]), Some((OTHER, None)))],
			),
		];

		let mut failures: Vec<String> = Vec::new();
		for (row, windows, probes) in rows {
			conn.batch_execute("DELETE FROM maintenance_windows")
				.await
				.expect("clear windows");
			for window in &windows {
				declare(&mut conn, group, window).await;
			}

			for (rank, take, reported) in probes {
				let at = format!("{row}, at {rank}");
				let environment = json!({ "server_group_id": group, "rank": rank });

				let state = private.post("/api/ansible/run_state").json(&environment).await;
				if !state.status_code().is_success() {
					failures.push(format!("{at}: run_state answered {}", state.text()));
					continue;
				}
				let state: Value = state.json();
				let window = &state["window"];
				let reports_as_expected = match reported {
					None => window.is_null(),
					Some((declared_by, amended_by)) => {
						window["declared_by"] == declared_by
							&& window["amended_by"] == amended_by.map_or(Value::Null, Value::from)
					}
				};
				if !reports_as_expected {
					failures.push(format!("{at}: run_state reported the window {window}"));
				}

				let response = private
					.post("/api/ansible/take_lease")
					.json(&environment)
					.await;
				let status = response.status_code();
				let refused = status == axum::http::StatusCode::CONFLICT;
				if state["refuses"].as_bool() != Some(refused) {
					failures.push(format!(
						"{at}: run_state says refuses = {}, but take_lease answered {status}",
						state["refuses"]
					));
				}

				match take {
					Take::Takes => {
						if status != axum::http::StatusCode::OK {
							failures.push(format!("{at}: take_lease answered {status}"));
							continue;
						}
						let lease: Value = response.json();
						let released = private
							.post("/api/ansible/release_lease")
							.json(&json!({ "lease_id": lease["id"] }))
							.await;
						if !released.status_code().is_success() {
							failures.push(format!("{at}: release_lease answered {}", released.text()));
						}
					}
					Take::Refused(needles) => {
						let detail = response.json::<Value>()["detail"]
							.as_str()
							.unwrap_or_default()
							.to_owned();
						if !refused {
							failures.push(format!("{at}: take_lease answered {status}, not 409"));
						}
						for needle in needles {
							if !detail.contains(needle) {
								failures.push(format!("{at}: refusal does not name {needle}: {detail}"));
							}
						}
					}
				}
			}
		}
		assert!(failures.is_empty(), "{}", failures.join("\n"));
	})
	.await
}

// --- planned upgrades ---

pub(crate) async fn plan_upgrade(
	conn: &mut AsyncPgConnection,
	group: Uuid,
	extra_columns: &str,
	extra_values: &str,
) {
	let version = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status)
		 VALUES ('{version}', 2, 63, 0, '', 'published');
		 INSERT INTO upgrade_plans (group_id, target_version_id, rank{extra_columns})
		 VALUES ('{group}', '{version}', 'production'{extra_values})"
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
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
			.post("/api/ansible/take_lease")
			.json(&json!({ "server_group_id": group, "intent": "upgrade" }))
			.await
			.assert_status_ok();
	})
	.await
}

/// An operator whose lease was taken over is walking into work somebody else
/// has started, which the refusal has to say rather than reading as an expiry,
/// and one whose lease was released unread is told that, not that it expired.
#[tokio::test(flavor = "multi_thread")]
async fn says_why_a_lease_no_longer_holds_rather_than_calling_it_expired() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_application(&mut conn, group, "kamaka-central", "tamanu-central", None).await;
		let mine = take_lease(&private, json!({ "server_group_id": group })).await;
		conn.batch_execute(&format!(
			"UPDATE ansible_leases SET released_at = NOW(), released_by = 'someone.else@bes.au'
			 WHERE id = '{mine}'"
		))
		.await
		.expect("take the lease over");

		let response = private
			.post("/api/ansible/for_group")
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

		let unread = take_lease(&private, json!({ "server_group_id": group })).await;
		conn.batch_execute(&format!(
			"UPDATE ansible_leases SET released_at = NOW(), release_reason = 'unstarted'
			 WHERE id = '{unread}'"
		))
		.await
		.expect("release the lease unread");
		let response = private
			.post("/api/ansible/for_group")
			.json(&json!({ "lease_id": unread }))
			.await;
		response.assert_status(axum::http::StatusCode::CONFLICT);
		let detail = response.json::<Value>()["detail"]
			.as_str()
			.expect("detail")
			.to_owned();
		assert!(detail.contains("no run read the environment"), "{detail}");
		assert!(!detail.contains("expired"), "{detail}");
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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

		let body = read_served(&private, json!({ "server_group_id": group })).await;
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
			"INSERT INTO ansible_leases (id, server_group_id, rank, intent, held_by, expires_at)
			 VALUES ('{lease}', '{group}', 'dev', 'configure', 'someone.else@bes.au',
			         NOW() + INTERVAL '20 minutes')"
		))
		.await
		.expect("seed a lease");

		private
			.post("/api/ansible/extend_lease")
			.json(&json!({ "lease_id": lease }))
			.await
			.assert_status(axum::http::StatusCode::CONFLICT);
	})
	.await
}

/// A machine ranked before anything has reported is already in its
/// environment, and a bound device is enough for a run to reach it, so a run is
/// how its applications get there.
// spec: INV#what-a-run-is-served
#[tokio::test(flavor = "multi_thread")]
async fn serves_a_machine_with_a_bound_device_and_nothing_on_it() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		let machine = Uuid::new_v4();
		let device = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO devices (id, role, tailscale_node_name)
			 VALUES ('{device}', 'server', 'kamaka-new.tail1234.ts.net');
			 INSERT INTO machines (id, name, group_id, rank, device_id)
			 VALUES ('{machine}', 'kamaka-new', '{group}', 'production', '{device}')"
		))
		.await
		.expect("seed a bare machine");

		let body = read_served(&private, json!({ "group": "kamaka" })).await;
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1, "{body}");
		assert_eq!(hosts[0]["id"], machine.to_string());
		assert_eq!(hosts[0]["applications"], json!([]));
		assert_eq!(hosts[0]["host_name"], "kamaka-new");
	})
	.await
}

/// A ranked machine with no device and nothing reported has no address a run
/// could reach, so it is left out rather than refusing the environment.
// spec: INV#what-a-run-is-served
#[tokio::test(flavor = "multi_thread")]
async fn leaves_out_a_machine_a_run_cannot_reach() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-central",
			"tamanu-central",
			Some("production"),
			Some("https://central.example.org"),
		)
		.await;
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id, rank)
			 VALUES (gen_random_uuid(), 'kamaka-waiting', '{group}', 'production');
			 INSERT INTO machines (id, name, group_id, rank, deleted_at)
			 VALUES (gen_random_uuid(), 'kamaka-gone', '{group}', 'production', NOW())"
		))
		.await
		.expect("seed machines");

		let body = read_served(&private, json!({ "group": "kamaka" })).await;
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 1, "{body}");
		assert_eq!(hosts[0]["name"], "kamaka-central");
	})
	.await
}

/// A rank no machine holds is no environment, so it is refused rather than
/// served empty.
// spec: GRP#environments
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_rank_no_machine_holds() {
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

		let refused = private
			.post("/api/ansible/take_lease")
			.json(&json!({ "group": "kamaka", "rank": "demo" }))
			.await;
		refused.assert_status(commons_tests::axum_test::http::StatusCode::CONFLICT);
		assert!(refused.text().contains("no machine at rank demo"));
	})
	.await
}
