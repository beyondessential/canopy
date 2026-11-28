//! The display names and pictures Canopy keeps for the people who use it.

use commons_tests::db::TestDb;
use database::tailscale_users::TailscaleUser;

async fn updated_at(conn: &mut database::diesel_async::AsyncPgConnection) -> jiff::Timestamp {
	TailscaleUser::all(conn)
		.await
		.unwrap()
		.into_iter()
		.find(|user| user.login == "pat@example.com")
		.expect("pat is known")
		.updated_at
}

/// Every authenticated request refreshes the caller's name and picture, so a
/// refresh that changes nothing leaves the row as it was.
#[tokio::test(flavor = "multi_thread")]
async fn a_refresh_that_changes_nothing_writes_nothing() {
	TestDb::run(async |mut conn, _| {
		TailscaleUser::upsert(&mut conn, "pat@example.com", "Pat Smith", None)
			.await
			.unwrap();
		let first = updated_at(&mut conn).await;

		TailscaleUser::upsert(&mut conn, "pat@example.com", "Pat Smith", None)
			.await
			.unwrap();
		assert_eq!(updated_at(&mut conn).await, first, "unchanged");

		TailscaleUser::upsert(
			&mut conn,
			"pat@example.com",
			"Pat Smith",
			Some("https://pic/pat"),
		)
		.await
		.unwrap();
		let known = TailscaleUser::all(&mut conn).await.unwrap();
		assert_eq!(known[0].profile_pic.as_deref(), Some("https://pic/pat"));
		assert!(
			updated_at(&mut conn).await > first,
			"a new picture is written"
		);
	})
	.await
}
