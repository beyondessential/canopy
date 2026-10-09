//! Device-facing name and certificate endpoints (CRT). The point of these is the
//! authorisation chain: every refusal has to be distinguishable by problem type,
//! so an agent can tell being unentitled from being paused from lacking the
//! grant without reading English.

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use diesel::{sql_query, sql_types};
use diesel_async::RunQueryDsl;
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use uuid::Uuid;

#[derive(diesel::QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

async fn count_certificates(conn: &mut AsyncPgConnection) -> i64 {
	sql_query("SELECT count(*) AS count FROM application_certificates")
		.get_result::<Count>(conn)
		.await
		.expect("count")
		.count
}

/// Each nextest test runs in its own process, so the zone configuration set here
/// is isolated. It has to be set before the server is built.
fn configure_zones(spec: &str) {
	unsafe { std::env::set_var("CANOPY_DNS_ZONES", spec) };
}

/// A group holding `domain`, and a server in it with both grants, bound to the
/// authenticated device.
async fn entitled(
	conn: &mut AsyncPgConnection,
	device_id: Uuid,
	domain: Option<&str>,
	dns: bool,
	tls: bool,
) -> Uuid {
	let group = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group}', 'crt-{group}')"
	))
	.await
	.expect("insert group");

	if let Some(domain) = domain {
		conn.batch_execute(&format!(
			"INSERT INTO server_group_domains (group_id, domain) VALUES ('{group}', '{domain}')"
		))
		.await
		.expect("claim domain");
	}

	let server = Uuid::new_v4();
	conn.batch_execute(&format!(
		"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{server}', '{group}', '{device_id}') RETURNING id) INSERT INTO applications (id, name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
		 VALUES ('{server}', 'crt', 'https://{server}.example.invalid', 'tamanu-central', '{group}', {dns}, {tls}, '{server}')"
	))
	.await
	.expect("insert server");
	server
}

/// A base64 CSR for `names`, the first also the subject common name.
fn csr_for(names: &[&str]) -> String {
	use base64::Engine;
	let key = KeyPair::generate().expect("keygen");
	let mut params =
		CertificateParams::new(names.iter().map(|n| n.to_string()).collect::<Vec<_>>())
			.expect("params");
	let mut dn = DistinguishedName::new();
	dn.push(DnType::CommonName, names[0]);
	params.distinguished_name = dn;
	let csr = params.serialize_request(&key).expect("serialize");
	base64::engine::general_purpose::STANDARD.encode(csr.der())
}

/// The problem type of a response body, which is what an agent matches on.
/// Canopy renders types as `/errors/<slug>`; the slug is the stable part.
fn problem_type(body: &serde_json::Value) -> String {
	body["type"]
		.as_str()
		.unwrap_or_default()
		.trim_start_matches("/errors/")
		.to_string()
}

// ── Entitlements ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn entitlements_report_what_the_server_may_act_on() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			let resp = public
				.get("/names/entitlements")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let body: serde_json::Value = resp.json();
			assert_eq!(body["may_manage_dns"], true);
			assert_eq!(body["may_manage_tls"], true);
			assert_eq!(body["paused"], false);
			assert_eq!(body["domains"][0], "fiji.tamanu.app");
			assert!(body["certificates"].as_array().unwrap().is_empty());
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn entitlements_are_empty_rather_than_an_error_when_there_is_nothing() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			// No domain, no grants: asking what one may do is not privileged.
			entitled(&mut conn, device_id, None, false, false).await;

			let resp = public
				.get("/names/entitlements")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let body: serde_json::Value = resp.json();
			assert_eq!(body["may_manage_dns"], false);
			assert!(body["domains"].as_array().unwrap().is_empty());
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_domain_whose_zone_has_gone_is_not_offered() {
	// The claim stands, but Canopy cannot act in that zone, so offering the
	// domain would have an agent request a name that can never be fulfilled.
	configure_zones("senaite.app=Z9");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			let resp = public
				.get("/names/entitlements")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			resp.assert_status_ok();
			let body: serde_json::Value = resp.json();
			assert!(
				body["domains"].as_array().unwrap().is_empty(),
				"a domain Canopy cannot act in is not offered: {body}"
			);
		},
	)
	.await;
}

