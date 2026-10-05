//! Reported checks with instances, and a check's own `detail` object.
//!
//! spec: STA#health-and-detail, STA#instances, STA#response,
//! CHK#checks-with-instances, CHK#silencing-one-instance

use commons_types::{namespace::Namespace, server::app_type::ApplicationType};
use database::check_policies::CheckPolicy;
use database::silenced_refs::ServerSilencedRef;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use serde_json::{Value, json};
use uuid::Uuid;

const CHECK: &str = "sync_facility_stale";
const REF: &str = "health/sync_facility_stale";

/// A tamanu-central in a group, on its own machine, with the calling device
/// enrolled on that machine. The machine and the application share an id, so a
/// unified push to it is about that application.
async fn central(conn: &mut AsyncPgConnection, device_id: Uuid) -> Uuid {
	let group_id = Uuid::new_v4();
	sql_query("INSERT INTO server_groups (id, name) VALUES ($1, 'instanced-group')")
		.bind::<sql_types::Uuid, _>(group_id)
		.execute(conn)
		.await
		.expect("insert group");
	let id = Uuid::new_v4();
	sql_query(
		"WITH m AS (INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', $1, $3, $2) RETURNING id) \
		 INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ($1, 'https://central.example.com', 'tamanu-central', $3, $1)",
	)
	.bind::<sql_types::Uuid, _>(id)
	.bind::<sql_types::Uuid, _>(device_id)
	.bind::<sql_types::Uuid, _>(group_id)
	.execute(conn)
	.await
	.expect("insert central");
	id
}

/// Review the check's catalog entry at `ceiling`, with `rules` if given, in the
/// namespace a tamanu-central's push files it under. An unreviewed entry is
/// capped at warning, which would hide the difference a rule makes.
async fn set_policy(
	conn: &mut AsyncPgConnection,
	check: &str,
	ceiling: &str,
	rules: Option<Value>,
) {
	CheckPolicy::upsert_default(
		conn,
		"alertd",
		&Namespace::of("alertd", Some(&ApplicationType::TamanuCentral)),
		check,
	)
	.await
	.expect("ensure catalog row");
	sql_query(
		"UPDATE check_policies SET ceiling = $1, rules = $2::jsonb, reviewed_at = NOW(), \
		 reviewed_by = 'test' WHERE check_name = $3",
	)
	.bind::<sql_types::Text, _>(ceiling)
	.bind::<sql_types::Nullable<sql_types::Text>, _>(rules.map(|r| r.to_string()))
	.bind::<sql_types::Text, _>(check)
	.execute(conn)
	.await
	.expect("set policy");
}

#[derive(QueryableByName, Debug)]
struct State {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
	#[diesel(sql_type = sql_types::Bool)]
	active: bool,
	#[diesel(sql_type = sql_types::Text)]
	message: String,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	description: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	observed_result: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Text>)]
	effective_result: Option<String>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Jsonb>)]
	detail: Option<Value>,
	#[diesel(sql_type = sql_types::Nullable<sql_types::Jsonb>)]
	instances: Option<Value>,
}

impl State {
	fn effective(&self) -> &str {
		self.effective_result.as_deref().unwrap_or_default()
	}

	/// The stored instances' keys, sorted.
	fn keys(&self) -> Vec<&str> {
		let mut keys: Vec<&str> = self
			.instances
			.as_ref()
			.and_then(Value::as_object)
			.map(|i| i.keys().map(String::as_str).collect())
			.unwrap_or_default();
		keys.sort_unstable();
		keys
	}

	/// One stored instance's field.
	fn instance(&self, key: &str, field: &str) -> &Value {
		&self.instances.as_ref().expect("the state holds instances")[key][field]
	}
}

