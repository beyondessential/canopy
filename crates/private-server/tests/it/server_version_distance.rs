use commons_tests::diesel_async::SimpleAsyncConnection;
use commons_types::version::VersionStatus;
use database::{statuses::Status, versions::Version};

#[tokio::test(flavor = "multi_thread")]
async fn version_distance_calculation_up_to_date() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		// Create a server
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Test Application', 'https://test.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111')"
		)
		.await
		.unwrap();

		// Create published versions
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('22222222-2222-2222-2222-222222222222', 2, 10, 0, 'published', 'Latest version'),
			('33333333-3333-3333-3333-333333333333', 2, 9, 5, 'published', 'Previous version')"
		)
		.await
		.unwrap();

		// Create status with current version
		conn.batch_execute(
			"INSERT INTO statuses (id, server_id, version, extra) VALUES
			('44444444-4444-4444-4444-444444444444', '11111111-1111-1111-1111-111111111111', '2.10.0', '{}')"
		)
		.await
		.unwrap();

		let server_id = "11111111-1111-1111-1111-111111111111".parse().unwrap();
		let status = Status::latest_for_server(&mut conn, server_id)
			.await
			.unwrap()
			.unwrap();

		// Verify status has correct version
		assert_eq!(status.version.as_ref().unwrap().to_string(), "2.10.0");

		// Get all published versions and compute distance
		let all_versions = Version::get_all(&mut conn).await.unwrap();
		let published_versions: Vec<_> = all_versions
			.into_iter()
			.filter(|v| v.status == VersionStatus::Published)
			.collect();

		assert!(!published_versions.is_empty());

		let latest = published_versions.first().unwrap();
		let latest_semver = latest.as_semver();
		let current_semver = &status.version.unwrap().0;

		let distance = if latest_semver.major != current_semver.major {
			1000 + (latest_semver.minor as i32 - current_semver.minor as i32).abs()
		} else {
			(latest_semver.minor as i32 - current_semver.minor as i32).abs()
		};

		assert_eq!(distance, 0);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn version_distance_calculation_minor_behind() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		// Create a server
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Test Application', 'https://test.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111')"
		)
		.await
		.unwrap();

		// Create published versions - latest is 2.12.0
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('22222222-2222-2222-2222-222222222222', 2, 12, 0, 'published', 'Latest version'),
			('33333333-3333-3333-3333-333333333333', 2, 10, 0, 'published', 'Previous version')"
		)
		.await
		.unwrap();

		// Application is on 2.10.0 (2 minors behind)
		conn.batch_execute(
			"INSERT INTO statuses (id, server_id, version, extra) VALUES
			('44444444-4444-4444-4444-444444444444', '11111111-1111-1111-1111-111111111111', '2.10.0', '{}')"
		)
		.await
		.unwrap();

		let server_id = "11111111-1111-1111-1111-111111111111".parse().unwrap();
		let status = Status::latest_for_server(&mut conn, server_id)
			.await
			.unwrap()
			.unwrap();

		let all_versions = Version::get_all(&mut conn).await.unwrap();
		let published_versions: Vec<_> = all_versions
			.into_iter()
			.filter(|v| v.status == VersionStatus::Published)
			.collect();

		let latest = published_versions.first().unwrap();
		let latest_semver = latest.as_semver();
		let current_semver = &status.version.unwrap().0;

		let distance = if latest_semver.major != current_semver.major {
			1000 + (latest_semver.minor as i32 - current_semver.minor as i32).abs()
		} else {
			(latest_semver.minor as i32 - current_semver.minor as i32).abs()
		};

		assert_eq!(distance, 2);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn version_distance_calculation_major_behind() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		// Create a server
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Test Application', 'https://test.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111')"
		)
		.await
		.unwrap();

		// Create published versions - latest is 3.2.0
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('22222222-2222-2222-2222-222222222222', 3, 2, 0, 'published', 'Latest version'),
			('33333333-3333-3333-3333-333333333333', 2, 10, 0, 'published', 'Old major version')"
		)
		.await
		.unwrap();

		// Application is on 2.10.0 (different major version)
		conn.batch_execute(
			"INSERT INTO statuses (id, server_id, version, extra) VALUES
			('44444444-4444-4444-4444-444444444444', '11111111-1111-1111-1111-111111111111', '2.10.0', '{}')"
		)
		.await
		.unwrap();

		let server_id = "11111111-1111-1111-1111-111111111111".parse().unwrap();
		let status = Status::latest_for_server(&mut conn, server_id)
			.await
			.unwrap()
			.unwrap();

		let all_versions = Version::get_all(&mut conn).await.unwrap();
		let published_versions: Vec<_> = all_versions
			.into_iter()
			.filter(|v| v.status == VersionStatus::Published)
			.collect();

		let latest = published_versions.first().unwrap();
		let latest_semver = latest.as_semver();
		let current_semver = &status.version.unwrap().0;

		let distance = if latest_semver.major != current_semver.major {
			1000 + (latest_semver.minor as i32 - current_semver.minor as i32).abs()
		} else {
			(latest_semver.minor as i32 - current_semver.minor as i32).abs()
		};

		// Different major version adds 1000, plus minor difference of 8 (abs(2-10))
		assert_eq!(distance, 1008);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn version_distance_none_when_no_published_versions() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		// Create a server
		conn.batch_execute(
			"WITH m AS (INSERT INTO machines (name, id) VALUES ('box', '11111111-1111-1111-1111-111111111111') RETURNING id) INSERT INTO applications (id, name, host, rank, type, machine_id) VALUES
			('11111111-1111-1111-1111-111111111111', 'Test Application', 'https://test.example.com', 'production', 'tamanu-central', '11111111-1111-1111-1111-111111111111')"
		)
		.await
		.unwrap();

		// Create only unpublished versions
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('22222222-2222-2222-2222-222222222222', 2, 10, 0, 'draft', 'Unpublished version')"
		)
		.await
		.unwrap();

		// Application has a version
		conn.batch_execute(
			"INSERT INTO statuses (id, server_id, version, extra) VALUES
			('44444444-4444-4444-4444-444444444444', '11111111-1111-1111-1111-111111111111', '2.9.0', '{}')"
		)
		.await
		.unwrap();

		let all_versions = Version::get_all(&mut conn).await.unwrap();
		let published_versions: Vec<_> = all_versions
			.into_iter()
			.filter(|v| v.status == VersionStatus::Published)
			.collect();

		// Should have no published versions
		assert!(published_versions.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn version_distance_orders_versions_correctly() {
	commons_tests::db::TestDb::run(|mut conn, _url| async move {
		// Create multiple published versions out of order
		conn.batch_execute(
			"INSERT INTO versions (id, major, minor, patch, status, changelog) VALUES
			('11111111-1111-1111-1111-111111111111', 2, 8, 0, 'published', 'Old version'),
			('22222222-2222-2222-2222-222222222222', 2, 12, 0, 'published', 'Latest version'),
			('33333333-3333-3333-3333-333333333333', 2, 10, 0, 'published', 'Middle version')",
		)
		.await
		.unwrap();

		let all_versions = Version::get_all(&mut conn).await.unwrap();
		let published_versions: Vec<_> = all_versions
			.into_iter()
			.filter(|v| v.status == VersionStatus::Published)
			.collect();

		// First should be the latest (2.12.0)
		let latest = published_versions.first().unwrap();
		assert_eq!(latest.major, 2);
		assert_eq!(latest.minor, 12);
		assert_eq!(latest.patch, 0);
	})
	.await;
}
