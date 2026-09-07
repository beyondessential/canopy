//! DB-layer tests for group-scoped artifact resolution (`database::artifacts`).
//!
//! spec: ART

use commons_tests::db::TestDb;
use database::{
	artifacts::{Artifact, NewArtifact, Scope, digest_of},
	diesel_async::AsyncPgConnection,
};
use diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

async fn seed_version(conn: &mut AsyncPgConnection, major: i32, minor: i32, patch: i32) -> Uuid {
	conn.batch_execute(&format!(
		"INSERT INTO versions (major, minor, patch, changelog, status) \
		 VALUES ({major}, {minor}, {patch}, '', 'published')"
	))
	.await
	.expect("seed version");

	let version = database::versions::Version::get_by_version(
		conn,
		commons_types::version::VersionStr(node_semver::Version {
			major: major as u64,
			minor: minor as u64,
			patch: patch as u64,
			build: vec![],
			pre_release: vec![],
		}),
	)
	.await
	.expect("read back version");
	version.id
}

async fn seed_group(conn: &mut AsyncPgConnection, name: &str) -> Uuid {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (name) VALUES ('{name}')"
	))
	.await
	.expect("seed group");

	let groups = database::server_groups::ServerGroup::list_all(conn)
		.await
		.expect("list groups");
	groups
		.into_iter()
		.find(|g| g.name == name)
		.expect("group is there")
		.id
}

fn unscoped(version_id: Uuid, artifact_type: &str, url: &str) -> NewArtifact {
	NewArtifact {
		version_id: Some(version_id),
		artifact_type: artifact_type.to_owned(),
		platform: "any".to_owned(),
		download_url: Some(url.to_owned()),
		device_id: None,
		version_range_pattern: None,
		group_id: None,
		content: None,
		content_type: None,
		digest: None,
		run_id: None,
	}
}

fn held(version_id: Uuid, artifact_type: &str, group: Uuid, bytes: &[u8]) -> NewArtifact {
	NewArtifact {
		version_id: Some(version_id),
		artifact_type: artifact_type.to_owned(),
		platform: "any".to_owned(),
		download_url: None,
		device_id: None,
		version_range_pattern: None,
		group_id: Some(group),
		content: Some(bytes.to_vec()),
		content_type: Some("application/sql".to_owned()),
		digest: Some(digest_of(bytes)),
		run_id: None,
	}
}

/// A group-scoped artifact and an unscoped one of the same type and platform
/// are both recorded, each group is offered the one for it, and no caller is
/// offered both.
#[tokio::test(flavor = "multi_thread")]
async fn each_group_is_offered_its_own_and_never_both() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;
		let other = seed_group(&mut conn, "drifting").await;

		Artifact::register(
			&mut conn,
			unscoped(version, "reporting-schema", "https://x/y"),
		)
		.await
		.expect("register unscoped");
		Artifact::register(
			&mut conn,
			held(version, "reporting-schema", theirs, b"theirs"),
		)
		.await
		.expect("register held");

		let offered = Artifact::get_for_version(&mut conn, version, Scope::Group(theirs))
			.await
			.expect("resolve for the owning group");
		assert_eq!(offered.len(), 1, "never offered both");
		assert_eq!(offered[0].group_id, Some(theirs));

		// Another group's read reaches the unscoped one, not the first group's.
		let offered = Artifact::get_for_version(&mut conn, version, Scope::Group(other))
			.await
			.expect("resolve for another group");
		assert_eq!(offered.len(), 1);
		assert_eq!(offered[0].group_id, None);

		// A read carrying no identity is answered with the unscoped set alone.
		let offered = Artifact::get_for_version(&mut conn, version, Scope::Unscoped)
			.await
			.expect("resolve anonymously");
		assert_eq!(offered.len(), 1);
		assert_eq!(offered[0].group_id, None);

		// An operator sees what resolution passed over.
		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert_eq!(all.len(), 2);
	})
	.await;
}

/// A group-scoped artifact is more specific than an unscoped one, so it wins
/// even where the unscoped one is exact and it is only a range match.
#[tokio::test(flavor = "multi_thread")]
async fn group_scope_outranks_an_exact_unscoped_artifact() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;

		Artifact::register(&mut conn, unscoped(version, "installer", "https://x/exact"))
			.await
			.expect("register exact unscoped");

		let mut ranged = held(version, "installer", theirs, b"ranged");
		ranged.version_id = None;
		ranged.version_range_pattern = Some("2.60.x".to_owned());
		Artifact::register(&mut conn, ranged)
			.await
			.expect("register ranged held");

		let offered = Artifact::get_for_version(&mut conn, version, Scope::Group(theirs))
			.await
			.expect("resolve");
		assert_eq!(offered.len(), 1);
		assert_eq!(
			offered[0].group_id,
			Some(theirs),
			"the group's range artifact beats an exact artifact for everyone"
		);
	})
	.await;
}