async fn state_where(conn: &mut AsyncPgConnection, column: &str, id: Uuid, r#ref: &str) -> State {
	sql_query(format!(
		"SELECT id, active, message, description, observed_result, effective_result, detail, \
		 instances FROM issues WHERE {column} = $1 AND source = 'alertd' AND ref = $2"
	))
	.bind::<sql_types::Uuid, _>(id)
	.bind::<sql_types::Text, _>(r#ref)
	.get_result(conn)
	.await
	.unwrap_or_else(|e| panic!("no state for {ref}: {e}"))
}

async fn state(conn: &mut AsyncPgConnection, application_id: Uuid, r#ref: &str) -> State {
	state_where(conn, "application_id", application_id, r#ref).await
}

async fn push(
	public: &axum_test::TestServer,
	cert: &str,
	conn: &mut AsyncPgConnection,
	id: Uuid,
	body: Value,
) -> Value {
	let response = public
		.post(&format!("/status/{id}"))
		.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
		.json(&body)
		.await;
	response.assert_status_ok();
	database::issues::process_incident_reeval_queue(conn, i64::MAX)
		.await
		.expect("drain incident reeval queue");
	response.json()
}

/// Central's facility staleness, one instance per device, with these results.
fn stale(instances: &[(&str, &str, &str)]) -> Value {
	json!({
		"health": [{
			"check": CHECK,
			"detail": { "fail_minutes": 30 },
			"instances": instances
				.iter()
				.map(|(key, result, label)| {
					let mut instance = json!({ "result": result, "detail": { "device": key } });
					if !label.is_empty() {
						instance["label"] = json!(label);
					}
					(key.to_string(), instance)
				})
				.collect::<serde_json::Map<_, _>>(),
		}],
	})
}

/// Every malformed check is refused with a 400 naming where it went wrong, and
/// records nothing.
// spec: STA#health-and-detail, STA#instances
#[tokio::test(flavor = "multi_thread")]
async fn malformed_checks_are_refused() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			let instance = json!({ "result": "passed" });
			// The bounds on how many instances a check may carry and how long
			// an instance's key or label may be: a reporter multiplying
			// instances buys work on every later push and silence of that
			// check, so the boundary is where that stops.
			let too_many: serde_json::Map<String, Value> = (0..=1000)
				.map(|i| (format!("k{i}"), instance.clone()))
				.collect();
			let long_name = "x".repeat(257);
			let refused = [
				(
					json!({ "check": "c", "result": "passed", "instances": { "a": instance } }),
					"exactly one of",
				),
				(
					json!({ "check": "c", "healthy": true, "instances": { "a": instance } }),
					"exactly one of",
				),
				(json!({ "check": "c" }), "must have a `result`"),
				(
					json!({ "check": "c", "detail": {} }),
					"must have a `result`",
				),
				(
					json!({ "check": "c", "result": "passed", "latency_ms": 4, "detail": { "x": 1 } }),
					"`latency_ms`",
				),
				(
					json!({ "check": "c", "latency_ms": 4, "instances": { "a": instance } }),
					"`latency_ms`",
				),
				(
					json!({ "check": "c", "instances": { "a": { "result": "passed", "detail": "x" } } }),
					"`health[0].instances.a.detail` must be an object",
				),
				(
					json!({ "check": "c", "instances": { "": instance } }),
					"keys must be non-empty",
				),
				(
					json!({ "check": "c", "instances": { "a": { "detail": {} } } }),
					"`health[0].instances.a` must have a `result`",
				),
				(
					json!({ "check": "c", "instances": { "a": { "healthy": true } } }),
					"legacy `healthy`",
				),
				(
					json!({ "check": "c", "instances": { "a": { "result": "passed", "healthy": true } } }),
					"legacy `healthy`",
				),
				(
					json!({ "check": "c", "instances": { "a": { "result": "broken" } } }),
					"must not be broken",
				),
				(
					json!({ "check": "c", "instances": { "a": { "result": "exploded" } } }),
					"must be one of passed, warning, failed, skipped",
				),
				(
					json!({ "check": "c", "instances": { "a": { "result": "passed", "label": 4 } } }),
					"`health[0].instances.a.label` must be a string",
				),
				(
					json!({ "check": "c", "instances": { "a": "passed" } }),
					"`health[0].instances.a` must be an object",
				),
				(
					json!({ "check": "c", "instances": [instance] }),
					"`health[0]` must have a `result`",
				),
				(
					json!({ "check": "c", "result": "passed", "instances": [1], "detail": { "x": 1 } }),
					"`instances`",
				),
				(
					json!({ "check": "c", "instances": too_many }),
					"more than the 1000 a check may carry",
				),
				(
					json!({ "check": "c", "instances": { long_name.clone(): instance } }),
					"keys must be at most 256 characters",
				),
				(
					json!({ "check": "c", "instances": { "a": { "result": "passed", "label": long_name } } }),
					"`health[0].instances.a.label` must be at most 256 characters",
				),
			];
			for (entry, expected) in refused {
				let response = public
					.post(&format!("/status/{id}"))
					.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
					.json(&json!({ "health": [entry] }))
					.await;
				response.assert_status_bad_request();
				let body: Value = response.json();
				let detail = body["detail"].as_str().unwrap_or_default();
				assert!(
					detail.contains(expected),
					"{entry} refused for the wrong reason: {detail}",
				);
			}

			// Refusals name the target too, wherever the check sits.
			let response = public
				.post(&format!("/status/{id}"))
				.add_header("x-forwarded-client-cert", &format!("Cert={cert}"))
				.json(&json!({
					"machine": {},
					"applications": { "central": {
						"type": "tamanu-central",
						"health": [{ "check": "c", "instances": { "a": { "result": "broken" } } }],
					} },
				}))
				.await;
			response.assert_status_bad_request();
			let body: Value = response.json();
			assert!(
				body["detail"].as_str().is_some_and(
					|d| d.contains("`applications.central.health[0].instances.a.result`")
				),
				"{body}",
			);

			let statuses: Count = sql_query("SELECT COUNT(*) AS count FROM statuses")
				.get_result(&mut conn)
				.await
				.expect("count statuses");
			assert_eq!(statuses.count, 0, "a refused push records nothing");
		},
	)
	.await
}

