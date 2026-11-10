//! Incident memory: entries, their history, search, relevance to an incident,
//! and the index.

use database::{
	memory::{
		self, Amendment, BODY_LIMIT, Find, Link, LinkKind, NewEntry, Outcome, RevisionKind,
		TITLE_LIMIT,
	},
	notes_log::Author,
};
use diesel_async::{AsyncPgConnection, SimpleAsyncConnection as _};
use uuid::Uuid;

fn entry(title: &str, link: Option<Link>) -> NewEntry {
	NewEntry {
		title: title.into(),
		body: format!("{title} body"),
		link,
	}
}

struct Seeded {
	group: Uuid,
	other_group: Uuid,
	incident: Uuid,
	other_incident: Uuid,
}

/// Two groups, each with an incident; the first group's incident has an issue
/// from check `disk-space` of source `alertd`, and is published.
async fn seed(conn: &mut AsyncPgConnection) -> Seeded {
	let s = Seeded {
		group: Uuid::new_v4(),
		other_group: Uuid::new_v4(),
		incident: Uuid::new_v4(),
		other_incident: Uuid::new_v4(),
	};
	let device = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let issue = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{device}', 'machine'); \
		 INSERT INTO server_groups (id, name) VALUES ('{g}', 'Pacific'), ('{og}', 'Highlands'); \
		 INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{machine}', '{g}', '{device}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
			('{machine}', 'https://example.com', 'tamanu-central', '{g}', 'production', '{machine}'); \
		 INSERT INTO check_policies (source, check_name, ceiling, escalates) \
			VALUES ('alertd', 'disk-space', 'failed', true), ('alertd', 'cpu', 'failed', true); \
		 INSERT INTO issues \
			(id, application_id, device_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen) \
		   VALUES ('{issue}', '{machine}', '{device}', 'alertd', 'r', 'disk-space', 'failed', 'failed', 'm', true, NOW(), NOW()); \
		 INSERT INTO incidents (id, server_group_id, rank, opened_at) VALUES \
			('{i}', '{g}', 'production', NOW()), ('{oi}', '{og}', 'production', NOW()); \
		 INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ('{i}', '{issue}', NOW()); \
		 INSERT INTO slack_outbox (kind, incident_id, payload, delivered_at) \
			VALUES ('incident_open', '{i}', '{{}}', NOW()); \
		 INSERT INTO incident_notes (incident_id, author, body) VALUES ('{i}', 'op', 'looked at it');",
		g = s.group,
		og = s.other_group,
		i = s.incident,
		oi = s.other_incident,
	))
	.await
	.expect("seed");
	s
}

