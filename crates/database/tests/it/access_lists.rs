use database::access_lists::{AccessListEntry, ListLevel};
use diesel_async::SimpleAsyncConnection as _;

const UP: &str = include_str!("../../../../migrations/2026-11-27-051442-0000_access_lists/up.sql");
const DOWN: &str =
	include_str!("../../../../migrations/2026-11-27-051442-0000_access_lists/down.sql");

// spec: ADM#how-a-level-is-held
#[tokio::test(flavor = "multi_thread")]
async fn a_list_holds_a_login_lowercased_and_matches_it_whatever_its_case() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let added = AccessListEntry::add(&mut conn, ListLevel::Pm, " Pat@Example.com ")
			.await
			.expect("add")
			.expect("newly added");
		assert_eq!(added.login, "pat@example.com");

		assert_eq!(
			AccessListEntry::highest_for(&mut conn, "PAT@example.COM")
				.await
				.expect("lookup"),
			Some(ListLevel::Pm),
		);
		assert_eq!(
			AccessListEntry::highest_for(&mut conn, "someone@example.com")
				.await
				.expect("lookup"),
			None,
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn adding_a_login_already_on_a_list_keeps_the_entry() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		AccessListEntry::add(&mut conn, ListLevel::Admin, "someone@example.com")
			.await
			.expect("first add")
			.expect("newly added");
		let again = AccessListEntry::add(&mut conn, ListLevel::Admin, "Someone@Example.com")
			.await
			.expect("re-adding must not fail");
		assert!(again.is_none(), "nothing was added");

		let all = AccessListEntry::list(&mut conn).await.expect("list");
		assert_eq!(all.len(), 1, "no duplicate entry");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_blank_login_is_refused() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		AccessListEntry::add(&mut conn, ListLevel::Pm, "  ")
			.await
			.expect_err("blank login");
	})
	.await
}

/// A login no identity could carry confers nothing, so it never goes on a list.
// spec: ADM#the-permissions-page
#[tokio::test(flavor = "multi_thread")]
async fn a_login_that_could_never_match_anyone_is_refused() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for bad in [
			"pm-team",
			"pat bes.au",
			"@example.com",
			"pat@",
			"pat @example.com",
			"p@t@example.com",
			"Pat <pat@example.com>",
		] {
			AccessListEntry::add(&mut conn, ListLevel::Pm, bad)
				.await
				.expect_err(bad);
		}
	})
	.await
}

// spec: ADM#the-levels
#[tokio::test(flavor = "multi_thread")]
async fn the_highest_list_a_login_is_on_wins_and_removal_is_per_list() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for level in [ListLevel::Pm, ListLevel::Admin] {
			AccessListEntry::add(&mut conn, level, "both@example.com")
				.await
				.expect("add");
		}
		assert_eq!(
			AccessListEntry::highest_for(&mut conn, "both@example.com")
				.await
				.expect("lookup"),
			Some(ListLevel::Admin),
		);

		assert!(
			AccessListEntry::remove(&mut conn, ListLevel::Admin, "BOTH@example.com")
				.await
				.expect("remove")
		);
		assert!(
			!AccessListEntry::remove(&mut conn, ListLevel::Admin, "both@example.com")
				.await
				.expect("remove again"),
			"a second removal finds nothing",
		);
		assert_eq!(
			AccessListEntry::highest_for(&mut conn, "both@example.com")
				.await
				.expect("lookup"),
			Some(ListLevel::Pm),
		);
	})
	.await
}

/// Replays the migration over an `admins` table holding the same login in two
/// cases.
// spec: ADM#how-a-level-is-held
#[tokio::test(flavor = "multi_thread")]
async fn existing_admins_are_still_admins_after_the_migration() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		conn.batch_execute(DOWN).await.expect("revert");
		conn.batch_execute(
			"INSERT INTO admins (email, created_at) VALUES \
			 ('Boss@Example.com', '2026-01-02T00:00:00Z'), \
			 ('boss@example.com', '2026-01-01T00:00:00Z'), \
			 ('ops@example.com', '2026-01-03T00:00:00Z');",
		)
		.await
		.expect("seed admins");
		conn.batch_execute(UP).await.expect("re-apply");

		let entries = AccessListEntry::list(&mut conn).await.expect("list");
		let logins: Vec<_> = entries
			.iter()
			.map(|e| (e.level, e.login.as_str(), e.created_at.to_string()))
			.collect();
		assert_eq!(
			logins,
			vec![
				(
					ListLevel::Admin,
					"boss@example.com",
					"2026-01-01T00:00:00Z".to_string()
				),
				(
					ListLevel::Admin,
					"ops@example.com",
					"2026-01-03T00:00:00Z".to_string()
				),
			],
		);
	})
	.await
}