#[derive(QueryableByName)]
struct Count {
	#[diesel(sql_type = sql_types::BigInt)]
	count: i64,
}

/// A plain check's fields read the same whether sent flat or nested in
/// `detail`: graded by the same rule, stored as the same detail, and written
/// into the same message.
// spec: STA#health-and-detail
#[tokio::test(flavor = "multi_thread")]
async fn nested_and_flat_detail_read_alike() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(
				&mut conn,
				"db",
				"warning",
				Some(json!({ "if": [{ ">": [{ "var": "check.latency_ms" }, 100] }, "failed"] })),
			)
			.await;

			let mut seen = Vec::new();
			for entry in [
				json!({ "check": "db", "result": "warning", "latency_ms": 420 }),
				json!({ "check": "db", "result": "warning", "detail": { "latency_ms": 420 } }),
			] {
				push(&public, &cert, &mut conn, id, json!({ "health": [entry] })).await;
				let state = state(&mut conn, id, "health/db").await;
				assert_eq!(
					state.effective(),
					"failed",
					"the rule reads check.latency_ms"
				);
				assert_eq!(state.detail, Some(json!({ "latency_ms": 420 })));
				assert!(
					state.instances.is_none(),
					"a plain check holds no instances"
				);
				assert!(state.message.contains("`420`"), "{}", state.message);
				seen.push((state.detail, state.message));
			}
			assert_eq!(seen[0], seen[1]);
		},
	)
	.await
}

/// A field named `instances` or `detail` that is not an object is one of a
/// plain check's flat fields, as reporters already in the field send them:
/// alertd's `version_drift` carries its containers as an `instances` array.
// spec: STA#health-and-detail
#[tokio::test(flavor = "multi_thread")]
async fn non_object_structure_names_are_flat_fields() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			let containers = json!([{ "name": "tamanu-central-api", "version": "2.64.3" }]);
			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [
					{
						"check": "version_drift",
						"result": "passed",
						"expected": { "tamanu": "2.64.3", "frontend": "2.64.3" },
						"instances": containers,
						"summary": "1 container(s) on expected version 2.64.3",
					},
					{ "check": "db", "result": "passed", "detail": "fine", "latency_ms": 3 },
				] }),
			)
			.await;

			let drift = state(&mut conn, id, "health/version_drift").await;
			assert!(
				drift.instances.is_none(),
				"a flat `instances` is not a set of instances"
			);
			let detail = drift.detail.expect("the check's fields are its detail");
			assert_eq!(detail["instances"], containers);
			assert_eq!(detail["expected"]["tamanu"], "2.64.3");

			let db = state(&mut conn, id, "health/db").await;
			assert_eq!(
				db.detail,
				Some(json!({ "detail": "fine", "latency_ms": 3 }))
			);
		},
	)
	.await
}

