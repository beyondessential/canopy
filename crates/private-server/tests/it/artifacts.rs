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
				"digest": database::artifacts::digest_of(b"hello"),
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
				"digest": database::artifacts::digest_of(b"hello"),
			}),
			// Bytes that are not the digest the registration names.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
				"group_id": group, "content_base64": "aGVsbG8=",
				"digest": database::artifacts::digest_of(b"something else"),
			}),
			// Bytes with no digest to check them against.
			serde_json::json!({
				"version_id": version, "artifact_type": "installer", "platform": "any",
				"group_id": group, "content_base64": "aGVsbG8=",
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
				"digest": database::artifacts::digest_of(b"kamaka schema"),
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

/// A media type describes bytes Canopy holds, and it holds none for an
/// artifact that names no group. Passed through, it trips the check constraint,
/// so operator input answers 500 instead of being refused.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn an_unscoped_artifact_carries_no_media_type() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "11111111-2222-0000-0000-111111111111";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published')",
		))
		.await
		.unwrap();

		let created = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "installer",
				"platform": "windows",
				"download_url": "https://example.com/x.exe",
				"content_type": "text/html",
			}))
			.await;
		assert_eq!(created.status_code(), axum::http::StatusCode::BAD_REQUEST);
	})
	.await
}

/// The media type is served back as a header when the bytes are read, so one no
/// header can carry would leave an artifact that answers a fault on every
/// download and can only be mended by registering it again.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn a_media_type_no_header_can_carry_is_refused() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "11111111-4444-0000-0000-111111111111";
		let group = "cccccccc-4444-0000-0000-cccccccccccc";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka')",
		))
		.await
		.unwrap();

		let created = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "reporting-schema",
				"platform": "any",
				"group_id": group,
				"content_base64": "a2FtYWthIHNjaGVtYQ==",
				"content_type": "application/sql\r\nx-injected: yes",
				"digest": database::artifacts::digest_of(b"kamaka schema"),
			}))
			.await;
		assert_eq!(created.status_code(), axum::http::StatusCode::BAD_REQUEST);
	})
	.await
}

/// The group an artifact names is a foreign key, so an id that names no group
/// answers the operator's own input with a database fault instead of a refusal.
// spec: ART#registration
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_naming_no_group_that_exists_is_refused() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "11111111-5555-0000-0000-111111111111";
		let gone = "cccccccc-5555-0000-0000-cccccccccccc";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published')",
		))
		.await
		.unwrap();

		let created = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "reporting-schema",
				"platform": "any",
				"group_id": gone,
				"content_base64": "a2FtYWthIHNjaGVtYQ==",
				"digest": database::artifacts::digest_of(b"kamaka schema"),
			}))
			.await;
		assert_eq!(created.status_code(), axum::http::StatusCode::BAD_REQUEST);
	})
	.await
}

/// A digest against a location is what whoever fetches the artifact checks the
/// bytes it got against. Dropped, a caller that supplied one gets no error and
/// no digest, and the fetch is unchecked.
// spec: ART#digests
#[tokio::test(flavor = "multi_thread")]
async fn a_digest_against_a_location_is_recorded() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "11111111-3333-0000-0000-111111111111";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published')",
		))
		.await
		.unwrap();

		let created = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "installer",
				"platform": "windows",
				"download_url": "https://example.com/x.exe",
				"digest": database::artifacts::digest_of(b"kamaka installer"),
			}))
			.await;
		created.assert_status_ok();

		let artifact: serde_json::Value = created.json();
		assert_eq!(
			artifact["digest"],
			database::artifacts::digest_of(b"kamaka installer")
		);
	})
	.await
}