// ── Addresses ───────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn registering_addresses_records_the_intent() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			let resp = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "Central.Fiji.Tamanu.App.",
					"addresses": ["192.0.2.1", "2001:db8::1"],
				}))
				.await;
			resp.assert_status_ok();
			let body: serde_json::Value = resp.json();
			assert_eq!(body["name"], "central.fiji.tamanu.app");
			assert_eq!(
				body["published"], false,
				"publishing happens in the background"
			);
			assert_eq!(body["addresses"].as_array().unwrap().len(), 2);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn each_refusal_is_distinguishable_by_problem_type() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let server =
				entitled(&mut conn, device_id, Some("fiji.tamanu.app"), false, false).await;

			// No grant.
			let resp = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "a.fiji.tamanu.app", "addresses": []}))
				.await;
			assert_eq!(problem_type(&resp.json()), "auth-insufficient-permissions");

			conn.batch_execute(&format!(
				"UPDATE applications SET may_manage_dns = true, may_manage_tls = true WHERE id = '{server}'"
			))
			.await
			.unwrap();

			// A name outside the group's domains — reported the same whether it is
			// unclaimed or someone else's, so this is not a directory.
			let resp = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "a.samoa.tamanu.app", "addresses": []}))
				.await;
			assert_eq!(problem_type(&resp.json()), "name-not-entitled");

			// Paused.
			conn.batch_execute(&format!(
				"UPDATE applications SET name_management_paused_at = now(), \
				 name_management_pause_reason = 'looking into it' WHERE id = '{server}'"
			))
			.await
			.unwrap();
			let resp = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "a.fiji.tamanu.app", "addresses": []}))
				.await;
			let body: serde_json::Value = resp.json();
			assert_eq!(problem_type(&body), "name-management-paused");
			assert!(
				body["detail"]
					.as_str()
					.unwrap_or_default()
					.contains("looking into it"),
				"the reason belongs in the refusal: {body}"
			);
		},
	)
	.await;
}

// ── Certificates ────────────────────────────────────────────────────────────

#[tokio::test(flavor = "multi_thread")]
async fn a_first_request_is_accepted_and_a_repeat_is_the_same_order() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let csr = csr_for(&["central.fiji.tamanu.app"]);

			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "central.fiji.tamanu.app", "csr": csr}))
				.await;
			// 202: recorded, nothing to collect yet — proving control through DNS
			// takes far longer than a handshake.
			resp.assert_status(axum::http::StatusCode::ACCEPTED);
			let body: serde_json::Value = resp.json();
			assert_eq!(body["state"], "pending");
			assert!(body["chain"].is_null());
			assert_eq!(body["usable"], false);

			// Repeating is safe and does not open a second order.
			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "central.fiji.tamanu.app", "csr": csr}))
				.await;
			resp.assert_status(axum::http::StatusCode::ACCEPTED);

			assert_eq!(
				count_certificates(&mut conn).await,
				1,
				"a repeat request is the same order"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_csr_carrying_another_name_is_refused_rather_than_trimmed() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			// The group controls fiji, so the *name* is entitled — but the CSR
			// smuggles a second name past that check, and must be refused.
			let csr = csr_for(&["central.fiji.tamanu.app", "central.samoa.tamanu.app"]);
			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "central.fiji.tamanu.app", "csr": csr}))
				.await;
			resp.assert_status_bad_request();

			assert_eq!(
				count_certificates(&mut conn).await,
				0,
				"nothing recorded for a refused request"
			);
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_certificate_becomes_collectable_once_issued() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let csr = csr_for(&["central.fiji.tamanu.app"]);
			let args = serde_json::json!({"name": "central.fiji.tamanu.app", "csr": csr});

			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&args)
				.await
				.assert_status(axum::http::StatusCode::ACCEPTED);

			// The worker would do this.
			conn.batch_execute(
				"UPDATE application_certificates SET state = 'issued', chain = '-----BEGIN CERT-----', \
				 issued_at = now(), not_after = now() + interval '80 days', profile = 'classic'",
			)
			.await
			.unwrap();

			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&args)
				.await;
			resp.assert_status_ok();
			let body: serde_json::Value = resp.json();
			assert_eq!(body["state"], "issued");
			assert_eq!(body["chain"], "-----BEGIN CERT-----");
			assert_eq!(body["usable"], true);
			assert_eq!(body["profile"], "classic");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revoked_compromised_key_is_refused_actionably() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let csr = csr_for(&["central.fiji.tamanu.app"]);
			let args = serde_json::json!({"name": "central.fiji.tamanu.app", "csr": csr});

			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&args)
				.await
				.assert_status(axum::http::StatusCode::ACCEPTED);

			// Bar the key the way a key-compromise revocation does.
			conn.batch_execute(
				"INSERT INTO compromised_keys (key_fingerprint) \
				 SELECT key_fingerprint FROM application_certificates",
			)
			.await
			.unwrap();

			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&args)
				.await;
			// The whole point: matchable on the type, so bestool rotates the key
			// itself rather than waiting for an operator.
			assert_eq!(problem_type(&resp.json()), "certificate-key-compromised");
		},
	)
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_status_push_carries_the_same_entitlements() {
	configure_zones("tamanu.app=Z1");

	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let server = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			let resp = public
				.post(&format!("/status/{server}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"source": "tamanu", "healthy": true}))
				.await;
			resp.assert_status_ok();
			let body: serde_json::Value = resp.json();
			assert_eq!(
				body["names"]["domains"][0], "fiji.tamanu.app",
				"an agent already reporting status learns of its domains: {body}"
			);
			assert_eq!(body["names"]["may_manage_tls"], true);
		},
	)
	.await;
}

