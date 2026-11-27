//! A group's project details: who edits them, how a change is recorded, and how
//! they read back.
//!
//! Spec: `.workhorse/specs/servers/groups.md` (id `GRP`), "Project details".

use commons_tests::{axum_test::TestServer, diesel_async::SimpleAsyncConnection};
use database::{
	access_lists::{AccessListEntry, ListLevel},
	audit::{Actor, RecordKind},
	diesel_async::AsyncPgConnection,
	tailscale_users::TailscaleUser,
};
use serde_json::{Value, json};

use crate::access::{log, post_in, problem, raised};
use crate::access_levels::{run_with_policy, trust_headers};

const PM: &str = "pat@example.com";
const OTHER_PM: &str = "lee@example.com";
const STRANGER: &str = "stranger@example.com";
const LED_BY_OTHER: &str = "11111111-1111-1111-1111-111111111111";
const UNLED: &str = "22222222-2222-2222-2222-222222222222";
const ACTION: &str = "group_project_details_changed";

async fn seed(conn: &mut AsyncPgConnection) {
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name, pm_in_charge) VALUES \
			('{LED_BY_OTHER}', 'Led by another', '{OTHER_PM}'), \
			('{UNLED}', 'Led by no one', NULL);"
	))
	.await
	.expect("seed groups");
}

async fn edit(
	private: &TestServer,
	login: &str,
	session: &str,
	group: &str,
	details: Value,
) -> Value {
	post_in(
		private,
		"/api/fleet/groups/update_project_details",
		Some(login),
		Some(session),
		json!({ "server_group_id": group, "details": details }),
	)
	.await
}

/// The group's project details as its page reads them, read by anyone.
async fn details_of(private: &TestServer, group: &str) -> Value {
	let response = private
		.post("/api/fleet/groups/get")
		.add_header("Tailscale-User-Login", STRANGER)
		.add_header("Tailscale-User-Name", "Someone")
		.json(&json!({ "server_group_id": group }))
		.await;
	response.assert_status_ok();
	response.json::<Value>()["project_details"].clone()
}

// spec: GRP#project-details
#[tokio::test(flavor = "multi_thread")]
async fn a_pm_edits_any_group_and_each_change_is_audited_before_and_after() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		seed(&mut conn).await;
		AccessListEntry::add(&mut conn, ListLevel::Pm, PM)
			.await
			.unwrap();
		let write = raised(&private, PM, "write").await;

		// Someone else is in charge of this one.
		let edited = edit(
			&private,
			PM,
			&write,
			LED_BY_OTHER,
			json!({ "project": "Clinic rollout", "pm_in_charge": OTHER_PM, "ground_contact": "Sam at the clinic" }),
		)
		.await;
		assert_eq!(edited["status"], 200, "{edited}");

		// No one is in charge of this one, and the PM puts themself in charge.
		let edited = edit(
			&private,
			PM,
			&write,
			UNLED,
			json!({ "pm_in_charge": PM }),
		)
		.await;
		assert_eq!(edited["status"], 200, "{edited}");

		let entries = log(&mut conn, ACTION).await;
		assert_eq!(entries.len(), 2, "one entry per edit");
		let by_group = |group: &str| {
			entries
				.iter()
				.find(|entry| entry.records.iter().any(|r| r.id == group))
				.unwrap_or_else(|| panic!("an entry for {group}"))
		};

		let led = by_group(LED_BY_OTHER);
		assert!(matches!(led.records[0].kind, RecordKind::Group));
		assert_eq!(
			led.detail,
			json!({
				"project": { "from": null, "to": "Clinic rollout" },
				"pm_in_charge": null,
				"ground_contact": { "from": null, "to": "Sam at the clinic" },
			}),
			"each changed detail with before and after, nothing for the one left as it was"
		);
		assert!(
			matches!(&led.actor, Actor::Operator { login, .. } if login == PM),
			"{:?}",
			led.actor
		);
		assert_eq!(
			by_group(UNLED).detail,
			json!({
				"project": null,
				"pm_in_charge": { "from": null, "to": PM },
				"ground_contact": null,
			}),
		);

		// The same details again leave everything as it was and record nothing.
		let again = edit(
			&private,
			PM,
			&write,
			LED_BY_OTHER,
			json!({ "project": "Clinic rollout", "pm_in_charge": OTHER_PM, "ground_contact": " Sam at the clinic " }),
		)
		.await;
		assert_eq!(again["status"], 200, "{again}");
		assert_eq!(log(&mut conn, ACTION).await.len(), 2, "no entry for an unchanged save");

		// Clearing one records its old value against none.
		let cleared = edit(
			&private,
			PM,
			&write,
			LED_BY_OTHER,
			json!({ "project": "", "pm_in_charge": OTHER_PM, "ground_contact": "Sam at the clinic" }),
		)
		.await;
		assert_eq!(cleared["status"], 200, "{cleared}");
		let entries = log(&mut conn, ACTION).await;
		assert_eq!(entries.len(), 3);
		assert!(
			entries.iter().any(|entry| entry.detail
				== json!({
					"project": { "from": "Clinic rollout", "to": null },
					"pm_in_charge": null,
					"ground_contact": null,
				})),
			"{entries:?}"
		);
	})
	.await;
}

