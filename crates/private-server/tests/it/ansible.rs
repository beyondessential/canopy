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

/// What the scenarios of a folded test found wrong, each naming its scenario so
/// a failing one says so instead of stopping the rest.
#[derive(Default)]
pub(crate) struct Failures(Vec<String>);

impl Failures {
	pub(crate) fn check(&mut self, scenario: &str, ok: bool, what: impl std::fmt::Display) {
		if !ok {
			self.0.push(format!("{scenario}: {what}"));
		}
	}

	pub(crate) fn eq(
		&mut self,
		scenario: &str,
		what: &str,
		actual: &Value,
		expected: impl Into<Value>,
	) {
		let expected = expected.into();
		if *actual != expected {
			self.0
				.push(format!("{scenario}: {what} was {actual}, not {expected}"));
		}
	}

	pub(crate) fn ok<T>(&mut self, scenario: &str, result: Result<T, String>) -> Option<T> {
		match result {
			Ok(value) => Some(value),
			Err(error) => {
				self.0.push(format!("{scenario}: {error}"));
				None
			}
		}
	}

	pub(crate) fn assert_none(self) {
		assert!(self.0.is_empty(), "{}", self.0.join("\n"));
	}
}

/// `read_served` that reports a refusal rather than panicking.
async fn try_read_served(
	private: &commons_tests::axum_test::TestServer,
	args: Value,
) -> Result<Value, String> {
	let taken = private.post("/api/ansible/take_lease").json(&args).await;
	if !taken.status_code().is_success() {
		return Err(format!("take_lease answered {}", taken.text()));
	}
	let lease = taken.json::<Value>()["id"].clone();
	let read = private
		.post("/api/ansible/for_group")
		.json(&json!({ "lease_id": lease }))
		.await;
	if !read.status_code().is_success() {
		return Err(format!("for_group answered {}", read.text()));
	}
	Ok(read.json())
}