/// Each instance is graded on its own, the check takes the most urgent of
/// them, and Canopy's message names the degraded ones by label, or by key
/// without one.
// spec: CHK#checks-with-instances
#[tokio::test(flavor = "multi_thread")]
async fn instances_are_graded_and_named_in_the_message() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(&mut conn, CHECK, "failed", None).await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[
					("north", "failed", "Northgate Clinic"),
					("harbour", "passed", "Harbour Hospital"),
					("ridge", "warning", ""),
				]),
			)
			.await;

			let state = state(&mut conn, id, REF).await;
			assert!(state.active);
			assert_eq!(state.effective(), "failed");
			assert_eq!(state.observed_result.as_deref(), Some("failed"));
			assert_eq!(state.keys(), ["harbour", "north", "ridge"]);
			assert_eq!(state.instance("north", "effective"), "failed");
			assert_eq!(state.instance("north", "label"), "Northgate Clinic");
			assert_eq!(state.instance("ridge", "effective"), "warning");
			assert_eq!(
				state.instance("harbour", "detail"),
				&json!({ "device": "harbour" })
			);
			assert_eq!(
				state.detail,
				Some(json!({ "fail_minutes": 30 })),
				"the check's detail holds its shared fields only",
			);
			assert!(
				state.message.contains("Northgate Clinic") && state.message.contains("ridge"),
				"degraded instances are named, by key without a label: {}",
				state.message,
			);
			assert!(
				!state.message.contains("Harbour"),
				"a passing instance is not named: {}",
				state.message,
			);
			assert!(state.description.is_some());
		},
	)
	.await
}

/// An instance left out of a push has recovered, and an empty set recovers
/// every instance the check held.
// spec: CHK#checks-with-instances, STA#instances
#[tokio::test(flavor = "multi_thread")]
async fn omitted_instances_recover() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(&mut conn, CHECK, "failed", None).await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[("north", "failed", ""), ("ridge", "warning", "")]),
			)
			.await;
			assert_eq!(state(&mut conn, id, REF).await.effective(), "failed");

			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[("ridge", "warning", "")]),
			)
			.await;
			let state_now = state(&mut conn, id, REF).await;
			assert_eq!(
				state_now.effective(),
				"warning",
				"north recovered by omission"
			);
			assert_eq!(state_now.keys(), ["ridge"]);
			assert!(
				!state_now.message.contains("north"),
				"{}",
				state_now.message
			);

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": CHECK, "instances": {} }] }),
			)
			.await;
			let state_now = state(&mut conn, id, REF).await;
			assert!(
				!state_now.active,
				"no instances left, so the check recovered"
			);
			assert_eq!(state_now.effective(), "passed");
			assert!(state_now.keys().is_empty());
			assert_eq!(
				state_now.instances,
				Some(json!({})),
				"still a check with instances, just none of them now",
			);
		},
	)
	.await
}