/// Which application a request concerns is resolved from the name it asks
/// about, not from the credential it presents. A box hosting two workloads
/// resolves each name to the one that declares it.
// spec: DNS#identity-and-authorisation
#[tokio::test(flavor = "multi_thread")]
async fn a_name_resolves_to_the_application_declaring_it() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			// A second workload on the same box, in the same group, with the
			// same grants.
			let second = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO applications \
				   (id, name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
				 SELECT '{second}', 'crt-2', 'https://{second}.example.invalid', 'tamanu-central', \
				        group_id, true, true, machine_id \
				 FROM applications WHERE id = '{first}'"
			))
			.await
			.expect("second application on the same box");

			// The second declares the name; the first does not.
			conn.batch_execute(&format!(
				"INSERT INTO application_names (application_id, name, addresses, \
				   published_addresses) \
				 VALUES ('{second}', 'two.fiji.tamanu.app', '{{}}', '{{}}')"
			))
			.await
			.expect("declare the name");

			public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "two.fiji.tamanu.app",
					"addresses": ["192.0.2.7"],
				}))
				.await
				.assert_status_ok();

			#[derive(diesel::QueryableByName)]
			struct Owner {
				#[diesel(sql_type = sql_types::Uuid)]
				application_id: Uuid,
			}
			let owner: Owner =
				sql_query("SELECT application_id FROM application_names WHERE name = $1")
					.bind::<sql_types::Text, _>("two.fiji.tamanu.app")
					.get_result(&mut conn)
					.await
					.expect("the name");
			assert_eq!(
				owner.application_id, second,
				"the registration landed on the application declaring the name, \
				 not on whichever workload the credential happened to reach first"
			);

			// A certificate request routes the same way, off the same credential.
			let csr = csr_for(&["two.fiji.tamanu.app"]);
			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "two.fiji.tamanu.app", "csr": csr}))
				.await
				.assert_status(axum::http::StatusCode::ACCEPTED);

			let cert_owner: Owner =
				sql_query("SELECT application_id FROM application_certificates WHERE name = $1")
					.bind::<sql_types::Text, _>("two.fiji.tamanu.app")
					.get_result(&mut conn)
					.await
					.expect("the order");
			assert_eq!(
				cert_owner.application_id, second,
				"the order was placed for the application declaring the name"
			);
		},
	)
	.await
}

