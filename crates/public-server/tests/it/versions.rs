use axum::http::StatusCode;
use commons_types::version::VersionStatus;
use diesel_async::SimpleAsyncConnection;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Version {
	pub id: Uuid,
	pub major: i32,
	pub minor: i32,
	pub patch: i32,
	pub changelog: String,
	pub status: VersionStatus,
	pub device_id: Option<Uuid>,
}

#[derive(Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct Artifact {
	pub id: Uuid,
	pub version_id: Option<Uuid>,
	pub platform: String,
	pub artifact_type: String,
	pub download_url: String,
	pub device_id: Option<Uuid>,
	pub version_range_pattern: Option<String>,
}

#[tokio::test(flavor = "multi_thread")]
async fn empty_versions_list() {
	commons_tests::server::run(async |_conn, public, _| {
		let response = public.get("/versions").await;
		response.assert_status_ok();
		response.assert_json::<Vec<Version>>(&Vec::new());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn versions_list_with_data() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Initial release', 'published')",
		)
		.await
		.unwrap();

		let response = public.get("/versions").await;
		response.assert_status_ok();
		let versions: Vec<Version> = response.json();
		assert_eq!(versions.len(), 1);
		assert_eq!(versions[0].major, 1);
		assert_eq!(versions[0].minor, 0);
		assert_eq!(versions[0].patch, 0);
		assert_eq!(versions[0].changelog, "Initial release");
		assert_eq!(versions[0].status, VersionStatus::Published);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn releases_rss_feed() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES
			(1, 0, 0, 'Initial release', 'published'),
			(1, 1, 0, '## Highlights\n\n- New thing', 'published'),
			(1, 2, 0, 'Draft only', 'draft')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/rss").await;
		response.assert_status_ok();
		response.assert_header("content-type", "application/rss+xml; charset=utf-8");

		let body = response.text();
		assert!(body.contains("<rss"), "should be an RSS document: {body}");
		assert!(body.contains("<title>Canopy releases</title>"));
		// Newest first, published only.
		assert!(body.contains("Canopy 1.1.0"));
		assert!(body.contains("Canopy 1.0.0"));
		assert!(!body.contains("Canopy 1.2.0"), "drafts must not appear");
		let first = body.find("Canopy 1.1.0").unwrap();
		let second = body.find("Canopy 1.0.0").unwrap();
		assert!(first < second, "items should be newest-first");
		// Changelog markdown is rendered to HTML in the item body.
		assert!(body.contains("New thing"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn releases_rss_feed_excludes_known_issue_versions() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('11111111-1111-1111-1111-111111111100', 1, 0, 0, 'good', 'published'),
			('11111111-1111-1111-1111-111111111101', 1, 0, 1, 'broken', 'published');
			INSERT INTO version_known_issues (author, description, min_major, min_minor, min_patch)
			VALUES ('admin', 'broken', 1, 0, 1)",
		)
		.await
		.unwrap();

		let response = public.get("/versions/rss").await;
		response.assert_status_ok();
		let body = response.text();
		assert!(body.contains("Canopy 1.0.0"));
		assert!(
			!body.contains("Canopy 1.0.1"),
			"known-issue versions must be hidden"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn view_version_artifacts_html() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/1.0.0").await;
		response.assert_status_ok();
		response.assert_header("content-type", "text/html; charset=utf-8");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn list_version_artifacts_empty() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/1.0.0/artifacts").await;
		response.assert_status_ok();
		response.assert_json::<Vec<Artifact>>(&Vec::new());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn list_version_artifacts_with_data() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES ('11111111-1111-1111-1111-111111111111', 1, 0, 0, 'Test version', 'published');
			INSERT INTO artifacts (version_id, platform, artifact_type, download_url) VALUES
			('11111111-1111-1111-1111-111111111111', 'windows', 'installer', 'https://example.com/installer.exe'),
			('11111111-1111-1111-1111-111111111111', 'macos', 'dmg', 'https://example.com/installer.dmg')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/1.0.0/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 2);

		let windows_artifact = artifacts.iter().find(|a| a.platform == "windows").unwrap();
		assert_eq!(windows_artifact.artifact_type, "installer");
		assert_eq!(windows_artifact.download_url, "https://example.com/installer.exe");

		let macos_artifact = artifacts.iter().find(|a| a.platform == "macos").unwrap();
		assert_eq!(macos_artifact.artifact_type, "dmg");
		assert_eq!(macos_artifact.download_url, "https://example.com/installer.dmg");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn view_mobile_install_page() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/1.0.0/mobile").await;
		response.assert_status_ok();
		response.assert_header("content-type", "text/html; charset=utf-8");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_for_version_empty() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (1, 0, 0, 'Test version', 'published')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/update-for/1.0.0").await;
		println!("{:?}", response.as_bytes());
		response.assert_status_ok();
		response.assert_json::<Vec<Version>>(&Vec::new());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn update_for_version_with_newer() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES
			(1, 0, 0, 'Old version', 'published'),
			(1, 0, 1, 'Patch update', 'published'),
			(1, 1, 0, 'Minor update', 'published')",
		)
		.await
		.unwrap();

		let response = public.get("/versions/update-for/1.0.0").await;
		println!("{:?}", response.as_bytes());
		response.assert_status_ok();
		let updates: Vec<Version> = response.json();
		assert_eq!(updates.len(), 2);

		// Should include newer versions
		assert!(updates.iter().any(|v| v.patch == 1));
		assert!(updates.iter().any(|v| v.minor == 1));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn public_excludes_versions_with_open_known_issues() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('11111111-1111-1111-1111-111111111100', 1, 0, 0, 'good', 'published'),
			('11111111-1111-1111-1111-111111111101', 1, 0, 1, 'broken', 'published');
			INSERT INTO version_known_issues (author, description, min_major, min_minor, min_patch)
			VALUES ('admin', 'broken', 1, 0, 1)",
		)
		.await
		.unwrap();

		// /versions lists only the ready one.
		let response = public.get("/versions").await;
		response.assert_status_ok();
		let versions: Vec<Version> = response.json();
		assert_eq!(versions.len(), 1);
		assert_eq!(versions[0].patch, 0);

		// update-for from 1.0.0 — 1.0.1 is broken, so no offer (would
		// otherwise be the latest-per-minor for 1.0.x).
		let response = public.get("/versions/update-for/1.0.0").await;
		response.assert_status_ok();
		let updates: Vec<Version> = response.json();
		assert_eq!(updates.len(), 0);

		// Range query for 1.0.x falls back to the ready 1.0.0.
		let response = public.get("/versions/1.0.x/artifacts").await;
		response.assert_status_ok();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn public_serves_versions_once_known_issue_is_resolved() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('11111111-1111-1111-1111-111111111101', 1, 0, 1, 'broken', 'published'),
			('11111111-1111-1111-1111-111111111102', 1, 0, 2, 'fix', 'published');
			-- Range covers only 1.0.1, fixed in 1.0.2.
			INSERT INTO version_known_issues (
				author, description, min_major, min_minor, min_patch,
				max_major, max_minor, max_patch,
				resolved_at, resolved_by, resolution_message
			)
			VALUES (
				'admin', 'broken', 1, 0, 1,
				1, 0, 2,
				now(), 'admin', 'fixed'
			)",
		)
		.await
		.unwrap();

		// /versions still excludes 1.0.1 (it's covered by the resolved
		// range) but includes 1.0.2.
		let response = public.get("/versions").await;
		response.assert_status_ok();
		let versions: Vec<Version> = response.json();
		assert_eq!(versions.len(), 1);
		assert_eq!(versions[0].patch, 2);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn version_not_found() {
	commons_tests::server::run(async |_conn, public, _| {
		let response = public.get("/versions/999.999.999").await;
		response.assert_status(StatusCode::NOT_FOUND);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn version_not_found_artifacts() {
	commons_tests::server::run(async |_conn, public, _| {
		let response = public.get("/versions/999.999.999/artifacts").await;
		response.assert_status(StatusCode::NOT_FOUND);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifacts_create_draft_version_if_not_found() {
	use database::versions::Version;

	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, device_id, public, _| {
			let response = public
				.post("/artifacts/999.999.999/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text("https://example.com/installer.exe")
				.await;
			response.assert_status_ok();

			let artifact: Artifact = response.json();
			assert_eq!(artifact.platform, "windows");
			assert_eq!(artifact.artifact_type, "installer");
			assert_eq!(artifact.download_url, "https://example.com/installer.exe");
			assert_eq!(artifact.device_id, Some(device_id));

			// Verify the version was created as a draft
			let version = Version::get_by_version(&mut conn, "999.999.999".parse().unwrap())
				.await
				.unwrap();
			assert_eq!(version.status, VersionStatus::Draft);
			assert_eq!(version.major, 999);
			assert_eq!(version.minor, 999);
			assert_eq!(version.patch, 999);
			assert_eq!(version.changelog, "");
			assert_eq!(version.device_id, Some(device_id));
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn version_create_publishes_draft_if_exists() {
	use database::versions::Version;

	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, device_id, public, _| {
			// First, create a draft version by creating an artifact
			let response = public
				.post("/artifacts/2.0.0/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text("https://example.com/installer.exe")
				.await;
			response.assert_status_ok();

			// Verify it's a draft
			let version = Version::get_by_version(&mut conn, "2.0.0".parse().unwrap())
				.await
				.unwrap();
			assert_eq!(version.status, VersionStatus::Draft);
			assert_eq!(version.changelog, "");
			assert_eq!(version.device_id, Some(device_id));

			// Now create/publish the version with a changelog
			let changelog = "# Version 2.0.0\n\nNew features and improvements";
			let response = public
				.post("/versions/2.0.0")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text(changelog)
				.await;
			response.assert_status_ok();

			let published_version: Version = response.json();
			assert_eq!(published_version.status, VersionStatus::Published);
			assert_eq!(published_version.changelog, changelog);
			assert_eq!(published_version.major, 2);
			assert_eq!(published_version.minor, 0);
			assert_eq!(published_version.patch, 0);
			assert_eq!(published_version.device_id, Some(device_id));

			// Verify the version in the database is now published with the new changelog and device_id
			let db_version = Version::get_by_version(&mut conn, "2.0.0".parse().unwrap())
				.await
				.unwrap();
			assert_eq!(db_version.status, VersionStatus::Published);
			assert_eq!(db_version.changelog, changelog);
			assert_eq!(db_version.device_id, Some(device_id));
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn version_create_duplicate_published_fails() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, _device_id, public, _| {
			// Create a published version
			conn.batch_execute(
				"INSERT INTO versions (major, minor, patch, changelog, status) VALUES (3, 0, 0, 'First changelog', 'published')",
			)
			.await
			.unwrap();

			// Try to create the same version again - should fail
			let changelog = "# Version 3.0.0\n\nDifferent changelog";
			let response = public
				.post("/versions/3.0.0")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text(changelog)
				.await;

			// Should fail with a database constraint error (500 or 409)
			assert!(
				response.status_code() == StatusCode::INTERNAL_SERVER_ERROR
				|| response.status_code() == StatusCode::CONFLICT,
				"Expected 500 or 409, got {}",
				response.status_code()
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn version_range_latest_matching() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (major, minor, patch, changelog, status) VALUES
			(1, 0, 0, 'Version 1.0.0', 'published'),
			(1, 0, 1, 'Version 1.0.1', 'published'),
			(1, 0, 2, 'Version 1.0.2', 'published')",
		)
		.await
		.unwrap();

		// Should return latest patch version in 1.0.x range
		let response = public.get("/versions/1.0/artifacts").await;
		response.assert_status_ok();

		// Should also work with mobile page
		let response = public.get("/versions/1.0/mobile").await;
		response.assert_status_ok();
		response.assert_header("content-type", "text/html; charset=utf-8");
	})
	.await
}

/// A range whose floor sits in an earlier minor line must still resolve to the
/// newer minor: semver orders lexicographically, so `1.1.0` satisfies
/// `>=1.0.5` even though its patch is lower.
#[tokio::test(flavor = "multi_thread")]
async fn version_range_resolves_across_minor_lines() {
	commons_tests::server::run(async |mut conn, public, _| {
		let older = "55555555-5555-5555-5555-555555555555";
		let newer = "66666666-6666-6666-6666-666666666666";
		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('{older}', 1, 0, 5, 'Version 1.0.5', 'published'),
			('{newer}', 1, 1, 0, 'Version 1.1.0', 'published');
			INSERT INTO artifacts (version_id, platform, artifact_type, download_url) VALUES
			('{older}', 'windows', 'installer', 'https://example.com/1.0.5.exe'),
			('{newer}', 'windows', 'installer', 'https://example.com/1.1.0.exe')",
		))
		.await
		.unwrap();

		let response = public.get("/versions/%3E%3D1.0.5/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(
			artifacts
				.iter()
				.map(|a| a.download_url.as_str())
				.collect::<Vec<_>>(),
			vec!["https://example.com/1.1.0.exe"],
			"the range must resolve to 1.1.0, not the floor it was anchored on",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_download_proxy() {
	commons_tests::server::run(async |mut conn, public, _| {
		let version_id = "22222222-2222-2222-2222-222222222222";
		let artifact_id = "33333333-3333-3333-3333-333333333333";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES ('{version_id}', 1, 2, 3, 'Test version', 'published');
			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url) VALUES
			('{artifact_id}', '{version_id}', 'windows', 'installer', 'https://example.com/installer.exe')",
		))
		.await
		.unwrap();

		// An unreadable artifact id is a client mistake, the same as an
		// unreadable version in the path, so it answers 400 rather than
		// polluting 5xx monitoring with input a caller controls.
		let response = public
			.get("/versions/1.2.3/artifacts/not-a-uuid/download")
			.await;
		response.assert_status(StatusCode::BAD_REQUEST);

		// Nonexistent artifact. 404 rather than an error, because an artifact
		// scoped to a group the caller is not offered has to be answered the
		// same way, and that answer must not be distinguishable.
		// spec: ART#who-is-offered-a-group-scoped-artifact
		let response = public
			.get("/versions/1.2.3/artifacts/44444444-4444-4444-4444-444444444444/download")
			.await;
		response.assert_status(StatusCode::NOT_FOUND);

		// Nonexistent version
		let response = public
			.get("/versions/9.9.9/artifacts/44444444-4444-4444-4444-444444444444/download")
			.await;
		response.assert_status(StatusCode::NOT_FOUND);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_download_proxy_with_mock_server() {
	commons_tests::server::run(async |mut conn, public, _| {
		let version_id = "55555555-5555-5555-5555-555555555555";
		let artifact_id = "66666666-6666-6666-6666-666666666666";
		let test_content = b"test artifact content";

		// Start a simple HTTP server to serve test content
		let server = axum::Router::new()
			.route("/test", axum::routing::get(|| async { test_content.to_vec() }))
			.into_make_service();

		let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
			.await
			.unwrap();
		let addr = listener.local_addr().unwrap();
		let server_url = format!("http://{}/test", addr);

		// Spawn the test server in background
		tokio::spawn(async move {
			axum::serve(listener, server).await.ok();
		});

		// Give the server a moment to start
		tokio::time::sleep(tokio::time::Duration::from_millis(10)).await;

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES ('{version_id}', 2, 3, 4, 'Test version', 'published');
			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url) VALUES
			('{artifact_id}', '{version_id}', 'windows', 'installer', '{server_url}')",
		))
		.await
		.unwrap();

		// Test downloading the artifact
		let response = public
			.get(&format!("/versions/2.3.4/artifacts/{artifact_id}/download"))
			.await;
		response.assert_status_ok();

		// Verify we got the content
		let text = response.text();
		assert_eq!(text.as_bytes(), test_content);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_range_matching() {
	commons_tests::server::run(async |mut conn, public, _| {
		let version_id_100 = "77777777-7777-7777-7777-777777777777";
		let version_id_101 = "88888888-8888-8888-8888-888888888888";
		let version_id_110 = "99999999-9999-9999-9999-999999999999";
		let range_artifact_id = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";
		let exact_artifact_id = "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES 
			('{version_id_100}', 1, 0, 0, 'v1.0.0', 'published'),
			('{version_id_101}', 1, 0, 1, 'v1.0.1', 'published'),
			('{version_id_110}', 1, 1, 0, 'v1.1.0', 'published');
			
			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url, version_range_pattern) VALUES
			('{range_artifact_id}', NULL, 'windows', 'installer', 'https://example.com/range.exe', '1.0.x'),
			('{exact_artifact_id}', '{version_id_101}', 'linux', 'tarball', 'https://example.com/exact.tar.gz', NULL)",
		))
		.await
		.unwrap();

		// 1.0.0 should have the range artifact
		let response = public.get("/versions/1.0.0/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 1);
		assert_eq!(artifacts[0].id.to_string(), range_artifact_id.to_lowercase());

		// 1.0.1 should have both range and exact artifacts
		let response = public.get("/versions/1.0.1/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 2);
		// Should be ordered: installer first, then tarball (both within their artifact types)
		let installer = artifacts.iter().find(|a| a.artifact_type == "installer").unwrap();
		let tarball = artifacts.iter().find(|a| a.artifact_type == "tarball").unwrap();
		assert_eq!(installer.id.to_string(), range_artifact_id.to_lowercase());
		assert_eq!(tarball.id.to_string(), exact_artifact_id.to_lowercase());

		// 1.1.0 should have no artifacts (range is 1.0.x)
		let response = public.get("/versions/1.1.0/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 0);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_specificity_conflict_resolution() {
	commons_tests::server::run(async |mut conn, public, _| {
		let version_id_101 = "cccccccc-cccc-cccc-cccc-cccccccccccc";
		let exact_artifact_id = "dddddddd-dddd-dddd-dddd-dddddddddddd";
		let broad_range_id = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeeee";
		let narrow_range_id = "ffffffff-ffff-ffff-ffff-ffffffffffff";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES 
			('{version_id_101}', 1, 0, 1, 'v1.0.1', 'published');
			
			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url, version_range_pattern) VALUES
			('{exact_artifact_id}', '{version_id_101}', 'windows', 'installer', 'https://example.com/exact.exe', NULL),
			('{broad_range_id}', NULL, 'windows', 'installer', 'https://example.com/1x.exe', '1.x'),
			('{narrow_range_id}', NULL, 'windows', 'installer', 'https://example.com/10x.exe', '1.0.x')",
		))
		.await
		.unwrap();

		// 1.0.1 has three artifacts for same platform+type
		// Should return only the exact match
		let response = public.get("/versions/1.0.1/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 1);
		assert_eq!(artifacts[0].id.to_string(), exact_artifact_id.to_lowercase());
	})
	.await
}

/// Deduplication has to be by key, not by adjacency. The SQL `ORDER BY` groups
/// each type+platform together, but the specificity sort then hoists every
/// exact artifact ahead of every range one. Any other exact artifact sorting
/// after the duplicated key therefore lands between its exact and range rows,
/// and they stop being neighbours.
#[tokio::test(flavor = "multi_thread")]
async fn artifact_specificity_dedups_non_adjacent_duplicates() {
	commons_tests::server::run(async |mut conn, public, _| {
		let version_id = "cccccccc-cccc-cccc-cccc-ccccccccccc1";
		let exact_windows = "dddddddd-dddd-dddd-dddd-ddddddddddd1";
		let range_windows = "eeeeeeee-eeee-eeee-eeee-eeeeeeeeeee1";
		let exact_android = "ffffffff-ffff-ffff-ffff-fffffffffff1";

		// installer/windows is duplicated; mobile/android sorts after it, so the
		// specificity sort yields [installer-exact, mobile-exact, installer-range]
		// and the two installer rows are no longer adjacent.
		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('{version_id}', 1, 0, 1, 'v1.0.1', 'published');

			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url, version_range_pattern) VALUES
			('{exact_windows}', '{version_id}', 'windows', 'installer', 'https://example.com/exact.exe', NULL),
			('{range_windows}', NULL, 'windows', 'installer', 'https://example.com/1x.exe', '1.x'),
			('{exact_android}', '{version_id}', 'android', 'mobile', 'https://example.com/exact.apk', NULL)",
		))
		.await
		.unwrap();

		let response = public.get("/versions/1.0.1/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		let installers: Vec<&Artifact> = artifacts
			.iter()
			.filter(|a| a.platform == "windows" && a.artifact_type == "installer")
			.collect();
		assert_eq!(
			installers.len(),
			1,
			"one entry per type+platform; got conflicting download URLs: {:?}",
			installers.iter().map(|a| &a.download_url).collect::<Vec<_>>(),
		);
		assert_eq!(installers[0].id.to_string(), exact_windows.to_lowercase());
		assert_eq!(artifacts.len(), 2, "windows installer + android mobile");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_range_specificity_conflict_resolution() {
	commons_tests::server::run(async |mut conn, public, _| {
		let version_id_100 = "11111111-1111-1111-1111-111111111111";
		let broad_range_id = "22222222-2222-2222-2222-222222222222";
		let narrow_range_id = "33333333-3333-3333-3333-333333333333";

		conn.batch_execute(&format!(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES 
			('{version_id_100}', 1, 0, 0, 'v1.0.0', 'published');
			
			INSERT INTO artifacts (id, version_id, platform, artifact_type, download_url, version_range_pattern) VALUES
			('{broad_range_id}', NULL, 'windows', 'installer', 'https://example.com/1x.exe', '1.x'),
			('{narrow_range_id}', NULL, 'windows', 'installer', 'https://example.com/10x.exe', '1.0.x')",
		))
		.await
		.unwrap();

		// 1.0.0 has two range artifacts for same platform+type
		// Should return only the more specific one (1.0.x)
		let response = public.get("/versions/1.0.0/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 1);
		assert_eq!(artifacts[0].id.to_string(), narrow_range_id.to_lowercase());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_multiple_ranges_no_exact_match() {
	commons_tests::server::run(async |mut conn, public, _| {
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

	// 2.44.5 has two range artifacts for same platform+type
		// Both match: 2.44.x matches and ^2.44.2 matches (since 2.44.5 >= 2.44.2)
		// These ranges are incomparable (neither allows_all the other):
		// - 2.44.x = >=2.44.0 <2.45.0
		// - ^2.44.2 = >=2.44.2 <3.0.0
		// When ranges are incomparable, pattern specificity is used:
		// - ^2.44.2 (caret) is ranked as more specific than 2.44.x (wildcard)
		let response = public.get("/versions/2.44.5/artifacts").await;
		response.assert_status_ok();
		let artifacts: Vec<Artifact> = response.json();
		assert_eq!(artifacts.len(), 1, "Should have exactly 1 artifact");
		// Caret ranges rank higher specificity than wildcard ranges
		assert_eq!(artifacts[0].id.to_string(), narrower_range_id.to_lowercase(), "Should be the caret range (^2.44.2) due to pattern specificity");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_create_range_authenticated() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |_conn, cert, _device_id, public, _| {
			let response = public
				.post("/artifacts/1.0.x/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text("https://example.com/installer-1.0.x.exe")
				.await;

			response.assert_status_ok();

			let artifact: Artifact = response.json();
			assert_eq!(artifact.artifact_type, "installer");
			assert_eq!(artifact.platform, "windows");
			assert_eq!(artifact.version_id, None);
			assert_eq!(artifact.version_range_pattern, Some("1.0.x".to_string()));
			assert_eq!(
				artifact.download_url,
				"https://example.com/installer-1.0.x.exe"
			);
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_create_range_invalid_pattern() {
	commons_tests::server::run_with_device_auth(
		"releaser",
		async |_conn, cert, _device_id, public, _| {
			let response = public
				.post("/artifacts/invalid@@range/installer/windows")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text("https://example.com/installer.exe")
				.await;

			response.assert_status_not_ok();
		},
	)
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn artifact_create_range_no_auth() {
	commons_tests::server::run(async |_conn, public, _| {
		let response = public
			.post("/artifacts/1.0.x/installer/windows")
			.text("https://example.com/installer.exe")
			.await;

		response.assert_status_not_ok();
	})
	.await
}

/// A malformed version in a URL path is a client mistake, not a server
/// fault: `AppError::VersionParse` fell through to the catch-all 500 arm,
/// so bad input polluted 5xx monitoring and told clients to retry something
/// that can never succeed.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_version_in_the_path_is_a_400() {
	commons_tests::server::run(async |_conn, public, _| {
		for path in [
			"/versions/update-for/not-a-version",
			"/versions/not-a-version",
		] {
			let response = public.get(path).await;
			assert_eq!(
				response.status_code(),
				StatusCode::BAD_REQUEST,
				"{path} should be a 400",
			);
		}
	})
	.await
}
/// The "newer version available" banner on a version page filtered only on
/// `Published`, while the page's own version comes through
/// `latest_matching_ready`, which also excludes anything a known issue
/// covers. So the banner could recommend — and link to — a version hidden
/// from every listing, whose own page 404s.
#[tokio::test(flavor = "multi_thread")]
async fn version_page_banner_does_not_recommend_a_known_issue_version() {
	commons_tests::server::run(async |mut conn, public, _| {
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, changelog, status) VALUES
			('22222222-2222-2222-2222-222222222200', 2, 0, 0, 'old', 'published'),
			('22222222-2222-2222-2222-222222222201', 2, 0, 1, 'fine', 'published'),
			('22222222-2222-2222-2222-222222222202', 2, 0, 2, 'broken', 'published');
			INSERT INTO version_known_issues (author, description, min_major, min_minor, min_patch)
			VALUES ('admin', 'broken', 2, 0, 2)",
		)
		.await
		.unwrap();

		// Sanity: the covered version really is hidden from the public site.
		public
			.get("/versions/2.0.2")
			.await
			.assert_status_not_found();

		let body = public.get("/versions/2.0.0").await.text();
		assert!(
			!body.contains("2.0.2"),
			"the banner must not point at a version a known issue covers: {body}",
		);
		assert!(
			body.contains("2.0.1"),
			"it should point at the newest ready patch instead: {body}",
		);
	})
	.await
}

/// The changelog cap was written `1024 * 1024 * 1024` next to a comment
/// saying "up to a MiB" — a GiB, so the intended limit was never enforced.
#[tokio::test(flavor = "multi_thread")]
async fn changelog_is_capped_at_a_mebibyte() {
	use database::versions::Version;

	const MIB: usize = 1024 * 1024;

	commons_tests::server::run_with_device_auth(
		"releaser",
		async |mut conn, cert, _device_id, public, _| {
			// Comfortably over the cap, all single-byte characters so the
			// byte count and the character count agree.
			let oversized = "x".repeat(MIB + 4096);
			let response = public
				.post("/versions/3.1.4")
				.add_header("x-forwarded-client-cert", &format!("Cert={}", cert))
				.text(&oversized)
				.await;
			response.assert_status_ok();

			let version = Version::get_by_version(&mut conn, "3.1.4".parse().unwrap())
				.await
				.unwrap();
			assert_eq!(
				version.changelog.len(),
				MIB,
				"the changelog should be truncated to a MiB, not stored whole",
			);
		},
	)
	.await
}