/// A check that could not run keeps the instances it held, presents each as
/// broken, retains its open failure, and recovers none of them.
// spec: CHK#checks-with-instances, STA#instances
#[tokio::test(flavor = "multi_thread")]
async fn a_broken_check_keeps_its_instances() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(&mut conn, CHECK, "failed", None).await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[
					("north", "failed", "Northgate Clinic"),
					("harbour", "passed", ""),
				]),
			)
			.await;
			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{
					"check": CHECK,
					"result": "broken",
					"detail": { "error": "relation sync_sessions does not exist" },
				}] }),
			)
			.await;

			let broken = state(&mut conn, id, REF).await;
			assert!(
				broken.active,
				"broken neither confirms nor clears the failure"
			);
			assert_eq!(broken.effective(), "failed", "the open failure is retained");
			assert_eq!(broken.observed_result.as_deref(), Some("broken"));
			assert_eq!(broken.keys(), ["harbour", "north"], "no instance recovered");
			for key in ["harbour", "north"] {
				assert_eq!(broken.instance(key, "observed"), "broken");
				assert_eq!(broken.instance(key, "effective"), "broken");
			}
			assert_eq!(
				broken.instance("north", "detail"),
				&json!({ "device": "north" }),
				"each held instance keeps the fields it was last given",
			);
			assert_eq!(
				broken.detail,
				Some(json!({ "error": "relation sync_sessions does not exist" })),
			);
			assert!(
				broken.message.contains("could not run"),
				"{}",
				broken.message
			);
			assert!(
				broken
					.description
					.as_deref()
					.is_some_and(|d| d.contains("broken")),
				"{:?}",
				broken.description,
			);

			// The next push that runs grades its instances afresh.
			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[("harbour", "passed", "")]),
			)
			.await;
			let recovered = state(&mut conn, id, REF).await;
			assert!(!recovered.active);
			assert_eq!(recovered.keys(), ["harbour"]);
		},
	)
	.await
}

/// A check moving between plain and instanced is one state throughout.
// spec: STA#instances
#[tokio::test(flavor = "multi_thread")]
async fn plain_and_instanced_are_one_state() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(&mut conn, CHECK, "failed", None).await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": CHECK, "result": "failed", "summary": "1 stale" }] }),
			)
			.await;
			let plain = state(&mut conn, id, REF).await;
			assert!(plain.instances.is_none());

			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[("north", "failed", "")]),
			)
			.await;
			let instanced = state(&mut conn, id, REF).await;
			assert_eq!(instanced.id, plain.id, "one state for the check");
			assert!(instanced.active);
			assert_eq!(instanced.keys(), ["north"]);

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": CHECK, "result": "passed" }] }),
			)
			.await;
			let back = state(&mut conn, id, REF).await;
			assert_eq!(back.id, plain.id);
			assert!(!back.active, "the plain pass recovers every instance");
			assert!(
				back.instances.is_none(),
				"back to a check without instances"
			);

			let issues: Count = sql_query(
				"SELECT COUNT(*) AS count FROM issues WHERE application_id = $1 AND ref = $2",
			)
			.bind::<sql_types::Uuid, _>(id)
			.bind::<sql_types::Text, _>(REF)
			.get_result(&mut conn)
			.await
			.expect("count issues");
			assert_eq!(issues.count, 1);
		},
	)
	.await
}

/// One rule on `check.<field>` grades a check's plain form and its instanced
/// form alike, reading an instance's field over the check's shared one.
// spec: CHK#checks-with-instances
#[tokio::test(flavor = "multi_thread")]
async fn one_rule_grades_both_forms_alike() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(
				&mut conn,
				CHECK,
				"warning",
				Some(json!({ "if": [{ ">": [{ "var": "check.minutes" }, 30] }, "failed"] })),
			)
			.await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": CHECK, "result": "warning", "minutes": 45 }] }),
			)
			.await;
			assert_eq!(state(&mut conn, id, REF).await.effective(), "failed");

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{
					"check": CHECK,
					"detail": { "minutes": 5 },
					"instances": {
						"north": { "result": "warning", "detail": { "minutes": 45 } },
						"ridge": { "result": "warning" },
					},
				}] }),
			)
			.await;
			let state = state(&mut conn, id, REF).await;
			assert_eq!(state.effective(), "failed");
			assert_eq!(
				state.instance("north", "effective"),
				"failed",
				"the instance's own field wins over the shared one",
			);
			assert_eq!(
				state.instance("ridge", "effective"),
				"warning",
				"an instance without the field reads the shared one",
			);
		},
	)
	.await
}