/// A registration replaces whatever is already registered for the same version,
/// type, platform and group, and the bytes it replaces do not survive.
#[tokio::test(flavor = "multi_thread")]
async fn registering_again_replaces_the_bytes_it_held() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;

		let first = Artifact::register(
			&mut conn,
			held(version, "reporting-schema", theirs, b"first build"),
		)
		.await
		.expect("first registration");

		let second = Artifact::register(
			&mut conn,
			held(version, "reporting-schema", theirs, b"second build"),
		)
		.await
		.expect("second registration");

		assert_eq!(first.id, second.id, "replaced in place, not duplicated");

		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert_eq!(all.len(), 1, "a caller is never offered two of a kind");

		let content = Artifact::content_for(&mut conn, second.id)
			.await
			.expect("read content")
			.expect("bytes are held");
		assert_eq!(content.bytes, b"second build");
		assert_eq!(content.digest, digest_of(b"second build"));
	})
	.await;
}

/// The same type and platform can be registered for one group and for another
/// without colliding, which the old version-only unique constraint could not
/// express.
#[tokio::test(flavor = "multi_thread")]
async fn two_groups_hold_their_own_of_the_same_kind() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let one = seed_group(&mut conn, "kamaka").await;
		let two = seed_group(&mut conn, "drifting").await;

		Artifact::register(&mut conn, held(version, "reporting-schema", one, b"one"))
			.await
			.expect("first group");
		Artifact::register(&mut conn, held(version, "reporting-schema", two, b"two"))
			.await
			.expect("second group");

		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert_eq!(all.len(), 2);
	})
	.await;
}

/// Each group is served its own, so the operator view marks both as offered.
/// Deduplicating once across the fleet picks one and hides the other, which is
/// the opposite of what an operator has to be able to see.
#[tokio::test(flavor = "multi_thread")]
async fn the_operator_view_marks_every_group_s_own_as_offered() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let one = seed_group(&mut conn, "kamaka").await;
		let two = seed_group(&mut conn, "drifting").await;

		Artifact::register(&mut conn, unscoped(version, "installer", "https://x/i.exe"))
			.await
			.expect("unscoped");
		Artifact::register(&mut conn, held(version, "reporting-schema", one, b"one"))
			.await
			.expect("first group");
		Artifact::register(&mut conn, held(version, "reporting-schema", two, b"two"))
			.await
			.expect("second group");

		let all =
			Artifact::get_for_version_all_matches_with_metadata(&mut conn, version, Scope::Fleet)
				.await
				.expect("operator view");

		assert_eq!(all.len(), 3);
		for (artifact, _, _, offered) in all {
			assert!(
				offered,
				"{} for {:?} is served to someone",
				artifact.artifact_type, artifact.group_id
			);
		}
	})
	.await;
}

/// The digest is a prefixed sha256 of the bytes. Pinned against a known answer
/// rather than against `digest_of` of the same input, which would hold just as
/// well if the function returned a constant.
#[test]
fn the_digest_is_a_prefixed_sha256() {
	assert_eq!(
		digest_of(b""),
		"sha256:e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
	);
}

/// A range artifact registered twice replaces itself. Before the identity
/// index this could not hold: `version_id` is NULL for every range artifact,
/// and the default treatment of NULL made each row distinct from the last.
#[tokio::test(flavor = "multi_thread")]
async fn a_range_artifact_replaces_itself() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		for url in ["https://x/first", "https://x/second"] {
			let mut ranged = unscoped(version, "installer", url);
			ranged.version_id = None;
			ranged.version_range_pattern = Some("2.60.x".to_owned());
			Artifact::register(&mut conn, ranged)
				.await
				.expect("register range artifact");
		}

		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert_eq!(all.len(), 1, "the second registration replaced the first");
		assert_eq!(all[0].download_url.as_deref(), Some("https://x/second"));
	})
	.await;
}

/// An artifact Canopy holds has no location, so an attempt to give it one is
/// refused rather than reaching the shape constraint as a database error.
#[tokio::test(flavor = "multi_thread")]
async fn a_held_artifact_cannot_be_given_a_url() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;

		let artifact = Artifact::register(
			&mut conn,
			held(version, "reporting-schema", theirs, b"schema"),
		)
		.await
		.expect("register");

		let refused = Artifact::update(
			&mut conn,
			artifact.id,
			"reporting-schema".to_owned(),
			"any".to_owned(),
			Some("https://example.com/elsewhere.sql".to_owned()),
		)
		.await;
		assert!(refused.is_err(), "a held artifact takes no location");

		// Renaming it without offering a location is still fine.
		Artifact::update(
			&mut conn,
			artifact.id,
			"reporting-assets".to_owned(),
			"any".to_owned(),
			None,
		)
		.await
		.expect("rename is allowed");
	})
	.await;
}