/// A machine asking about a name none of its applications declares is refused
/// the same way whether another application holds it or nobody does — so the
/// endpoint is not a directory of what other machines serve.
// spec: DNS#identity-and-authorisation
#[tokio::test(flavor = "multi_thread")]
async fn a_name_held_elsewhere_is_refused_exactly_as_an_unheld_one() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let mine = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			// A second workload on my box, so an undeclared name is genuinely
			// ambiguous and the resolution has to go by declaration.
			let sibling = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO applications \
				   (id, name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
				 SELECT '{sibling}', 'crt-2', 'https://{sibling}.example.invalid', 'tamanu-central', \
				        group_id, true, true, machine_id \
				 FROM applications WHERE id = '{mine}'"
			))
			.await
			.expect("sibling workload");

			// Someone else's box declares a name under the same domain.
			let elsewhere = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO machines (name, id) VALUES ('box', '{elsewhere}'); \
				 INSERT INTO applications (id, name, host, type, machine_id) \
				 VALUES ('{elsewhere}', 'theirs', 'https://{elsewhere}.example.invalid', 'tamanu-central', '{elsewhere}'); \
				 INSERT INTO application_names (application_id, name, addresses, \
				   published_addresses) \
				 VALUES ('{elsewhere}', 'theirs.fiji.tamanu.app', '{{}}', '{{}}')"
			))
			.await
			.expect("another machine's name");

			let held_elsewhere = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "theirs.fiji.tamanu.app",
					"addresses": ["192.0.2.7"],
				}))
				.await;
			let held_by_nobody = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "nobody.fiji.tamanu.app",
					"addresses": ["192.0.2.7"],
				}))
				.await;

			assert_eq!(
				held_elsewhere.status_code(),
				held_by_nobody.status_code(),
				"a name held elsewhere and a name held by nobody refuse alike"
			);
			// Identical once the name the caller itself asked about is taken
			// out — echoing that back tells it nothing it did not send. What
			// must not differ is anything that would reveal the name is held
			// somewhere else.
			let normalise = |resp: serde_json::Value, name: &str| {
				serde_json::to_string(&resp)
					.expect("json")
					.replace(name, "<the requested name>")
			};
			assert_eq!(
				normalise(held_elsewhere.json(), "theirs.fiji.tamanu.app"),
				normalise(held_by_nobody.json(), "nobody.fiji.tamanu.app"),
				"the refusals differ only in the name asked about, so neither is \
				 a directory of what other machines serve"
			);
		},
	)
	.await
}

/// An agent holds a machine identity, so it asks on behalf of the box and gets
/// an answer for every workload on it.
// spec: DNS#what-an-application-may-act-on
#[tokio::test(flavor = "multi_thread")]
async fn entitlements_carry_one_entry_per_application_on_the_machine() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			let ask = async |public: &axum_test::TestServer| -> serde_json::Value {
				let resp = public
					.get("/names/entitlements")
					.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
					.await;
				resp.assert_status_ok();
				resp.json()
			};

			// One workload on the box: the flat fields still answer, so an
			// agent that has not learned the plural shape keeps working.
			let one = ask(&public).await;
			assert_eq!(one["applications"].as_array().unwrap().len(), 1);
			assert_eq!(one["applications"][0]["type"], "tamanu-central");
			assert_eq!(one["may_manage_dns"], true);
			assert_eq!(one["domains"][0], "fiji.tamanu.app");

			// A second workload, without the grants, on the same box. The
			// entry names its type, which is what a reporter correlates on:
			// Canopy's own id for a workload never goes on the wire.
			let second = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO applications \
				   (id, name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
				 SELECT '{second}', 'ent-2', 'https://{second}.example.invalid', 'tamanu-facility', \
				        group_id, false, false, machine_id \
				 FROM applications WHERE id = '{first}'"
			))
			.await
			.expect("second application on the same box");

			let two = ask(&public).await;
			let entries = two["applications"].as_array().unwrap();
			assert_eq!(entries.len(), 2, "one entry per workload on the box");
			let grants: std::collections::BTreeMap<&str, bool> = entries
				.iter()
				.map(|e| {
					(
						e["type"].as_str().unwrap(),
						e["may_manage_dns"].as_bool().unwrap(),
					)
				})
				.collect();
			assert!(grants["tamanu-central"]);
			assert!(!grants["tamanu-facility"]);

			assert_eq!(
				two["may_manage_dns"], false,
				"no single workload's grants are the box's, so the flat fields \
				 stop answering and the list is the answer"
			);
			assert!(two["domains"].as_array().unwrap().is_empty());
		},
	)
	.await
}

