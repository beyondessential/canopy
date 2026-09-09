//! The planned-upgrades calendar feed: what a subscriber's calendar fetches,
//! and what a URL carrying the wrong secret gets.

use commons_tests::diesel_async::SimpleAsyncConnection;
use commons_types::{
	server::rank::ServerRank,
	version::{VersionStatus, VersionStr},
};
use database::{
	reported_detail::ReportedDetail,
	server_groups::ServerGroup,
	upgrade_plans::{PlannedWhen, UpgradePlan},
	versions::{NewVersion, Version},
};
use diesel::SelectableHelper;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::civil::{date, time};
use uuid::Uuid;

/// Undo RFC 5545 line folding, so a test can assert on a value that the
/// wire format split across lines.
fn unfold(body: &str) -> String {
	body.replace("\r\n ", "")
}

/// The secret the test harness configures the feed with.
const SECRET: &str = "test-secret";

const GROUP: &str = "11111111-1111-1111-1111-111111111111";
const SERVER: &str = "22222222-2222-2222-2222-222222222222";

/// A group running 2.60.0, and a published 2.61.0 to plan onto.
async fn seed(conn: &mut AsyncPgConnection) -> (Uuid, Version) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{GROUP}', 'kamaka'); \
		 INSERT INTO machines (id, group_id) VALUES ('{SERVER}', '{GROUP}'); \
		 INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES \
			('{SERVER}', 'https://central.kamaka.example', 'tamanu-central', 'production', '{GROUP}', '{SERVER}');"
	))
	.await
	.expect("seed");

	let running: VersionStr = "2.60.0".parse().expect("parse");
	ReportedDetail::record(
		conn,
		Some(SERVER.parse().expect("uuid")),
		// The fixture gives the box the same id as the application on it.
		SERVER.parse().expect("uuid"),
		"test",
		&serde_json::json!({}),
		Some(&running),
	)
	.await
	.expect("report");
	let group: Uuid = GROUP.parse().expect("uuid");
	ServerGroup::recompute_version(conn, group)
		.await
		.expect("recompute");

	let target = diesel::insert_into(database::schema::versions::table)
		.values(NewVersion {
			major: 2,
			minor: 61,
			patch: 0,
			status: VersionStatus::Published,
			changelog: String::new(),
			device_id: None,
		})
		.returning(Version::as_returning())
		.get_result(conn)
		.await
		.expect("publish");

	(group, target)
}