/// A range artifact, for the specificity rules that need one.
fn ranged(artifact_type: &str, pattern: &str, url: &str) -> NewArtifact {
	NewArtifact {
		version_id: None,
		artifact_type: artifact_type.to_owned(),
		platform: "any".to_owned(),
		download_url: Some(url.to_owned()),
		device_id: None,
		version_range_pattern: Some(pattern.to_owned()),
		group_id: None,
		content: None,
		content_type: None,
		digest: None,
		run_id: None,
	}
}

/// Register a pair for one type both ways round and return which id won each
/// time. Resolution has to reorder rather than take what the query happened to
/// return first, so a rule only counts as pinned when it holds either way.
async fn winner_either_way(
	conn: &mut AsyncPgConnection,
	version: Uuid,
	winner: impl Fn(&str) -> NewArtifact,
	loser: impl Fn(&str) -> NewArtifact,
) -> (Uuid, Uuid, Uuid, Uuid) {
	let first_winner = Artifact::register(conn, winner("winner-first"))
		.await
		.expect("winner registered first");
	Artifact::register(conn, loser("winner-first"))
		.await
		.expect("loser registered second");

	Artifact::register(conn, loser("loser-first"))
		.await
		.expect("loser registered first");
	let second_winner = Artifact::register(conn, winner("loser-first"))
		.await
		.expect("winner registered second");

	let offered = Artifact::get_for_version(conn, version, Scope::Unscoped)
		.await
		.expect("offered");

	let pick = |artifact_type: &str| {
		offered
			.iter()
			.find(|a| a.artifact_type == artifact_type)
			.unwrap_or_else(|| panic!("something offered for {artifact_type}"))
			.id
	};

	assert_eq!(offered.len(), 2, "one per type, whatever the order");
	(
		pick("winner-first"),
		first_winner.id,
		pick("loser-first"),
		second_winner.id,
	)
}

/// Within one scope an exact-version artifact is more specific than any range.
/// The group rule short-circuits ahead of this one, so a test that crosses
/// scopes never reaches it.
#[tokio::test(flavor = "multi_thread")]
async fn an_exact_artifact_displaces_a_range_of_the_same_scope() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		let (a, expected_a, b, expected_b) = winner_either_way(
			&mut conn,
			version,
			|t| unscoped(version, t, "https://x/exact"),
			|t| ranged(t, "2.60.x", "https://x/range"),
		)
		.await;

		assert_eq!(a, expected_a, "exact wins when it is registered first");
		assert_eq!(b, expected_b, "and when the range is");
	})
	.await;
}

/// Between two ranges that both match, the narrower is more specific.
#[tokio::test(flavor = "multi_thread")]
async fn the_narrower_of_two_ranges_wins() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		let (a, expected_a, b, expected_b) = winner_either_way(
			&mut conn,
			version,
			|t| ranged(t, "~2.60.0", "https://x/narrow"),
			|t| ranged(t, "^2.0.0", "https://x/wide"),
		)
		.await;

		assert_eq!(a, expected_a, "narrow wins when it is registered first");
		assert_eq!(b, expected_b, "and when the wide one is");
	})
	.await;
}

/// Where two ranges cover the same versions neither is narrower, so the
/// tiebreak is how explicitly each was written: `^` over `~` over `.x`.
#[tokio::test(flavor = "multi_thread")]
async fn equally_wide_ranges_fall_back_to_pattern_rank() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		let (a, expected_a, b, expected_b) = winner_either_way(
			&mut conn,
			version,
			|t| ranged(t, "~2.60.0", "https://x/tilde"),
			|t| ranged(t, "2.60.x", "https://x/wildcard"),
		)
		.await;

		assert_eq!(a, expected_a, "~ outranks .x when registered first");
		assert_eq!(b, expected_b, "and when .x is");
	})
	.await;
}

/// A pattern Canopy cannot parse matches nothing rather than everything, so a
/// malformed range withholds a file instead of offering it to the whole fleet.
#[tokio::test(flavor = "multi_thread")]
async fn a_malformed_range_matches_nothing() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		Artifact::register(
			&mut conn,
			ranged("installer", "definitely not a range", "https://x/nope"),
		)
		.await
		.expect("register");

		let offered = Artifact::get_for_version(&mut conn, version, Scope::Unscoped)
			.await
			.expect("offered");
		assert!(offered.is_empty(), "offered to nobody");

		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert!(all.is_empty(), "and matches the version for no one");
	})
	.await;
}