/// A blank location is no location. The check constraint only tests for NULL,
/// so an empty string would pass it and leave an artifact nothing can be
/// fetched from.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn a_blank_download_url_is_not_a_location() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "dddddddd-0000-0000-0000-dddddddddddd";
		let artifact = "eeeeeeee-0000-0000-0000-eeeeeeeeeeee";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url)
			 VALUES ('{artifact}', '{version}', 'any', 'installer', 'https://example.com/x.exe')",
		))
		.await
		.unwrap();

		let created = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "installer",
				"platform": "linux",
				"download_url": "   ",
			}))
			.await;
		assert_eq!(created.status_code(), axum::http::StatusCode::BAD_REQUEST);

		let updated = private
			.post("/api/versions/update_artifact")
			.json(&serde_json::json!({
				"artifact_id": artifact,
				"artifact_type": "installer",
				"platform": "any",
				"download_url": "",
			}))
			.await;
		assert_eq!(updated.status_code(), axum::http::StatusCode::CONFLICT);

		let listed = private
			.post("/api/versions/get_version_artifacts")
			.json(&serde_json::json!({ "version": "2.60.0" }))
			.await;
		let artifacts: Vec<serde_json::Value> = listed.json();
		assert_eq!(artifacts.len(), 1, "nothing blank was written");
		assert_eq!(artifacts[0]["download_url"], "https://example.com/x.exe");
	})
	.await
}

/// The create route carries a body limit sized from the held-bytes cap, so an
/// upload well past axum's 2 MB default is accepted, and one past the cap is
/// refused by the handler naming the limit rather than by axum with a
/// plain-text 413 the SPA has nothing structured to render.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn an_upload_over_the_limit_is_told_what_it_is() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "ffffffff-0000-0000-0000-ffffffffffff";
		let group = "ffffffff-1111-1111-1111-ffffffffffff";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka')",
		))
		.await
		.unwrap();

		// "AAAA" decodes to three zero bytes, so the repeat count sets the size.
		let four_mib_bytes = 3 * (4 * 1024 * 1024 / 3);
		let four_mib = "A".repeat(4 * (four_mib_bytes / 3));
		let accepted = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "reporting-schema",
				"platform": "any",
				"group_id": group,
				"content_base64": four_mib,
				"digest": database::artifacts::digest_of(&vec![0u8; four_mib_bytes]),
			}))
			.await;
		accepted.assert_status_ok();

		let over_limit = "A".repeat(4 * (32 * 1024 * 1024 / 3 + 1));
		let refused = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "reporting-schema",
				"platform": "linux",
				"group_id": group,
				"content_base64": over_limit,
			}))
			.await;

		assert_eq!(refused.status_code(), axum::http::StatusCode::BAD_REQUEST);
		let problem: serde_json::Value = refused.json();
		assert!(
			problem["title"]
				.as_str()
				.expect("a problem-details title")
				.contains("32 MiB"),
			"the refusal names the limit, but got: {problem}"
		);
	})
	.await
}