/// A rule evaluated for a reported instance reads the push's report fields and
/// the target's tags, as it does for a plain check.
// spec: CHK#checks-with-instances
#[tokio::test(flavor = "multi_thread")]
async fn instance_rules_read_the_report_and_tags() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			sql_query(
				"UPDATE applications SET tags = '{\"environment\": \"prod\"}'::jsonb WHERE id = $1",
			)
			.bind::<sql_types::Uuid, _>(id)
			.execute(&mut conn)
			.await
			.expect("set tags");
			set_policy(
				&mut conn,
				CHECK,
				"warning",
				Some(json!({ "if": [
					{ "==": [{ "var": "status.region" }, "north"] }, "failed",
					{ "==": [{ "var": "tag.environment" }, "prod"] }, "skipped",
				] })),
			)
			.await;

			let mut body = stale(&[("north", "warning", "")]);
			body["region"] = json!("north");
			push(&public, &cert, &mut conn, id, body).await;
			assert_eq!(
				state(&mut conn, id, REF)
					.await
					.instance("north", "effective"),
				"failed",
				"status.region reaches the instance's rule",
			);

			let mut body = stale(&[("north", "warning", "")]);
			body["region"] = json!("south");
			push(&public, &cert, &mut conn, id, body).await;
			let state = state(&mut conn, id, REF).await;
			assert_eq!(
				state.instance("north", "effective"),
				"skipped",
				"tag.environment reaches the instance's rule",
			);
			assert_eq!(state.effective(), "skipped", "every instance skipped");
		},
	)
	.await
}

/// Silencing one instance on the application quiets that instance alone: the
/// check is graded on the rest at once and on every later push, its message
/// stops naming the silenced one, and the reporter is still told the check's
/// own policy.
// spec: CHK#silencing-one-instance, STA#response
#[tokio::test(flavor = "multi_thread")]
async fn an_instance_silence_quiets_that_instance_only() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(&mut conn, CHECK, "failed", None).await;
			let body = stale(&[
				("north", "failed", "Northgate Clinic"),
				("ridge", "warning", "Ridge Health Centre"),
				("harbour", "passed", "Harbour Hospital"),
			]);

			push(&public, &cert, &mut conn, id, body.clone()).await;
			assert_eq!(state(&mut conn, id, REF).await.effective(), "failed");

			ServerSilencedRef::add(&mut conn, id, "alertd", REF, Some("north"), Some("test"))
				.await
				.expect("silence north");
			let regraded = state(&mut conn, id, REF).await;
			assert_eq!(
				regraded.effective(),
				"warning",
				"graded on the rest at once"
			);
			assert_eq!(regraded.instance("north", "effective"), "skipped");

			let response = push(&public, &cert, &mut conn, id, body).await;
			let filed = state(&mut conn, id, REF).await;
			assert_eq!(filed.effective(), "warning", "and on the next push");
			assert_eq!(filed.instance("north", "observed"), "failed");
			assert_eq!(filed.instance("north", "effective"), "skipped");
			assert_eq!(filed.instance("ridge", "effective"), "warning");
			assert!(
				filed.message.contains("Ridge Health Centre")
					&& !filed.message.contains("Northgate"),
				"{}",
				filed.message,
			);
			assert_eq!(
				response["check_severities"][CHECK], "fail",
				"an instance silence does not change what the reporter is told",
			);
		},
	)
	.await
}

/// The push response answers an instanced check once, under its name, and
/// nothing per instance, in the flat answer and in a split push's per-target
/// one. Machine checks take instances the same way.
// spec: STA#response
#[tokio::test(flavor = "multi_thread")]
async fn the_response_answers_an_instanced_check_once() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(&mut conn, CHECK, "failed", None).await;

			let response = push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[("north", "failed", ""), ("ridge", "passed", "")]),
			)
			.await;
			let severities = response["check_severities"]
				.as_object()
				.expect("check severities");
			assert_eq!(severities[CHECK], "fail");
			assert!(
				severities
					.keys()
					.all(|k| !k.contains("north") && !k.contains("ridge")),
				"{severities:?}",
			);

			let response = push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({
					"machine": {
						"health": [{
							"check": "disk_free",
							"instances": {
								"/": { "result": "passed" },
								"/var": { "result": "warning", "detail": { "free_percent": 6 } },
							},
						}],
					},
					"applications": {
						"central": {
							"type": "tamanu-central",
							"health": [{
								"check": CHECK,
								"instances": { "north": { "result": "failed" } },
							}],
						},
					},
				}),
			)
			.await;
			let central = &response["applications"]["central"]["check_severities"];
			assert_eq!(central[CHECK], "fail");
			assert!(central.get("north").is_none());
			let machine = &response["machine"]["check_severities"];
			assert!(machine.get("disk_free").is_some());
			assert!(machine.get("/var").is_none());

			let disk = state_where(&mut conn, "machine_id", id, "health/disk_free").await;
			assert!(disk.active);
			assert_eq!(disk.effective(), "warning");
			assert_eq!(disk.keys(), ["/", "/var"]);
		},
	)
	.await
}