/// The push response is not a summary of the entitlements answer, it is the
/// answer: an agent that only ever pushes status learns of a new domain or a
/// withdrawn grant without asking a second question.
// spec: DNS#what-an-application-may-act-on
// spec: STA
#[tokio::test(flavor = "multi_thread")]
async fn the_push_response_carries_the_whole_entitlements_answer() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			// A second workload on the box, so the answer is one the flat
			// fields cannot carry on their own.
			let second = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO applications \
				   (id, name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
				 SELECT '{second}', 'push-2', 'https://{second}.example.invalid', \
				        'tamanu-facility', group_id, false, true, machine_id \
				 FROM applications WHERE id = '{first}'"
			))
			.await
			.expect("second application on the same box");

			let standalone = public
				.get("/names/entitlements")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.await;
			standalone.assert_status_ok();
			let standalone: serde_json::Value = standalone.json();

			let pushed = public
				.post(&format!("/status/{first}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				// Two workloads on the box, so the push has to say which of them
				// it speaks for.
				.json(&serde_json::json!({
					"source": "alertd",
					"health": [],
					"tamanuServerKind": "central",
				}))
				.await;
			pushed.assert_status_ok();
			let pushed: serde_json::Value = pushed.json();

			assert_eq!(
				pushed["names"], standalone,
				"the two routes answer the same question, so they answer it the \
				 same way"
			);
			assert_eq!(
				pushed["names"]["applications"].as_array().unwrap().len(),
				2,
				"both workloads on the box are in the answer"
			);
		},
	)
	.await
}

// ── Resolving the application ───────────────────────────────────────────────

/// A second workload on the same box as `first`, in the same group.
async fn sibling(conn: &mut AsyncPgConnection, first: Uuid, r#type: &str, tls: bool) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO applications \
		   (id, name, host, type, group_id, may_manage_dns, may_manage_tls, machine_id) \
		 SELECT '{id}', 'crt-sibling', 'https://{id}.example.invalid', '{type}', \
		        group_id, true, {tls}, machine_id \
		 FROM applications WHERE id = '{first}'"
	))
	.await
	.expect("sibling workload");
	id
}

#[derive(diesel::QueryableByName)]
struct Undeclared {
	#[diesel(sql_type = sql_types::Text)]
	dns_name: String,
	#[diesel(sql_type = sql_types::Text)]
	asked_for: String,
}

async fn undeclared(conn: &mut AsyncPgConnection) -> Vec<(String, String)> {
	sql_query("SELECT dns_name, asked_for FROM undeclared_dns_names ORDER BY dns_name")
		.load::<Undeclared>(conn)
		.await
		.expect("undeclared")
		.into_iter()
		.map(|r| (r.dns_name, r.asked_for))
		.collect()
}

async fn declared_by(conn: &mut AsyncPgConnection, name: &str) -> Option<Uuid> {
	#[derive(diesel::QueryableByName)]
	struct Owner {
		#[diesel(sql_type = sql_types::Uuid)]
		application_id: Uuid,
	}
	sql_query("SELECT application_id FROM application_names WHERE name = $1")
		.bind::<sql_types::Text, _>(name)
		.get_result::<Owner>(conn)
		.await
		.ok()
		.map(|o| o.application_id)
}

/// Two workloads with the same grants under the same domains cannot be told
/// apart, so the request is refused as undeclared and recorded for an operator.
// spec: DNS#resolving-the-application
// spec: DNS#undeclared-requests
#[tokio::test(flavor = "multi_thread")]
async fn an_ambiguous_request_is_refused_as_undeclared_and_recorded() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			sibling(&mut conn, first, "senaite", true).await;

			let csr = csr_for(&["lab.fiji.tamanu.app"]);
			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "lab.fiji.tamanu.app", "csr": csr}))
				.await;
			resp.assert_status(axum::http::StatusCode::FORBIDDEN);
			assert_eq!(problem_type(&resp.json()), "dns-name-undeclared");
			assert_eq!(count_certificates(&mut conn).await, 0);
			assert_eq!(
				undeclared(&mut conn).await,
				vec![("lab.fiji.tamanu.app".to_string(), "certificate".to_string())]
			);

			// Asking again updates the one record rather than adding another, and
			// says what the latest ask was for.
			public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(
					&serde_json::json!({"name": "lab.fiji.tamanu.app", "addresses": ["192.0.2.7"]}),
				)
				.await
				.assert_status(axum::http::StatusCode::FORBIDDEN);
			assert_eq!(
				undeclared(&mut conn).await,
				vec![("lab.fiji.tamanu.app".to_string(), "addresses".to_string())]
			);
		},
	)
	.await
}

