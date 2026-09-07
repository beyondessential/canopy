//! Endpoint tests for the secret variables an environment or an application
//! carries: that a value reaches a run, that a name is a tag or a secret and
//! never both, and that a value Canopy cannot produce refuses the whole
//! inventory.
//!
//! spec: INV#secret-variables

use commons_tests::diesel_async::{AsyncPgConnection, SimpleAsyncConnection};
use serde_json::{Value, json};
use uuid::Uuid;

use crate::inventory::{insert_group, insert_ranked_application};

/// Record a name with no value behind it, which only a lost or half-written
/// secret store would produce.
async fn declare_without_value(conn: &mut AsyncPgConnection, group: Uuid, rank: &str, name: &str) {
	conn.batch_execute(&format!(
		"INSERT INTO inventory_secret_variables (server_group_id, rank, name)
		 VALUES ('{group}', '{rank}', '{name}')"
	))
	.await
	.expect("declare secret");
}

#[tokio::test(flavor = "multi_thread")]
async fn an_environment_secret_reaches_every_host() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(
			&mut conn,
			"kamaka",
			json!({ "timezone": "Pacific/Auckland" }),
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-facility",
			"tamanu-facility",
			Some("production"),
			None,
			json!({}),
		)
		.await;

		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "production",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_ok();

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "server_group_id": group, "rank": "production" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();

		assert_eq!(body["vars"]["salt"], "pepper");
		assert_eq!(body["secret_vars"], json!(["salt"]));
		let hosts = body["hosts"].as_array().expect("hosts");
		assert_eq!(hosts.len(), 2);
		for host in hosts {
			assert_eq!(host["vars"]["salt"], "pepper");
			assert_eq!(host["secret_vars"], json!(["salt"]));
			// It belongs to the environment, so it is not the application's own.
			assert!(host["own_vars"].get("salt").is_none());
		}
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_applications_secret_overlays_the_environments() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka", json!({})).await;
		let central = insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-facility",
			"tamanu-facility",
			Some("production"),
			None,
			json!({}),
		)
		.await;

		for args in [
			json!({ "server_group_id": group, "rank": "production", "name": "salt", "value": "shared" }),
			json!({ "application_id": central, "name": "salt", "value": "its-own" }),
		] {
			private
				.post("/api/inventory_secrets/set")
				.json(&args)
				.await
				.assert_status_ok();
		}

		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "server_group_id": group, "rank": "production" }))
			.await;
		response.assert_status_ok();
		let body: Value = response.json();
		let hosts = body["hosts"].as_array().expect("hosts");

		assert_eq!(hosts[0]["name"], "kamaka-prod-central");
		assert_eq!(hosts[0]["vars"]["salt"], "its-own");
		assert_eq!(hosts[0]["own_vars"]["salt"], "its-own");
		assert_eq!(hosts[1]["vars"]["salt"], "shared");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_secret_with_no_value_refuses_the_inventory() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(
			&mut conn,
			"kamaka",
			json!({ "timezone": "Pacific/Auckland" }),
		)
		.await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		declare_without_value(&mut conn, group, "production", "salt").await;

		// Serving the rest would hand a run a member that looks configured and
		// is missing a value.
		let response = private
			.post("/api/inventory/for_group")
			.json(&json!({ "server_group_id": group, "rank": "production" }))
			.await;
		response.assert_status(axum::http::StatusCode::BAD_GATEWAY);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_name_is_a_tag_or_a_secret_and_never_both() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(
			&mut conn,
			"kamaka",
			json!({ "timezone": "Pacific/Auckland" }),
		)
		.await;
		let central = insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;

		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "production",
				"name": "timezone",
				"value": "pepper",
			}))
			.await
			.assert_status_bad_request();

		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "production",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_ok();

		// A group tag collides at any rank under it, the group's tags reaching
		// every environment in it.
		private
			.post("/api/fleet/groups/update")
			.json(&json!({
				"server_group_id": group,
				"data": { "tags": { "salt": "in-the-clear" } },
			}))
			.await
			.assert_status_bad_request();

		// And the other way round: the application tag write is refused too.
		private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": central,
				"data": { "tags": { "salt": "in-the-clear" } },
			}))
			.await
			.assert_status_bad_request();

		// As is carrying the name in from another group.
		let other = insert_group(&mut conn, "drifting", json!({})).await;
		let mover = insert_ranked_application(
			&mut conn,
			other,
			"drifting-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({ "salt": "in-the-clear" }),
		)
		.await;
		private
			.post("/api/fleet/applications/update")
			.json(&json!({
				"server_id": mover,
				"data": { "group_id": group, "rank": "production" },
			}))
			.await
			.assert_status_bad_request();
	})
	.await;
}