/// Status history records the push as sent, `instances` and nested `detail`
/// included.
// spec: STA#health-and-detail
#[tokio::test(flavor = "multi_thread")]
async fn status_history_records_the_push_verbatim() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			let body = stale(&[
				("north", "failed", "Northgate Clinic"),
				("ridge", "passed", ""),
			]);
			push(&public, &cert, &mut conn, id, body.clone()).await;

			#[derive(QueryableByName)]
			struct Health {
				#[diesel(sql_type = sql_types::Jsonb)]
				health: Value,
			}
			let recorded: Health = sql_query("SELECT health FROM statuses WHERE server_id = $1")
				.bind::<sql_types::Uuid, _>(id)
				.get_result(&mut conn)
				.await
				.expect("status recorded");
			assert_eq!(recorded.health, body["health"]);
		},
	)
	.await
}

/// A rule grading a check that ran as broken neither confirms nor clears its
/// previous definite result: an open failure is retained through it, as it is
/// through a check reported broken, and a later definite pass clears it.
// spec: CHK#stability
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_graded_broken_retains_the_prior_failure() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(
				&mut conn,
				"db",
				"failed",
				Some(json!({ "if": [{ "==": [{ "var": "check.runner" }, "flaky"] }, "broken"] })),
			)
			.await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": "db", "result": "failed" }] }),
			)
			.await;
			assert_eq!(
				state(&mut conn, id, "health/db").await.effective(),
				"failed"
			);

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": "db", "result": "passed", "runner": "flaky" }] }),
			)
			.await;
			let held = state(&mut conn, id, "health/db").await;
			assert!(
				held.active,
				"a graded brokenness does not clear the failure"
			);
			assert_eq!(held.observed_result.as_deref(), Some("passed"));
			assert_eq!(held.effective(), "failed", "the open failure is retained");
			assert!(
				held.description
					.as_deref()
					.is_some_and(|d| d.contains("broken")),
				"{:?}",
				held.description,
			);

			push(
				&public,
				&cert,
				&mut conn,
				id,
				json!({ "health": [{ "check": "db", "result": "passed" }] }),
			)
			.await;
			let cleared = state(&mut conn, id, "health/db").await;
			assert!(!cleared.active);
			assert_eq!(cleared.effective(), "passed");
		},
	)
	.await
}

/// Brokenness is never one instance's, so a rule grading one instance as
/// broken grades it as a warning.
// spec: CHK#checks-with-instances
#[tokio::test(flavor = "multi_thread")]
async fn a_rule_grading_an_instance_broken_warns() {
	commons_tests::server::run_with_device_auth(
		"server",
		async |mut conn, cert, device_id, public, _| {
			let id = central(&mut conn, device_id).await;
			set_policy(
				&mut conn,
				CHECK,
				"failed",
				Some(json!({ "if": [{ "==": [{ "var": "check.device" }, "north"] }, "broken"] })),
			)
			.await;

			push(
				&public,
				&cert,
				&mut conn,
				id,
				stale(&[("north", "failed", ""), ("harbour", "passed", "")]),
			)
			.await;
			let state = state(&mut conn, id, REF).await;
			assert_eq!(state.instance("north", "effective"), "warning");
			assert_eq!(state.instance("harbour", "effective"), "passed");
			assert_eq!(state.effective(), "warning");
			assert!(state.active);
		},
	)
	.await
}