/// What an environment serves a run, each scenario in a group of its own and read
/// by that group's identifier: its machines and the applications on them, a
/// shared box whole, the rank asked for, and what is left out (an archived
/// application, Canopy's own, a machine nothing can reach) or served without an
/// address.
// spec: INV#what-a-run-is-served
#[tokio::test(flavor = "multi_thread")]
async fn serves_each_environment_what_a_run_configures() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let mut failures = Failures::default();

		let scenario = "an environment's machines and applications";
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
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(&private, json!({ "server_group_id": group })).await,
		) {
			failures.eq(scenario, "the group", &body["group"], "kamaka-prod");
			// Nothing here carries a rank, so the whole group is one environment at
			// the default rank.
			failures.eq(scenario, "the rank", &body["rank"], "dev");
			let hosts = &body["hosts"];
			failures.check(
				scenario,
				hosts.as_array().map(Vec::len) == Some(2),
				format!("two hosts: {body}"),
			);
			// A bound device's tailnet name is the address, and the machine carries
			// the applications a run configures on it.
			failures.eq(
				scenario,
				"the first host",
				&hosts[0]["name"],
				"kamaka-prod-central",
			);
			failures.eq(
				scenario,
				"the first address",
				&hosts[0]["address"],
				"kamaka-prod-central",
			);
			failures.eq(
				scenario,
				"the first application's type",
				&hosts[0]["applications"][0]["type"],
				"tamanu-central",
			);
			failures.eq(
				scenario,
				"the first application's id",
				&hosts[0]["applications"][0]["id"],
				central.to_string(),
			);
			// No device on the box, so the address is the recorded host of an
			// application on it, as a bare name.
			failures.eq(
				scenario,
				"the second address",
				&hosts[1]["address"],
				"facility.kamaka.example",
			);
		}

		let scenario = "a shared box";
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
		// A box serves one environment, so both workloads on it sit in it and the
		// box is one host carrying both.
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(
				&private,
				json!({ "server_group_id": group, "rank": "production" }),
			)
			.await,
		) {
			let hosts = &body["hosts"];
			failures.check(
				scenario,
				hosts.as_array().map(Vec::len) == Some(1),
				format!("one host: {body}"),
			);
			failures.eq(scenario, "the host", &hosts[0]["id"], machine.to_string());
			failures.eq(
				scenario,
				"the address",
				&hosts[0]["address"],
				"kamaka-shared-box",
			);
			failures.check(
				scenario,
				hosts[0]["applications"].as_array().map(Vec::len) == Some(2),
				format!("the whole box is in the environment: {body}"),
			);
		}

		let scenario = "the rank asked for";
		let group = insert_group(&mut conn, "kamaka-ranks").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-ranks-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-ranks-demo",
			"tamanu-central",
			Some("demo"),
			None,
		)
		.await;
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(
				&private,
				json!({ "server_group_id": group, "rank": "demo" }),
			)
			.await,
		) {
			failures.eq(scenario, "the rank", &body["rank"], "demo");
			failures.check(
				scenario,
				body["hosts"].as_array().map(Vec::len) == Some(1),
				format!("one host: {body}"),
			);
			failures.eq(
				scenario,
				"the host",
				&body["hosts"][0]["name"],
				"kamaka-ranks-demo",
			);
		}

		let scenario = "an archived application";
		let group = insert_group(&mut conn, "kamaka-archived").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-archived-central",
			"tamanu-central",
			None,
		)
		.await;
		let gone = insert_application(
			&mut conn,
			group,
			"kamaka-archived-old",
			"tamanu-facility",
			None,
		)
		.await;
		conn.batch_execute(&format!(
			"UPDATE applications SET deleted_at = NOW() WHERE id = '{gone}'"
		))
		.await
		.expect("archive");
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(&private, json!({ "server_group_id": group })).await,
		) {
			failures.check(
				scenario,
				body["hosts"].as_array().map(Vec::len) == Some(1),
				format!("one host: {body}"),
			);
			failures.eq(
				scenario,
				"the host",
				&body["hosts"][0]["name"],
				"kamaka-archived-central",
			);
		}

		let scenario = "Canopy's own application";
		let group = insert_group(&mut conn, "kamaka-meta").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-meta-central",
			"tamanu-central",
			None,
		)
		.await;
		conn.batch_execute(&format!(
			"UPDATE machines SET group_id = '{group}'
			 WHERE id = '00000000-0000-0000-0000-000000000000';
			 UPDATE applications SET group_id = '{group}'
			 WHERE id = '00000000-0000-0000-0000-000000000000'"
		))
		.await
		.expect("put the meta application in the group");
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(&private, json!({ "server_group_id": group })).await,
		) {
			failures.check(
				scenario,
				body["hosts"].as_array().map(Vec::len) == Some(1),
				format!("one host: {body}"),
			);
			failures.eq(
				scenario,
				"the host",
				&body["hosts"][0]["name"],
				"kamaka-meta-central",
			);
		}

		let scenario = "no address held";
		let group = insert_group(&mut conn, "kamaka-nowhere").await;
		insert_application(
			&mut conn,
			group,
			"kamaka-nowhere-central",
			"tamanu-central",
			None,
		)
		.await;
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(&private, json!({ "server_group_id": group })).await,
		) {
			failures.eq(
				scenario,
				"the address",
				&body["hosts"][0]["address"],
				Value::Null,
			);
		}

		let scenario = "a machine with a bound device and nothing on it";
		let group = insert_group(&mut conn, "kamaka-bare").await;
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
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(&private, json!({ "server_group_id": group })).await,
		) {
			failures.check(
				scenario,
				body["hosts"].as_array().map(Vec::len) == Some(1),
				format!("one host: {body}"),
			);
			failures.eq(
				scenario,
				"the host",
				&body["hosts"][0]["id"],
				machine.to_string(),
			);
			failures.eq(
				scenario,
				"its applications",
				&body["hosts"][0]["applications"],
				json!([]),
			);
			failures.eq(
				scenario,
				"its host name",
				&body["hosts"][0]["host_name"],
				"kamaka-new",
			);
		}

		let scenario = "a machine a run cannot reach";
		let group = insert_group(&mut conn, "kamaka-unreachable").await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-unreachable-central",
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
		if let Some(body) = failures.ok(
			scenario,
			try_read_served(&private, json!({ "server_group_id": group })).await,
		) {
			failures.check(
				scenario,
				body["hosts"].as_array().map(Vec::len) == Some(1),
				format!("one host: {body}"),
			);
			failures.eq(
				scenario,
				"the host",
				&body["hosts"][0]["name"],
				"kamaka-unreachable-central",
			);
		}

		failures.assert_none();
	})
	.await
}

