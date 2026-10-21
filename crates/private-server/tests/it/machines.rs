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

/// Adding a machine is one transaction: a form refused part-way through, here
/// for a reserved tag key, leaves no machine behind for a retry to duplicate.
#[tokio::test(flavor = "multi_thread")]
async fn a_refused_addition_leaves_no_machine() {
	commons_tests::server::run(async |_conn, _, private| {
		let refused = private
			.post("/api/fleet/machines/create")
			.json(&json!({
				"name": "half-made",
				"rank": "production",
				"alert_when_unreachable": false,
				"tags": { "canopy:anything": "x" },
			}))
			.await;
		refused.assert_status_bad_request();

		let listed = private
			.post("/api/fleet/machines/list")
			.json(&json!({}))
			.await;
		listed.assert_status_ok();
		let machines: Vec<serde_json::Value> = listed.json();
		assert!(
			machines.iter().all(|m| m["name"] != "half-made"),
			"{machines:?}"
		);
	})
	.await
}

/// A machine added with a rank carries it from the start, so what arrives on
/// it is never pending; one added without stays pending until ranked.
// spec: FLT#machines-come-from-operators
#[tokio::test(flavor = "multi_thread")]
async fn adding_a_machine_ranks_it() {
	commons_tests::server::run(async |mut conn, _, private| {
		use commons_types::server::rank::ServerRank;
		use database::machines::Machine;

		let ranked = private
			.post("/api/fleet/machines/create")
			.json(&json!({ "name": "ranked-box", "rank": "test" }))
			.await;
		ranked.assert_status_ok();
		let ranked: commons_types::Uuid = ranked.json();
		let ranked = Machine::get_by_id(&mut conn, ranked).await.unwrap();
		assert_eq!(ranked.rank, Some(ServerRank::Test));

		let pending = private
			.post("/api/fleet/machines/create")
			.json(&json!({ "name": "pending-box" }))
			.await;
		pending.assert_status_ok();
		let pending: commons_types::Uuid = pending.json();
		let pending = Machine::get_by_id(&mut conn, pending).await.unwrap();
		assert_eq!(pending.rank, None);
	})
	.await
}

/// Adding a machine with its reachability alert off writes the machine-scoped
/// silence on canopy's reachability check as part of the addition, so the one
/// write-graded call covers it. Left on, nothing is silenced.
// spec: SAFE#grading-the-administrative-surface
#[tokio::test(flavor = "multi_thread")]
async fn adding_a_machine_can_silence_its_reachability() {
	commons_tests::server::run(async |_conn, _, private| {
		let silences = async |id: &str| -> Vec<serde_json::Value> {
			let listed = private
				.post("/api/silenced_refs/list_for_machine")
				.json(&json!({ "machine_id": id }))
				.await;
			listed.assert_status_ok();
			listed.json()
		};

		let quiet = private
			.post("/api/fleet/machines/create")
			.json(&json!({ "name": "comes-and-goes", "alert_when_unreachable": false }))
			.await;
		quiet.assert_status_ok();
		let quiet: String = quiet.json();
		let rows = silences(&quiet).await;
		assert_eq!(rows.len(), 1, "{rows:?}");
		assert_eq!(rows[0]["source"], "canopy");
		assert_eq!(rows[0]["ref"], "reachability");
		assert_eq!(rows[0]["instance"], serde_json::Value::Null);

		for body in [
			json!({ "name": "stays-up" }),
			json!({ "name": "stays-up-too", "alert_when_unreachable": true }),
		] {
			let created = private.post("/api/fleet/machines/create").json(&body).await;
			created.assert_status_ok();
			let id: String = created.json();
			assert!(silences(&id).await.is_empty(), "{body}");
		}
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
