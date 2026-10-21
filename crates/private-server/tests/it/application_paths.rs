//! The fleet's endpoints live under `/api/fleet`, and every address they used
//! to answer at still lands.
//!
//! `servers` named the box and the workload at once, and the fleet's records
//! were spread across three top-level prefixes. Each has its own word now, all
//! three under one.

use axum::http::StatusCode;

#[tokio::test(flavor = "multi_thread")]
async fn the_fleets_endpoints_answer_under_fleet() {
	commons_tests::server::run(async |_conn, _, private| {
		for path in [
			"/api/fleet/applications/list",
			"/api/fleet/machines/list",
			"/api/fleet/groups/list",
		] {
			let response = private.post(path).json(&serde_json::json!({})).await;
			response.assert_status_ok();
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_old_prefix_redirects_permanently() {
	commons_tests::server::run(async |_conn, _, private| {
		for (from, to) in [
			// `servers` was the applications' prefix before the split.
			("/api/servers/list", "/api/fleet/applications/list"),
			("/api/applications/list", "/api/fleet/applications/list"),
			("/api/machines/list", "/api/fleet/machines/list"),
			("/api/server_groups/list", "/api/fleet/groups/list"),
		] {
			let response = private.post(from).json(&serde_json::json!({})).await;
			// Permanent, not temporary: these are POSTs, and only the permanent
			// form obliges the caller to repeat the method and body rather than
			// retrying as a GET.
			response.assert_status(StatusCode::PERMANENT_REDIRECT);
			response.assert_header("location", to);
		}
	})
	.await
}

/// The redirects must not shadow what they point at, which a route matching
/// the new prefix as well as the old would do — as a loop.
#[tokio::test(flavor = "multi_thread")]
async fn the_new_prefix_is_not_itself_redirected() {
	commons_tests::server::run(async |_conn, _, private| {
		let response = private
			.post("/api/fleet/applications/list")
			.json(&serde_json::json!({}))
			.await;
		response.assert_status_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_fleet_pages_are_left_to_the_spa() {
	commons_tests::server::run(async |_conn, _, private| {
		// `/fleet/figures` without the `/api` prefix is the SPA's own route, and
		// shares only a word with the endpoints.
		let response = private.get("/fleet/figures").await;
		response.assert_status_ok();
		response.assert_header("content-type", "text/html; charset=utf-8");
	})
	.await
}
