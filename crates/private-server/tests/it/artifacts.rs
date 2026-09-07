use commons_tests::diesel_async::SimpleAsyncConnection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize)]
pub struct ArtifactData {
	pub id: Uuid,
	pub artifact_type: String,
	pub platform: String,
	pub download_url: String,
	pub is_exact: bool,
	pub version_range_pattern: Option<String>,
	pub has_range_override: bool,
	pub is_used_in_public_api: bool,
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_multiple_ranges_pattern_specificity_private_endpoint() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version_id_245 = "44444444-4444-4444-4444-444444444444";
		let broader_range_id = "55555555-5555-5555-5555-555555555555";
		let narrower_range_id = "66666666-6666-6666-6666-666666666666";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('{version_id_245}', 2, 44, 5, 'v2.44.5', 'published');

			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url, version_range_pattern) VALUES
			('{broader_range_id}', NULL, 'windows', 'installer', 'https://example.com/2.44.x.exe', '2.44.x'),
			('{narrower_range_id}', NULL, 'windows', 'installer', 'https://example.com/caret.exe', '^2.44.2')",
		))
		.await
		.unwrap();

		// The operator view shows every artifact that matches, including the
		// ones specificity passed over, and marks which one is actually served.
		// What resolution hides is a fact about how a version was published.
		// spec: ART#what-a-version-offers
		let response = private
			.post("/api/versions/get_version_artifacts")
			.json(&serde_json::json!({"version": "2.44.5"}))
			.await;

		response.assert_status_ok();
		let artifacts: Vec<ArtifactData> = response.json();

		assert_eq!(artifacts.len(), 2, "both matching ranges are shown");

		let chosen = artifacts
			.iter()
			.find(|a| a.is_used_in_public_api)
			.expect("one of them is the one served");
		assert_eq!(chosen.id.to_string(), narrower_range_id.to_lowercase());
		assert_eq!(
			chosen.version_range_pattern,
			Some("^2.44.2".to_string()),
			"the more specific range is the one served"
		);

		let passed_over = artifacts
			.iter()
			.find(|a| !a.is_used_in_public_api)
			.expect("the broader range is shown but not served");
		assert_eq!(passed_over.id.to_string(), broader_range_id.to_lowercase());
	})
	.await
}

/// A registration that names neither a location nor a group, names both a group
/// and a location, or carries bytes without a group, is a client mistake and is
/// refused as one. Writing the row and letting the check constraint catch it
/// answers 500 for input the operator controls.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_that_rests_nowhere_is_refused() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "77777777-7777-7777-7777-777777777777";
		let group = "88888888-8888-8888-8888-888888888888";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka')",
		))
		.await
		.unwrap();

		let refusals = [
			// Neither a location nor a group.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
			}),
			// A group and a location together: it rests in one place or the other.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
				"group_id": group, "content_base64": "aGVsbG8=",
				"download_url": "https://example.com/x.exe",
			}),
			// A group with no bytes to hold.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
				"group_id": group,
			}),
			// Bytes with no group to hold them for.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
				"content_base64": "aGVsbG8=", "download_url": "https://example.com/x.exe",
			}),
			// Bytes that are not base64.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
				"group_id": group, "content_base64": "not base64 at all!!",
			}),
		];

		for args in refusals {
			let response = private
				.post("/api/versions/create_artifact")
				.json(&args)
				.await;
			assert_eq!(
				response.status_code(),
				axum::http::StatusCode::BAD_REQUEST,
				"refused as a client mistake: {args}"
			);
		}
	})
	.await
}

/// Editing only an artifact's type or platform must not take its location away.
/// The field is optional on the wire, so an omitted URL used to null the column
/// and fail the constraint, losing the artifact and answering 500.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn an_unscoped_artifact_cannot_lose_its_location() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "99999999-9999-9999-9999-999999999999";
		let artifact = "aaaaaaaa-0000-0000-0000-aaaaaaaaaaaa";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url)
			 VALUES ('{artifact}', '{version}', 'any', 'installer', 'https://example.com/x.exe')",
		))
		.await
		.unwrap();

		let response = private
			.post("/api/versions/update_artifact")
			.json(&serde_json::json!({
				"artifact_id": artifact,
				"artifact_type": "installer",
				"platform": "windows",
			}))
			.await;
		// Refused as a conflict, not left to the check constraint, which would
		// answer 500 for something the operator asked for.
		assert_eq!(response.status_code(), axum::http::StatusCode::CONFLICT);

		let listed = private
			.post("/api/versions/get_version_artifacts")
			.json(&serde_json::json!({ "version": "2.60.0" }))
			.await;
		listed.assert_status_ok();
		let artifacts: Vec<serde_json::Value> = listed.json();
		assert_eq!(artifacts.len(), 1);
		assert_eq!(
			artifacts[0]["download_url"], "https://example.com/x.exe",
			"the location it had is still the location it has"
		);
	})
	.await
}

/// An operator registers a group-scoped artifact by carrying its bytes. Canopy
/// holds them, takes the digest of what it received, and offers the group's
/// name back so the operator can see whose it is.
// spec: ART#where-an-artifact-rests, ART#digests
#[tokio::test(flavor = "multi_thread")]
async fn an_operator_registers_a_group_scoped_artifact() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "bbbbbbbb-0000-0000-0000-bbbbbbbbbbbb";
		let group = "cccccccc-0000-0000-0000-cccccccccccc";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka')",
		))
		.await
		.unwrap();

		// "kamaka schema" — the digest asserted below is of exactly these bytes.
		let response = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "reporting-schema",
				"platform": "any",
				"group_id": group,
				"content_base64": "a2FtYWthIHNjaGVtYQ==",
				"content_type": "application/sql",
			}))
			.await;
		response.assert_status_ok();

		let artifact: serde_json::Value = response.json();
		assert_eq!(artifact["canopy_holds_bytes"], true);
		assert!(artifact["download_url"].is_null(), "it rests in Canopy");
		assert_eq!(artifact["group_id"], group);
		assert_eq!(artifact["group_name"], "kamaka");
		assert_eq!(
			artifact["digest"],
			database::artifacts::digest_of(b"kamaka schema")
		);
	})
	.await
}
