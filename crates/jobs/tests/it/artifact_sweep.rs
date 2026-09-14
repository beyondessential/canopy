//! The sweep drops the bytes a failed registration left and nothing else.
//!
//! spec: ART

use commons_servers::artifact_store::ArtifactStore;
use commons_tests::db::TestDb;
use commons_tests::diesel_async::SimpleAsyncConnection;
use database::artifacts::{Artifact, NewArtifact, digest_of};
use jiff::Timestamp;
use uuid::Uuid;

const VERSION: &str = "11111111-1111-1111-1111-111111111111";
const GROUP: &str = "aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa";

async fn seed(conn: &mut database::diesel_async::AsyncPgConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO versions (id, major, minor, patch, changelog, status)
		 VALUES ('{VERSION}', 2, 60, 0, '', 'published');
		 INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka')",
	))
	.await
	.expect("seed");
}

/// Register a held artifact and put its bytes where they rest, as an upload does.
async fn register(
	conn: &mut database::diesel_async::AsyncPgConnection,
	store: &ArtifactStore,
	bytes: &[u8],
) -> Uuid {
	let artifact = Artifact::register(
		conn,
		NewArtifact {
			id: None,
			version_id: Some(VERSION.parse().unwrap()),
			artifact_type: "reporting-schema".into(),
			platform: "any".into(),
			download_url: None,
			device_id: None,
			version_range_pattern: None,
			group_id: Some(GROUP.parse().unwrap()),
			content_type: Some("application/sql".into()),
			digest: Some(digest_of(bytes)),
			run_id: None,
		},
	)
	.await
	.expect("register");

	store.put(artifact.id, bytes.to_vec()).await.expect("store");
	artifact.id
}

/// An artifact still registered keeps its bytes however old they are. A group
/// can sit on one version for a year without a rebuild, so age alone is never a
/// reason to drop what a row still names.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn a_registered_artifact_keeps_its_bytes_however_old() {
	TestDb::run(|mut conn, _url| async move {
		seed(&mut conn).await;
		let store = ArtifactStore::memory();
		let artifact = register(&mut conn, &store, b"kamaka schema").await;

		let ancient = Timestamp::now() - std::time::Duration::from_secs(400 * 24 * 3600);
		store.backdate(artifact, ancient);

		jobs::artifact_sweep::tick(&mut conn, &store, Timestamp::now()).await;

		assert_eq!(
			store.get(artifact).await.unwrap().as_deref(),
			Some(&b"kamaka schema"[..]),
			"a registered artifact was swept"
		);
	})
	.await;
}

/// Bytes no artifact reaches are what the sweep is for: a registration whose row
/// write never landed leaves them, and nothing else can find them.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn bytes_no_registration_reaches_are_swept() {
	TestDb::run(|mut conn, _url| async move {
		seed(&mut conn).await;
		let store = ArtifactStore::memory();
		let kept = register(&mut conn, &store, b"kamaka schema").await;

		let orphan = Uuid::new_v4();
		store
			.put(orphan, b"nothing names these".to_vec())
			.await
			.unwrap();
		store.backdate(orphan, Timestamp::now() - jobs::artifact_sweep::GRACE);

		jobs::artifact_sweep::tick(&mut conn, &store, Timestamp::now()).await;

		assert!(
			store.get(orphan).await.unwrap().is_none(),
			"the orphan survived"
		);
		assert!(
			store.get(kept).await.unwrap().is_some(),
			"the registered one went"
		);
	})
	.await;
}

/// The bytes go in before the row that names them, so an object younger than the
/// grace has a registration possibly still in flight behind it. Sweeping on age
/// alone would race an upload and drop an artifact that registered successfully.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn bytes_still_within_the_grace_are_left_alone() {
	TestDb::run(|mut conn, _url| async move {
		seed(&mut conn).await;
		let store = ArtifactStore::memory();

		// Stored, with no row yet: exactly the window an upload passes through.
		let in_flight = Uuid::new_v4();
		store
			.put(in_flight, b"mid-registration".to_vec())
			.await
			.unwrap();

		jobs::artifact_sweep::tick(&mut conn, &store, Timestamp::now()).await;

		assert!(
			store.get(in_flight).await.unwrap().is_some(),
			"an upload still in flight was swept out from under itself"
		);
	})
	.await;
}