/// Naming the application type tells two workloads apart, and the request
/// declares the name for the one it named, ending the undeclared record.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn a_named_type_resolves_and_declares() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let lab = sibling(&mut conn, first, "senaite", true).await;

			let csr = csr_for(&["lab.fiji.tamanu.app"]);
			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "lab.fiji.tamanu.app", "csr": csr}))
				.await
				.assert_status(axum::http::StatusCode::FORBIDDEN);
			assert_eq!(undeclared(&mut conn).await.len(), 1);

			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "lab.fiji.tamanu.app",
					"csr": csr,
					"application_type": "senaite",
				}))
				.await
				.assert_status(axum::http::StatusCode::ACCEPTED);

			assert_eq!(
				declared_by(&mut conn, "lab.fiji.tamanu.app").await,
				Some(lab)
			);
			assert!(
				undeclared(&mut conn).await.is_empty(),
				"an answered request is no longer waiting on anyone"
			);

			// Once declared, the type is unneeded.
			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "lab.fiji.tamanu.app", "csr": csr}))
				.await
				.assert_status(axum::http::StatusCode::ACCEPTED);
		},
	)
	.await
}

/// Where only one workload on the box holds the grant the request needs and
/// covers the name, that is the one, with no type and no operator.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn the_grants_resolve_when_only_one_application_could_act() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			sibling(&mut conn, first, "senaite", false).await;

			let csr = csr_for(&["central.fiji.tamanu.app"]);
			public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "central.fiji.tamanu.app", "csr": csr}))
				.await
				.assert_status(axum::http::StatusCode::ACCEPTED);
			assert_eq!(
				declared_by(&mut conn, "central.fiji.tamanu.app").await,
				Some(first)
			);
		},
	)
	.await
}

/// A type contradicting the declaration on the same machine is refused, naming
/// the declaring application's type, rather than quietly following either.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn a_type_contradicting_the_declaration_is_refused_naming_it() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let lab = sibling(&mut conn, first, "senaite", true).await;
			conn.batch_execute(&format!(
				"INSERT INTO application_names (application_id, name) \
				 VALUES ('{lab}', 'lab.fiji.tamanu.app')"
			))
			.await
			.expect("declare");

			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "lab.fiji.tamanu.app",
					"csr": csr_for(&["lab.fiji.tamanu.app"]),
					"application_type": "tamanu-central",
				}))
				.await;
			resp.assert_status(axum::http::StatusCode::CONFLICT);
			let body: serde_json::Value = resp.json();
			assert_eq!(problem_type(&body), "dns-name-type-mismatch");
			assert!(
				body["title"]
					.as_str()
					.unwrap_or_default()
					.contains("senaite"),
				"the refusal names the declaring type: {body}"
			);
		},
	)
	.await
}

/// A single-application machine asking for a name another machine's
/// application holds is refused as undeclared, as on any machine.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn a_name_held_elsewhere_is_undeclared_on_a_single_application_machine() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let elsewhere = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO machines (id, name) VALUES ('{elsewhere}', 'elsewhere'); \
				 INSERT INTO applications (id, name, host, type, machine_id) \
				 VALUES ('{elsewhere}', 'theirs', 'https://{elsewhere}.example.invalid', 'tamanu-central', '{elsewhere}'); \
				 INSERT INTO application_names (application_id, name) \
				 VALUES ('{elsewhere}', 'theirs.fiji.tamanu.app')"
			))
			.await
			.expect("another machine's name");

			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "theirs.fiji.tamanu.app",
					"csr": csr_for(&["theirs.fiji.tamanu.app"]),
				}))
				.await;
			assert_eq!(problem_type(&resp.json()), "dns-name-undeclared");
			assert_eq!(undeclared(&mut conn).await.len(), 1);
		},
	)
	.await
}

/// On a single-application machine, a name another machine's application
/// holds meets the same checks as one nobody holds, so outside the group's
/// domains both are refused as unentitled and nothing tells them apart.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn a_name_held_elsewhere_outside_the_domains_is_unentitled_as_an_unheld_one() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let elsewhere = Uuid::new_v4();
			conn.batch_execute(&format!(
				"INSERT INTO machines (id, name) VALUES ('{elsewhere}', 'elsewhere'); \
				 INSERT INTO applications (id, name, host, type, machine_id) \
				 VALUES ('{elsewhere}', 'theirs', 'https://{elsewhere}.example.invalid', 'tamanu-central', '{elsewhere}'); \
				 INSERT INTO application_names (application_id, name) \
				 VALUES ('{elsewhere}', 'theirs.samoa.tamanu.app')"
			))
			.await
			.expect("another machine's name");

			for name in ["theirs.samoa.tamanu.app", "nobody.samoa.tamanu.app"] {
				let resp = public
					.post("/certificates/request")
					.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
					.json(&serde_json::json!({
						"name": name,
						"csr": csr_for(&[name]),
					}))
					.await;
				assert_eq!(problem_type(&resp.json()), "name-not-entitled", "{name}");
			}
			assert!(undeclared(&mut conn).await.is_empty());
			assert_eq!(count_certificates(&mut conn).await, 0);
		},
	)
	.await
}