/// The listing's offered flag, over the wire. An artifact is offered where it
/// wins inside a scope that is actually resolved, so an unscoped artifact and
/// the group's own that displaces it are both served, to different callers, and
/// both say so. A range the exact displaces inside one scope is served to
/// nobody.
// spec: ART#what-a-version-offers
#[tokio::test(flavor = "multi_thread")]
async fn the_listing_says_which_artifacts_are_offered() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "11111111-2222-0000-0000-111111111111";
		let group = "11111111-3333-0000-0000-111111111111";
		let unscoped_schema = "11111111-4444-0000-0000-111111111111";
		let group_schema = "11111111-5555-0000-0000-111111111111";
		let exact_installer = "11111111-6666-0000-0000-111111111111";
		let range_installer = "11111111-7777-0000-0000-111111111111";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO server_groups (id, name) VALUES ('{group}', 'kamaka');

			 INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url)
			 VALUES ('{unscoped_schema}', '{version}', 'any', 'reporting-schema', 'https://example.com/all.sql'),
			        ('{exact_installer}', '{version}', 'windows', 'installer', 'https://example.com/exact.exe');

			 INSERT INTO artifacts (id, version_id, platform, artifact_type, version_range_pattern, download_url)
			 VALUES ('{range_installer}', NULL, 'windows', 'installer', '2.60.x', 'https://example.com/range.exe');

			 INSERT INTO artifacts (id, version_id, platform, artifact_type, group_id, content, content_type, digest)
			 VALUES ('{group_schema}', '{version}', 'any', 'reporting-schema', '{group}', 'kamaka schema', 'application/sql', 'sha256:x')",
		))
		.await
		.unwrap();

		let response = private
			.post("/api/versions/get_version_artifacts")
			.json(&serde_json::json!({ "version": "2.60.0" }))
			.await;
		response.assert_status_ok();
		let artifacts: Vec<serde_json::Value> = response.json();

		let offered = |id: &str| -> bool {
			artifacts
				.iter()
				.find(|a| a["id"] == id)
				.unwrap_or_else(|| panic!("{id} is listed"))["is_used_in_public_api"]
				.as_bool()
				.expect("a flag")
		};

		assert!(
			offered(group_schema),
			"the group is offered the one held for it"
		);
		assert!(
			offered(unscoped_schema),
			"every other group is still offered the unscoped one"
		);
		assert!(offered(exact_installer), "the exact wins its own scope");
		assert!(
			!offered(range_installer),
			"the range it displaces is served to nobody"
		);
	})
	.await
}

/// What an exact artifact overrides follows the resolution rules rather than a
/// group match: it displaces a range its own scope can see, so a group's exact
/// artifact overrides an unscoped range, and an unscoped exact overrides
/// nothing in a group whose own range outranks it. The registration answers
/// with what the listing would say rather than describing the row a second
/// time.
// spec: ART#what-a-version-offers
#[tokio::test(flavor = "multi_thread")]
async fn a_registration_answers_what_it_overrides() {
	commons_tests::server::run(async |mut conn, _public, private| {
		let version = "22222222-1111-0000-0000-222222222222";
		let ours = "22222222-2222-0000-0000-222222222222";
		let theirs = "22222222-3333-0000-0000-222222222222";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status)
			 VALUES ('{version}', 2, 60, 0, '', 'published');
			 INSERT INTO server_groups (id, name) VALUES
			 ('{ours}', 'kamaka'), ('{theirs}', 'drifting');

			 INSERT INTO artifacts (version_id, platform, artifact_type, version_range_pattern, download_url)
			 VALUES (NULL, 'any', 'reporting-schema', '2.60.x', 'https://example.com/range.sql');

			 INSERT INTO artifacts (version_id, platform, artifact_type, version_range_pattern, group_id, content, content_type, digest)
			 VALUES (NULL, 'windows', 'installer', '2.60.x', '{theirs}', 'theirs', 'application/octet-stream', 'sha256:x')",
		))
		.await
		.unwrap();

		let held = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "reporting-schema",
				"platform": "any",
				"group_id": ours,
				"content_base64": "a2FtYWthIHNjaGVtYQ==",
				"digest": database::artifacts::digest_of(b"kamaka schema"),
			}))
			.await;
		held.assert_status_ok();
		let held: serde_json::Value = held.json();
		assert_eq!(held["is_exact"], true);
		assert_eq!(
			held["has_range_override"], true,
			"the group's own displaces the unscoped range for that group"
		);
		assert_eq!(held["is_used_in_public_api"], true);

		let unscoped = private
			.post("/api/versions/create_artifact")
			.json(&serde_json::json!({
				"version_id": version,
				"artifact_type": "installer",
				"platform": "windows",
				"download_url": "https://example.com/x.exe",
			}))
			.await;
		unscoped.assert_status_ok();
		let unscoped: serde_json::Value = unscoped.json();
		assert_eq!(
			unscoped["has_range_override"], false,
			"a group's range outranks an unscoped exact, so nothing is displaced"
		);
	})
	.await
}