/// A request that names no environment is refused with no lease taken, and each
/// refusal says what is wrong: nothing ranked yet (a group with no machine, or
/// with only a pending one), several environments with no rank named, a rank no
/// machine holds, a group Canopy does not have or has archived or that a name
/// does not settle, and no group asked for at all.
// spec: GRP#environments
#[tokio::test(flavor = "multi_thread")]
async fn refuses_a_request_that_names_no_environment() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let pending = insert_group(&mut conn, "kamaka-pending").await;
		let machine = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id) VALUES ('{machine}', 'kamaka-box', '{pending}');
			 INSERT INTO applications (id, name, type, group_id, machine_id)
			 VALUES (gen_random_uuid(), 'kamaka-pending-central', 'tamanu-central', '{pending}', '{machine}')"
		))
		.await
		.expect("seed a pending box");

		insert_group(&mut conn, "kamaka-empty").await;

		let several = insert_group(&mut conn, "kamaka-several").await;
		for (name, rank) in [("kamaka-several-central", "production"), ("kamaka-several-demo", "demo")] {
			insert_ranked_application(&mut conn, several, name, "tamanu-central", Some(rank), None)
				.await;
		}

		let ranked = insert_group(&mut conn, "kamaka-ranked").await;
		insert_ranked_application(
			&mut conn,
			ranked,
			"kamaka-ranked-central",
			"tamanu-central",
			Some("production"),
			None,
		)
		.await;

		let archived = insert_group(&mut conn, "kamaka-gone").await;
		insert_application(
			&mut conn,
			archived,
			"kamaka-gone-central",
			"tamanu-central",
			None,
		)
		.await;
		conn.batch_execute(&format!(
			"UPDATE server_groups SET deleted_at = NOW() WHERE id = '{archived}'"
		))
		.await
		.expect("archive");

		let twin = insert_group(&mut conn, "kamaka-twin").await;
		insert_group(&mut conn, "kamaka-twin").await;
		insert_application(&mut conn, twin, "kamaka-twin-central", "tamanu-central", None).await;

		use axum::http::StatusCode;
		let rows: Vec<(&str, Value, StatusCode, &str)> = vec![
			(
				"a group with only a pending box",
				json!({ "server_group_id": pending }),
				StatusCode::CONFLICT,
				"nothing ranked yet",
			),
			(
				"a group with nothing in it",
				json!({ "group": "kamaka-empty" }),
				StatusCode::CONFLICT,
				"nothing ranked yet",
			),
			(
				"several environments with no rank named",
				json!({ "group": "kamaka-several" }),
				StatusCode::CONFLICT,
				"name the rank",
			),
			(
				"a rank no machine holds",
				json!({ "group": "kamaka-ranked", "rank": "demo" }),
				StatusCode::CONFLICT,
				"no machine at rank demo",
			),
			(
				"a group Canopy does not have",
				json!({ "group": "nowhere" }),
				StatusCode::NOT_FOUND,
				"no server group named",
			),
			(
				"an archived group",
				json!({ "group": "kamaka-gone" }),
				StatusCode::CONFLICT,
				"archived",
			),
			(
				"a name that answers for two groups",
				json!({ "group": "kamaka-twin" }),
				StatusCode::CONFLICT,
				"ask by identifier",
			),
			(
				"neither a group nor an identifier",
				json!({}),
				StatusCode::BAD_REQUEST,
				"exactly one of",
			),
		];

		let mut failures = Failures::default();
		for (row, args, status, needle) in rows {
			let response = private.post("/api/ansible/take_lease").json(&args).await;
			failures.check(
				row,
				response.status_code() == status,
				format!("take_lease answered {}, not {status}", response.status_code()),
			);
			let text = response.text();
			failures.check(row, text.contains(needle), format!("refusal does not say {needle}: {text}"));
		}
		failures.assert_none();
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
				vec![Declared::new(Over::Machine(facility), OTHER, IN_TWO_HOURS)],
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

				let state = private
					.post("/api/ansible/run_state")
					.json(&environment)
					.await;
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
							failures
								.push(format!("{at}: release_lease answered {}", released.text()));
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
								failures.push(format!(
									"{at}: refusal does not name {needle}: {detail}"
								));
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
