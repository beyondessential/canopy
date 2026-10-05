//! Machines through the operator API.

use serde_json::json;

/// A name is the one thing every machine has, so a machine can be added with
/// that and nothing else; the group an edit can supply later.
// spec: FLT#naming
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_is_created_with_nothing_but_a_name() {
	commons_tests::server::run(async |_conn, _, private| {
		let created = private
			.post("/api/fleet/machines/create")
			.json(&json!({ "name": "  box-1  " }))
			.await;
		created.assert_status_ok();
		let id: String = created.json();

		let listed = private
			.post("/api/fleet/machines/list")
			.json(&json!({}))
			.await;
		listed.assert_status_ok();
		let machines: Vec<serde_json::Value> = listed.json();
		let matching: Vec<_> = machines.iter().filter(|m| m["id"] == id).collect();
		assert_eq!(matching.len(), 1, "{machines:?}");
		assert_eq!(matching[0]["name"], "box-1", "the name is trimmed");
	})
	.await
}

/// A machine always has a name: creating one without a name, or renaming one
/// to blank, is refused rather than leaving a box Canopy can only call by its
/// id.
// spec: FLT#naming
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_is_never_left_without_a_name() {
	commons_tests::server::run(async |_conn, _, private| {
		let missing = private
			.post("/api/fleet/machines/create")
			.json(&json!({}))
			.await;
		assert!(
			missing.status_code().is_client_error(),
			"{}",
			missing.text()
		);

		let blank = private
			.post("/api/fleet/machines/create")
			.json(&json!({ "name": "   " }))
			.await;
		blank.assert_status_bad_request();

		let created = private
			.post("/api/fleet/machines/create")
			.json(&json!({ "name": "box-1" }))
			.await;
		created.assert_status_ok();
		let id: String = created.json();

		let renamed_blank = private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": id, "name": "" }))
			.await;
		renamed_blank.assert_status_bad_request();

		let cleared = private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": id, "name": null }))
			.await;
		cleared.assert_status_ok();
		let machine: serde_json::Value = cleared.json();
		assert_eq!(machine["name"], "box-1", "a null name leaves it alone");

		let renamed = private
			.post("/api/fleet/machines/update")
			.json(&json!({ "machine_id": id, "name": "box-2" }))
			.await;
		renamed.assert_status_ok();
		let machine: serde_json::Value = renamed.json();
		assert_eq!(machine["name"], "box-2");
	})
	.await
}