#[tokio::test(flavor = "multi_thread")]
async fn a_dated_plan_is_an_all_day_entry() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let (group, target) = seed(&mut conn).await;
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				..Default::default()
			},
			Some("night of the 14th"),
			"someone@example.com",
		)
		.await
		.expect("record");
		let resp = public
			.get(&format!("/calendar/{SECRET}/upgrades.ics"))
			.await;
		assert_eq!(resp.status_code().as_u16(), 200);
		assert_eq!(
			resp.header("content-type"),
			"text/calendar; charset=utf-8",
			"a calendar client picks the feed up by its content type"
		);

		let raw = resp.text();
		assert!(raw.starts_with("BEGIN:VCALENDAR\r\n"), "{raw}");
		assert!(raw.ends_with("END:VCALENDAR\r\n"), "{raw}");
		assert!(
			raw.lines().all(|line| line.trim_end().len() <= 75),
			"no content line may exceed 75 octets: {raw}"
		);

		let body = unfold(&raw);
		assert!(body.contains("SUMMARY:kamaka upgrade to 2.61.0"), "{body}");
		assert!(body.contains("DTSTART;VALUE=DATE:20260814"), "{body}");
		assert!(
			body.contains("DTEND;VALUE=DATE:20260815"),
			"an all-day entry ends on the following day: {body}"
		);
		assert!(
			body.contains("DESCRIPTION:Now on 2.60.0\\nnight of the 14th"),
			"{body}"
		);
		// The feed is a public URL, so it carries no operator's address.
		assert!(!body.contains("someone@example.com"), "{body}");
		assert!(body.contains("TRANSP:TRANSPARENT"), "{body}");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn an_hour_is_resolved_from_its_zone_to_an_instant() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let (group, target) = seed(&mut conn).await;
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				time: Some(time(22, 0, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
				..Default::default()
			},
			None,
			"someone@example.com",
		)
		.await
		.expect("record");
		let body = unfold(
			&public
				.get(&format!("/calendar/{SECRET}/upgrades.ics"))
				.await
				.text(),
		);
		// 22:00 in Fiji (UTC+12) is 10:00 UTC the same day.
		assert!(body.contains("DTSTART:20260814T100000Z"), "{body}");
		assert!(body.contains("DTEND:20260814T110000Z"), "{body}");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_window_ends_where_the_plan_closes_it() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let (group, target) = seed(&mut conn).await;
		let plan = UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				time: Some(time(9, 0, 0, 0)),
				end: Some(time(11, 30, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
			},
			None,
			"someone@example.com",
		)
		.await
		.expect("record");
		let body = unfold(
			&public
				.get(&format!("/calendar/{SECRET}/upgrades.ics"))
				.await
				.text(),
		);
		// 09:00 to 11:30 in Fiji (UTC+12) is 21:00 to 23:30 UTC the day before.
		assert!(body.contains("DTSTART:20260813T210000Z"), "{body}");
		assert!(body.contains("DTEND:20260813T233000Z"), "{body}");

		UpgradePlan::amend(
			&mut conn,
			plan.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				time: Some(time(22, 0, 0, 0)),
				end: Some(time(2, 0, 0, 0)),
				zone: Some("Pacific/Fiji".into()),
			},
			None,
			"someone@example.com",
		)
		.await
		.expect("amend");

		let body = unfold(
			&public
				.get(&format!("/calendar/{SECRET}/upgrades.ics"))
				.await
				.text(),
		);
		assert!(body.contains("DTSTART:20260814T100000Z"), "{body}");
		assert!(
			body.contains("DTEND:20260814T140000Z"),
			"a window closing earlier in the day than it opened runs into the next morning: {body}"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_url_with_the_wrong_secret_has_nothing_at_it() {
	commons_tests::server::run(async |_conn, public, _private| {
		assert_eq!(
			public
				.get("/calendar/not-the-secret/upgrades.ics")
				.await
				.status_code()
				.as_u16(),
			404
		);

		assert_eq!(
			public
				.get(&format!("/calendar/{SECRET}/upgrades.ics"))
				.await
				.status_code()
				.as_u16(),
			200
		);
	})
	.await
}

const CLONE_BOX: &str = "33333333-3333-3333-3333-333333333333";

/// A clone's entry names the environment and carries the clone's own running
/// version. The team reads this feed to know what is being upgraded when, and an
/// entry that reads as the site would put a clone's window in the diary as
/// production's.
// spec: UPG#the-calendar-feed
#[tokio::test(flavor = "multi_thread")]
async fn a_clones_entry_names_the_environment_and_its_own_version() {
	commons_tests::server::run(async |mut conn, public, _private| {
		let (group, target) = seed(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO machines (id, group_id) VALUES ('{CLONE_BOX}', '{GROUP}'); \
			 INSERT INTO applications (id, host, type, rank, group_id, machine_id) VALUES \
				('{CLONE_BOX}', 'https://clone.kamaka.example', 'tamanu-central', 'clone', '{GROUP}', '{CLONE_BOX}');"
		))
		.await
		.expect("add a clone");
		// The clone is a minor ahead of production, so an entry carrying 2.60.0
		// would be reading the group's headline rather than the environment's.
		let clone_running: VersionStr = "2.60.5".parse().expect("parse");
		ReportedDetail::record(
			&mut conn,
			Some(CLONE_BOX.parse().expect("uuid")),
			CLONE_BOX.parse().expect("uuid"),
			"test",
			&serde_json::json!({}),
			Some(&clone_running),
		)
		.await
		.expect("report");

		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Clone,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 14)),
				..Default::default()
			},
			None,
			"someone@example.com",
		)
		.await
		.expect("record the clone's plan");
		UpgradePlan::record(
			&mut conn,
			group,
			ServerRank::Production,
			target.id,
			PlannedWhen {
				date: Some(date(2026, 8, 21)),
				..Default::default()
			},
			None,
			"someone@example.com",
		)
		.await
		.expect("record production's plan");

		let body = unfold(
			&public
				.get(&format!("/calendar/{SECRET}/upgrades.ics"))
				.await
				.text(),
		);
		assert!(
			body.contains("SUMMARY:kamaka clone upgrade to 2.61.0"),
			"the clone's entry names the environment: {body}"
		);
		assert!(
			body.contains("SUMMARY:kamaka upgrade to 2.61.0"),
			"and production's reads as the site alone: {body}"
		);
		assert!(
			body.contains("DESCRIPTION:Now on 2.60.5"),
			"the clone's entry carries the clone's own version: {body}"
		);
		assert!(
			body.contains("DESCRIPTION:Now on 2.60.0"),
			"and production's carries production's: {body}"
		);
	})
	.await
}