/// The applications on a box take its group, so moving the box is the move
/// that carries a tag into an environment holding a secret of that name.
#[tokio::test(flavor = "multi_thread")]
async fn moving_a_machine_cannot_carry_a_colliding_tag_in() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "production",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_ok();

		let other = insert_group(&mut conn, "drifting", json!({})).await;
		let machine = Uuid::new_v4();
		let mover = Uuid::new_v4();
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, name, group_id)
			 VALUES ('{machine}', 'drifting-box', '{other}');
			 INSERT INTO applications (id, name, type, rank, group_id, machine_id, tags)
			 VALUES ('{mover}', 'drifting-central', 'tamanu-central', 'production', '{other}', '{machine}', '{{\"salt\": \"in-the-clear\"}}')"
		))
		.await
		.expect("seed the mover");

		private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": machine, "group_id": group }))
			.await
			.assert_status_bad_request();

		// Refused rather than half-applied: the box is still where it was.
		let response = private
			.post("/api/fleet/machines/get")
			.json(&json!({ "machine_id": machine }))
			.await;
		response.assert_status_ok();
		assert_eq!(response.json::<Value>()["group_id"], other.to_string());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_names_are_listed_without_their_values() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "production",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_ok();

		let response = private
			.post("/api/inventory_secrets/for_group")
			.json(&json!({ "server_group_id": group }))
			.await;
		response.assert_status_ok();
		let body = response.text();
		assert!(
			!body.contains("pepper"),
			"a value must not be listed: {body}"
		);

		let listed: Value = serde_json::from_str(&body).expect("json");
		let listed = listed.as_array().expect("array");
		assert_eq!(listed.len(), 1);
		assert_eq!(listed[0]["name"], "salt");
		assert_eq!(listed[0]["rank"], "production");
		assert_eq!(listed[0]["set_by"], "admin@localhost");
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn removing_forgets_the_value() {
	commons_tests::server::run(async move |mut conn, _public, private| {
		let group = insert_group(&mut conn, "kamaka", json!({})).await;
		insert_ranked_application(
			&mut conn,
			group,
			"kamaka-prod-central",
			"tamanu-central",
			Some("production"),
			None,
			json!({}),
		)
		.await;
		let scope = json!({ "server_group_id": group, "rank": "production" });

		private
			.post("/api/inventory_secrets/set")
			.json(&json!({
				"server_group_id": group,
				"rank": "production",
				"name": "salt",
				"value": "pepper",
			}))
			.await
			.assert_status_ok();
		private
			.post("/api/inventory_secrets/remove")
			.json(&json!({ "server_group_id": group, "rank": "production", "name": "salt" }))
			.await
			.assert_status_ok();

		// Gone from the inventory, and gone rather than left valueless: a
		// leftover declaration would refuse every read.
		let response = private.post("/api/inventory/for_group").json(&scope).await;
		response.assert_status_ok();
		let body: Value = response.json();
		assert_eq!(body["secret_vars"], json!([]));
		assert!(body["vars"].get("salt").is_none());

		private
			.post("/api/inventory_secrets/remove")
			.json(&json!({ "server_group_id": group, "rank": "production", "name": "salt" }))
			.await
			.assert_status_not_found();
	})
	.await;
}