// spec: GRP#project-details
// spec: ADM#the-levels
#[tokio::test(flavor = "multi_thread")]
async fn an_all_level_caller_is_refused_and_a_pm_cannot_use_the_groups_own_edit() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		seed(&mut conn).await;
		AccessListEntry::add(&mut conn, ListLevel::Pm, PM)
			.await
			.unwrap();

		// Raised as far as a session goes, so each refusal is the level's.
		let stranger = raised(&private, STRANGER, "danger").await;
		let refused = edit(
			&private,
			STRANGER,
			&stranger,
			UNLED,
			json!({ "project": "Mine now" }),
		)
		.await;
		assert_eq!(refused["status"], 403, "{refused}");
		assert!(
			problem(&refused).ends_with("auth-insufficient-permissions"),
			"{refused}"
		);

		let pm = raised(&private, PM, "danger").await;
		let refused = post_in(
			&private,
			"/api/fleet/groups/update",
			Some(PM),
			Some(&pm),
			json!({ "server_group_id": UNLED, "data": { "name": "Renamed" } }),
		)
		.await;
		assert_eq!(refused["status"], 403, "{refused}");
		assert!(
			problem(&refused).ends_with("auth-insufficient-permissions"),
			"{refused}"
		);

		assert_eq!(
			details_of(&private, UNLED).await["project"],
			Value::Null,
			"nothing was changed"
		);
		assert!(log(&mut conn, ACTION).await.is_empty());
	})
	.await;
}

// spec: ADM#how-a-level-is-held
#[tokio::test(flavor = "multi_thread")]
async fn removing_someone_from_the_pm_list_refuses_their_next_project_details_edit() {
	trust_headers();

	run_with_policy(None, async |mut conn, private, _| {
		seed(&mut conn).await;
		AccessListEntry::add(&mut conn, ListLevel::Pm, PM)
			.await
			.unwrap();
		let write = raised(&private, PM, "write").await;

		let edited = edit(&private, PM, &write, UNLED, json!({ "project": "First" })).await;
		assert_eq!(edited["status"], 200, "{edited}");

		AccessListEntry::remove(&mut conn, ListLevel::Pm, PM)
			.await
			.unwrap();

		let refused = edit(&private, PM, &write, UNLED, json!({ "project": "Second" })).await;
		assert_eq!(refused["status"], 403, "{refused}");
		assert!(
			problem(&refused).ends_with("auth-insufficient-permissions"),
			"{refused}"
		);
		assert_eq!(details_of(&private, UNLED).await["project"], "First");
	})
	.await;
}

// spec: GRP#project-details
#[tokio::test(flavor = "multi_thread")]
async fn a_pm_in_charge_without_text_either_side_of_an_at_is_refused() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		for pm in ["pat", "@example.com", "pat@", "@"] {
			let response = private
				.post("/api/fleet/groups/update_project_details")
				.json(&json!({
					"server_group_id": UNLED,
					"details": { "project": "Changed", "pm_in_charge": pm },
				}))
				.await;
			assert_eq!(response.status_code().as_u16(), 400, "{pm:?} is refused");
		}
		assert_eq!(
			details_of(&private, UNLED).await,
			json!({
				"project": null,
				"pm_in_charge": null,
				"pm_in_charge_name": null,
				"ground_contact": null,
			}),
			"a refused edit changes nothing"
		);

		private
			.post("/api/fleet/groups/update_project_details")
			.json(&json!({
				"server_group_id": UNLED,
				"details": { "pm_in_charge": " Pat@Example.com " },
			}))
			.await
			.assert_status_ok();
		assert_eq!(
			details_of(&private, UNLED).await["pm_in_charge"],
			"Pat@Example.com"
		);
	})
	.await;
}

// spec: GRP#project-details
#[tokio::test(flavor = "multi_thread")]
async fn the_pm_in_charge_reads_by_display_name_where_known_and_empty_details_read_as_none() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;
		TailscaleUser::upsert(&mut conn, "Pat@Example.com", "Pat Smith", None)
			.await
			.unwrap();

		let saved = private
			.post("/api/fleet/groups/update_project_details")
			.json(&json!({
				"server_group_id": UNLED,
				"details": { "project": "", "pm_in_charge": "pat@EXAMPLE.com", "ground_contact": "   " },
			}))
			.await;
		saved.assert_status_ok();
		let read = details_of(&private, UNLED).await;
		assert_eq!(
			saved.json::<Value>(),
			read,
			"the save answers what a read does"
		);
		assert_eq!(
			read,
			json!({
				"project": null,
				"pm_in_charge": "pat@EXAMPLE.com",
				"pm_in_charge_name": "Pat Smith",
				"ground_contact": null,
			}),
			"known by display name ignoring case, and empty details are none"
		);

		// A login Canopy knows no name for reads as entered.
		assert_eq!(
			details_of(&private, LED_BY_OTHER).await,
			json!({
				"project": null,
				"pm_in_charge": OTHER_PM,
				"pm_in_charge_name": null,
				"ground_contact": null,
			}),
		);
	})
	.await;
}

// spec: GRP#project-details
#[tokio::test(flavor = "multi_thread")]
async fn the_groups_own_edit_does_not_carry_the_project_details() {
	commons_tests::server::run(async |mut conn, _, private| {
		seed(&mut conn).await;

		private
			.post("/api/fleet/groups/update")
			.json(&json!({
				"server_group_id": UNLED,
				"data": { "name": "Renamed", "project": "Smuggled", "pm_in_charge": PM, "ground_contact": "Smuggled" },
			}))
			.await
			.assert_status_ok();
		assert_eq!(
			details_of(&private, UNLED).await,
			json!({
				"project": null,
				"pm_in_charge": null,
				"pm_in_charge_name": null,
				"ground_contact": null,
			}),
		);
	})
	.await;
}