/// Canopy records which device registered an artifact and the run that produced
/// it, so one that arrived by automation is distinguishable from one entered by
/// hand. A re-registration carries the new provenance rather than keeping the
/// old, since the row now describes a different build.
// spec: ART#registration
#[tokio::test(flavor = "multi_thread")]
async fn provenance_is_recorded_and_replaced() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;
		let run = Uuid::new_v4();

		let mut first = held(version, "reporting-schema", theirs, b"first");
		first.run_id = Some(run);
		let registered = Artifact::register(&mut conn, first)
			.await
			.expect("register with a run");
		assert_eq!(registered.run_id, Some(run));

		// Entered by hand this time: the run that is no longer named is cleared
		// rather than left standing over bytes it did not produce.
		let second = held(version, "reporting-schema", theirs, b"second");
		let replaced = Artifact::register(&mut conn, second)
			.await
			.expect("register without a run");
		assert_eq!(replaced.id, registered.id);
		assert_eq!(replaced.run_id, None);
	})
	.await;
}

/// Canopy keeps none of what it has stopped serving, so deleting an artifact
/// takes the bytes with it rather than leaving them addressable.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn deleting_an_artifact_takes_its_bytes() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;

		let artifact = Artifact::register(
			&mut conn,
			held(version, "reporting-schema", theirs, b"schema"),
		)
		.await
		.expect("register");

		Artifact::delete(&mut conn, artifact.id)
			.await
			.expect("delete");

		assert!(
			Artifact::content_for(&mut conn, artifact.id)
				.await
				.expect("read content")
				.is_none()
		);
		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert!(all.is_empty());
	})
	.await;
}

/// An artifact Canopy does not hold has no bytes to read, which is what makes
/// the download fall through to the location it recorded instead.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn an_unscoped_artifact_holds_no_bytes() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		let artifact = Artifact::register(&mut conn, unscoped(version, "installer", "https://x/i"))
			.await
			.expect("register");

		assert!(
			Artifact::content_for(&mut conn, artifact.id)
				.await
				.expect("read content")
				.is_none()
		);
	})
	.await;
}

/// A group's artifacts go with the group. Bytes Canopy holds for a group that
/// no longer exists are bytes it has stopped serving.
// spec: ART#where-an-artifact-rests
#[tokio::test(flavor = "multi_thread")]
async fn deleting_a_group_takes_its_artifacts() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;
		let theirs = seed_group(&mut conn, "kamaka").await;

		Artifact::register(&mut conn, unscoped(version, "installer", "https://x/i"))
			.await
			.expect("unscoped");
		Artifact::register(
			&mut conn,
			held(version, "reporting-schema", theirs, b"schema"),
		)
		.await
		.expect("held");

		conn.batch_execute(&format!("DELETE FROM server_groups WHERE id = '{theirs}'"))
			.await
			.expect("delete the group");

		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert_eq!(all.len(), 1, "the group's went with it");
		assert_eq!(all[0].group_id, None);
	})
	.await;
}

/// An unscoped exact artifact replaces itself too. Its key carries a NULL range
/// pattern and a NULL group, which the default treatment of NULL would have
/// made distinct from the row already there.
// spec: ART#registration
#[tokio::test(flavor = "multi_thread")]
async fn an_unscoped_exact_artifact_replaces_itself() {
	TestDb::run(|mut conn, _url| async move {
		let version = seed_version(&mut conn, 2, 60, 0).await;

		let first = Artifact::register(&mut conn, unscoped(version, "installer", "https://x/one"))
			.await
			.expect("first");
		let second = Artifact::register(&mut conn, unscoped(version, "installer", "https://x/two"))
			.await
			.expect("second");

		assert_eq!(first.id, second.id);
		assert_eq!(second.download_url.as_deref(), Some("https://x/two"));

		let all = Artifact::get_for_version_all_matches(&mut conn, version, Scope::Fleet)
			.await
			.expect("operator view");
		assert_eq!(all.len(), 1);
	})
	.await;
}

/// A group's name is shown whatever state the group is in, so a reference to an
/// archived group still reads as that group rather than as nothing.
#[tokio::test(flavor = "multi_thread")]
async fn an_archived_group_is_still_named() {
	TestDb::run(|mut conn, _url| async move {
		let theirs = seed_group(&mut conn, "kamaka").await;

		conn.batch_execute(&format!(
			"UPDATE server_groups SET deleted_at = now() WHERE id = '{theirs}'"
		))
		.await
		.expect("archive the group");

		let names = database::server_groups::ServerGroup::names_by_id(&mut conn)
			.await
			.expect("names");
		assert_eq!(names.get(&theirs).map(String::as_str), Some("kamaka"));
	})
	.await;
}