#[tokio::test(flavor = "multi_thread")]
async fn an_entry_has_a_history_of_every_change() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let s = seed(&mut conn).await;
		let agent = Author::token("agent-1");
		let operator = Author::operator("alice");

		let created = memory::record(&mut conn, entry("Disks fill on Mondays", None), agent)
			.await
			.unwrap();
		assert_eq!(created.revision, 1);
		assert!(created.link().is_none());

		let Outcome::Changed(amended) = memory::amend(
			&mut conn,
			created.id,
			1,
			Amendment {
				body: Some("It is the weekly export".into()),
				link: Some(Some(Link::Group { id: s.group })),
				..Default::default()
			},
			operator,
		)
		.await
		.unwrap() else {
			panic!("expected the amendment to apply");
		};
		assert_eq!(amended.revision, 2);
		assert_eq!(amended.link(), Some(Link::Group { id: s.group }));
		assert_eq!(amended.updated_by, "alice");
		assert_eq!(amended.created_by, "agent-1");

		assert!(matches!(
			memory::amend(&mut conn, created.id, 2, Amendment::default(), operator)
				.await
				.unwrap(),
			Outcome::Unchanged(_)
		));
		assert!(matches!(
			memory::amend(
				&mut conn,
				created.id,
				1,
				Amendment {
					title: Some("stale".into()),
					..Default::default()
				},
				operator
			)
			.await
			.unwrap(),
			Outcome::Stale(held) if held.revision == 2
		));

		let retired = memory::retire(&mut conn, created.id, "fixed upstream", operator)
			.await
			.unwrap();
		assert_eq!(retired.revision, 3);
		assert_eq!(retired.retired_reason.as_deref(), Some("fixed upstream"));
		assert!(
			memory::retire(&mut conn, created.id, "again", operator)
				.await
				.is_err()
		);
		assert!(
			memory::amend(&mut conn, created.id, 3, Amendment::default(), operator)
				.await
				.is_err(),
			"a retired entry is restored before it is changed"
		);

		// Restoring version 1 revives the entry with that version's content.
		let Outcome::Changed(restored) = memory::restore(&mut conn, created.id, 3, 1, operator)
			.await
			.unwrap()
		else {
			panic!("expected the restoration to apply");
		};
		assert_eq!(restored.revision, 4);
		assert!(!restored.is_retired());
		assert_eq!(restored.title, "Disks fill on Mondays");
		assert!(restored.link().is_none());
		assert!(restored.retired_reason.is_none());

		let history = memory::history(&mut conn, created.id).await.unwrap();
		let shape: Vec<_> = history
			.iter()
			.map(|r| (r.revision, r.kind, r.via.as_str(), r.restored_from))
			.collect();
		assert_eq!(
			shape,
			vec![
				(4, RevisionKind::Restored, "operator", Some(1)),
				(3, RevisionKind::Retired, "operator", None),
				(2, RevisionKind::Amended, "operator", None),
				(1, RevisionKind::Recorded, "token", None),
			]
		);
		assert_eq!(history[3].body, "Disks fill on Mondays body");

		assert!(matches!(
			memory::restore(&mut conn, created.id, 3, 1, operator)
				.await
				.unwrap(),
			Outcome::Stale(_)
		));
		assert!(
			memory::restore(&mut conn, created.id, 4, 99, operator)
				.await
				.is_err()
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn titles_bodies_and_links_are_checked() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let _ = seed(&mut conn).await;
		let by = Author::operator("alice");

		let long_title = memory::record(
			&mut conn,
			NewEntry {
				title: "t".repeat(TITLE_LIMIT + 1),
				body: String::new(),
				link: None,
			},
			by,
		)
		.await
		.unwrap_err();
		assert!(long_title.to_string().contains("512"));
		let long_body = memory::record(
			&mut conn,
			NewEntry {
				title: "t".into(),
				body: "b".repeat(BODY_LIMIT + 1),
				link: None,
			},
			by,
		)
		.await
		.unwrap_err();
		assert!(long_body.to_string().contains("16384"));
		assert!(
			memory::record(&mut conn, entry("  ", None), by)
				.await
				.is_err(),
			"a blank title is refused"
		);

		let no_such_check = Link::Check {
			source: "alertd".into(),
			name: "nope".into(),
		};
		assert!(
			memory::record(&mut conn, entry("x", Some(no_such_check)), by)
				.await
				.is_err()
		);
		assert!(
			memory::record(
				&mut conn,
				entry("x", Some(Link::Group { id: Uuid::new_v4() })),
				by
			)
			.await
			.is_err()
		);
		assert!(
			memory::record(
				&mut conn,
				entry(
					"x",
					Some(Link::Check {
						source: "alertd".into(),
						name: "disk-space".into()
					})
				),
				by
			)
			.await
			.is_ok()
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn find_searches_filters_and_leaves_out_what_is_retired() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let s = seed(&mut conn).await;
		let by = Author::operator("alice");
		let disk = Link::Check {
			source: "alertd".into(),
			name: "disk-space".into(),
		};
		memory::record(&mut conn, entry("Fleet wide quirk", None), by)
			.await
			.unwrap();
		let pacific = memory::record(
			&mut conn,
			entry("Pacific needs the VPN", Some(Link::Group { id: s.group })),
			by,
		)
		.await
		.unwrap();
		memory::record(
			&mut conn,
			entry("Disk space thresholds", Some(disk.clone())),
			by,
		)
		.await
		.unwrap();
		let incident = memory::record(
			&mut conn,
			entry("One-off cause", Some(Link::Incident { id: s.incident })),
			by,
		)
		.await
		.unwrap();

		let find = |f: Find| async move { f };
		let all = memory::find(
			&mut conn,
			&find(Find {
				limit: 50,
				..Default::default()
			})
			.await,
		)
		.await
		.unwrap();
		assert_eq!(all.total, 4);

		let words = memory::find(
			&mut conn,
			&Find {
				text: Some("vp".into()),
				limit: 50,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(
			words.entries.iter().map(|e| e.id).collect::<Vec<_>>(),
			vec![pacific.id],
			"a word prefix matches"
		);

		for (kind, expect) in [
			(LinkKind::Fleet, 1),
			(LinkKind::Group, 1),
			(LinkKind::Check, 1),
			(LinkKind::Incident, 1),
		] {
			let found = memory::find(
				&mut conn,
				&Find {
					link_kind: Some(kind),
					limit: 50,
					..Default::default()
				},
			)
			.await
			.unwrap();
			assert_eq!(found.total, expect, "{kind:?}");
		}
		let by_check = memory::find(
			&mut conn,
			&Find {
				check: Some(("alertd".into(), "disk-space".into())),
				limit: 50,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(by_check.total, 1);

		// Relevant to the incident: itself, its group, its issues' check.
		let relevant = memory::find(
			&mut conn,
			&Find {
				relevant_to_incident: Some(s.incident),
				limit: 50,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(relevant.total, 3);
		let other = memory::find(
			&mut conn,
			&Find {
				relevant_to_incident: Some(s.other_incident),
				limit: 50,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(other.total, 0);
		assert_eq!(
			memory::relevant_count_for_incident(&mut conn, s.incident)
				.await
				.unwrap(),
			3
		);

		memory::retire(&mut conn, incident.id, "done", by)
			.await
			.unwrap();
		assert_eq!(
			memory::relevant_count_for_incident(&mut conn, s.incident)
				.await
				.unwrap(),
			2
		);
		let with_retired = memory::find(
			&mut conn,
			&Find {
				include_retired: true,
				limit: 2,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!((with_retired.total, with_retired.entries.len()), (4, 2));

		let names = memory::resolve_links(&mut conn, [Link::Group { id: s.group }, disk])
			.await
			.unwrap();
		assert_eq!(names[&Link::Group { id: s.group }], "Pacific");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_index_shows_what_is_known_and_where_trouble_has_no_memory() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let s = seed(&mut conn).await;
		let by = Author::operator("alice");
		memory::record(&mut conn, entry("Fleet wide quirk", None), by)
			.await
			.unwrap();
		memory::record(
			&mut conn,
			entry("Highlands is slow", Some(Link::Group { id: s.other_group })),
			by,
		)
		.await
		.unwrap();

		let index = memory::index(&mut conn, None, 90).await.unwrap();
		assert_eq!(index.entries.len(), 2);

		assert_eq!(index.checks.len(), 1);
		let check = &index.checks[0];
		assert_eq!(
			(check.source.as_str(), check.name.as_str()),
			("alertd", "disk-space")
		);
		assert_eq!((check.incidents, check.published), (1, 1));
		assert_eq!(check.groups, vec!["Pacific".to_owned()]);
		assert_eq!(check.linked_entries, 0, "a gap is shown, not left out");

		let pacific = index.groups.iter().find(|g| g.name == "Pacific").unwrap();
		assert_eq!(
			(pacific.incidents, pacific.published, pacific.linked_entries),
			(1, 1, 0)
		);
		let highlands = index.groups.iter().find(|g| g.name == "Highlands").unwrap();
		assert_eq!(
			(
				highlands.incidents,
				highlands.published,
				highlands.linked_entries
			),
			(1, 0, 1)
		);

		assert_eq!(index.incidents_with_notes.len(), 1);
		assert_eq!(index.incidents_with_notes[0].incident_id, s.incident);
		assert_eq!(index.incidents_with_notes[0].notes, 1);

		let narrowed = memory::index(&mut conn, Some(s.group), 90).await.unwrap();
		assert_eq!(narrowed.groups.len(), 1);
		assert_eq!(narrowed.groups[0].name, "Pacific");
		assert_eq!(
			narrowed
				.entries
				.iter()
				.map(|e| e.title.as_str())
				.collect::<Vec<_>>(),
			vec!["Fleet wide quirk"],
			"another group's entry is left out"
		);
	})
	.await
}