/// A single-application machine naming a type other than its one
/// application's is refused, naming the type it hosts, and declares nothing.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn a_type_the_machine_does_not_host_is_refused_naming_what_it_does() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;

			let resp = public
				.post("/certificates/request")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "msupply.fiji.tamanu.app",
					"csr": csr_for(&["msupply.fiji.tamanu.app"]),
					"application_type": "msupply",
				}))
				.await;
			resp.assert_status(axum::http::StatusCode::CONFLICT);
			let body: serde_json::Value = resp.json();
			assert_eq!(problem_type(&body), "dns-name-type-mismatch");
			assert!(
				body["title"]
					.as_str()
					.unwrap_or_default()
					.contains("tamanu-central"),
				"the refusal names the type the machine hosts: {body}"
			);
			assert_eq!(
				declared_by(&mut conn, "msupply.fiji.tamanu.app").await,
				None
			);
			assert_eq!(count_certificates(&mut conn).await, 0);
			assert!(undeclared(&mut conn).await.is_empty());
		},
	)
	.await
}

/// A denied name is refused as denied, whatever would otherwise resolve it,
/// and is not recorded, so asking again raises nothing.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn a_denied_name_is_refused_as_denied_and_not_recorded() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let server = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			conn.batch_execute(&format!(
				"INSERT INTO denied_dns_names (machine_id, dns_name, denied_by, note) \
				 SELECT machine_id, 'old.fiji.tamanu.app', 'admin@localhost', 'site retired' \
				 FROM applications WHERE id = '{server}'"
			))
			.await
			.expect("deny");

			for _ in 0..2 {
				let resp = public
					.post("/certificates/request")
					.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
					.json(&serde_json::json!({
						"name": "old.fiji.tamanu.app",
						"csr": csr_for(&["old.fiji.tamanu.app"]),
					}))
					.await;
				resp.assert_status(axum::http::StatusCode::FORBIDDEN);
				let body: serde_json::Value = resp.json();
				assert_eq!(problem_type(&body), "dns-name-denied");
				assert!(
					!body.to_string().contains("site retired"),
					"the operator's note stays in Canopy: {body}"
				);
			}
			assert_eq!(count_certificates(&mut conn).await, 0);
			assert!(undeclared(&mut conn).await.is_empty());
		},
	)
	.await
}

/// An address registration resolves by a named type as a certificate request
/// does, and declares the name for the application it named.
// spec: DNS#resolving-the-application
#[tokio::test(flavor = "multi_thread")]
async fn an_address_registration_resolves_by_a_named_type() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let first = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			let lab = sibling(&mut conn, first, "senaite", true).await;

			public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({
					"name": "lab.fiji.tamanu.app",
					"addresses": ["192.0.2.7"],
					"application_type": "senaite",
				}))
				.await
				.assert_status_ok();
			assert_eq!(
				declared_by(&mut conn, "lab.fiji.tamanu.app").await,
				Some(lab)
			);
		},
	)
	.await
}

/// A denial records a decision rather than an observation, so it stands long
/// after the machine stopped asking.
// spec: DNS#denied-dns-names
#[tokio::test(flavor = "multi_thread")]
async fn a_denial_outlasts_a_day_without_asking() {
	configure_zones("tamanu.app=Z1");
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _private| {
			let server = entitled(&mut conn, device_id, Some("fiji.tamanu.app"), true, true).await;
			conn.batch_execute(&format!(
				"INSERT INTO denied_dns_names (machine_id, dns_name, denied_by, created_at) \
				 SELECT machine_id, 'old.fiji.tamanu.app', 'admin@localhost', now() - interval '30 days' \
				 FROM applications WHERE id = '{server}'"
			))
			.await
			.expect("deny long ago");

			let resp = public
				.post("/names/register")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.json(&serde_json::json!({"name": "old.fiji.tamanu.app", "addresses": []}))
				.await;
			assert_eq!(problem_type(&resp.json()), "dns-name-denied");
		},
	)
	.await
}
