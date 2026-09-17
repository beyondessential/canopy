//! Machines through the operator API.

use serde_json::json;

/// Every field on the create form is optional, including the group an edit can
/// supply later, so a machine can be added with nothing filled in at all.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_is_created_with_nothing_but_the_defaults() {
	commons_tests::server::run(async |_conn, _, private| {
		let created = private
			.post("/api/fleet/machines/create")
			.json(&json!({}))
			.await;
		created.assert_status_ok();
		let id: String = created.json();

		let listed = private
			.post("/api/fleet/machines/list")
			.json(&json!({}))
			.await;
		listed.assert_status_ok();
		let machines: Vec<serde_json::Value> = listed.json();
		let matching = machines.iter().filter(|m| m["id"] == id).count();
		assert_eq!(matching, 1, "{machines:?}");
	})
	.await
}
