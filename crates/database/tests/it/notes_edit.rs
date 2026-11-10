//! Notes on issues and incidents: written by an operator or an agent, edited
//! with the replaced text kept, and deleted without being lost.

use database::notes::{AuthorKind, Edited, IncidentNote, IssueNote};
use diesel_async::SimpleAsyncConnection as _;
use uuid::Uuid;

async fn seed(conn: &mut diesel_async::AsyncPgConnection) -> (Uuid, Uuid) {
	let group_id = Uuid::new_v4();
	let machine_id = Uuid::new_v4();
	let device_id = Uuid::new_v4();
	let issue_id = Uuid::new_v4();
	let incident_id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{device_id}', 'machine'); \
		 INSERT INTO server_groups (id, name) VALUES ('{group_id}', 'g'); \
		 INSERT INTO machines (name, id, group_id, device_id) VALUES ('box', '{machine_id}', '{group_id}', '{device_id}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
			('{machine_id}', 'https://example.com', 'tamanu-central', '{group_id}', 'production', '{machine_id}'); \
		 INSERT INTO issues \
			(id, application_id, device_id, source, ref, check_name, observed_result, effective_result, message, active, first_seen, last_seen) \
		   VALUES ('{issue_id}', '{machine_id}', '{device_id}', 'test', 'r', 'r', 'failed', 'failed', 'm', true, NOW(), NOW()); \
		 INSERT INTO incidents (id, server_group_id, rank, opened_at) \
		   VALUES ('{incident_id}', '{group_id}', 'production', NOW());"
	))
	.await
	.expect("seed");
	(issue_id, incident_id)
}

#[tokio::test(flavor = "multi_thread")]
async fn an_agent_note_is_marked_and_an_edit_keeps_what_it_replaced() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (issue_id, _) = seed(&mut conn).await;
		let note = IssueNote::add(&mut conn, issue_id, "agent-1", AuthorKind::Agent, "first")
			.await
			.unwrap();
		assert_eq!((note.author_kind, note.revision), (AuthorKind::Agent, 1));

		let Edited::Changed(edited) = IssueNote::edit(&mut conn, note.id, 1, "second", "agent-2")
			.await
			.unwrap()
		else {
			panic!("expected the edit to apply");
		};
		assert_eq!((edited.body.as_str(), edited.revision), ("second", 2));

		let history = IssueNote::edit_history(&mut conn, note.id).await.unwrap();
		assert_eq!(history.len(), 1);
		assert_eq!(
			(
				history[0].revision,
				history[0].body.as_str(),
				history[0].edited_by.as_str()
			),
			(1, "first", "agent-2")
		);

		assert!(matches!(
			IssueNote::edit(&mut conn, note.id, 2, "second", "agent-2")
				.await
				.unwrap(),
			Edited::Unchanged(_)
		));
		let Edited::Stale(current) = IssueNote::edit(&mut conn, note.id, 1, "third", "agent-2")
			.await
			.unwrap()
		else {
			panic!("expected a stale refusal");
		};
		assert_eq!((current.body.as_str(), current.revision), ("second", 2));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_deleted_note_leaves_the_lists_but_is_kept() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (issue_id, incident_id) = seed(&mut conn).await;
		let keep = IncidentNote::add(&mut conn, incident_id, "op", AuthorKind::Operator, "keep")
			.await
			.unwrap();
		let drop_me = IncidentNote::add(&mut conn, incident_id, "bot", AuthorKind::Agent, "drop")
			.await
			.unwrap();

		let deleted = IncidentNote::delete(&mut conn, drop_me.id, "op")
			.await
			.unwrap()
			.expect("the note was there to delete");
		assert_eq!(deleted.deleted_by.as_deref(), Some("op"));
		assert!(deleted.deleted_at.is_some());
		assert!(
			IncidentNote::delete(&mut conn, drop_me.id, "op")
				.await
				.unwrap()
				.is_none(),
			"deleting twice has nothing more to record"
		);

		let listed = IncidentNote::list_for_incident(&mut conn, incident_id, 10, false)
			.await
			.unwrap();
		assert_eq!(
			listed.iter().map(|n| n.id).collect::<Vec<_>>(),
			vec![keep.id]
		);
		let all = IncidentNote::list_for_incident(&mut conn, incident_id, 10, true)
			.await
			.unwrap();
		assert_eq!(all.len(), 2);

		assert!(
			IncidentNote::edit(&mut conn, drop_me.id, 1, "revived", "bot")
				.await
				.is_err(),
			"a deleted note cannot be edited"
		);

		let issue_note = IssueNote::add(&mut conn, issue_id, "op", AuthorKind::Operator, "x")
			.await
			.unwrap();
		IssueNote::delete(&mut conn, issue_note.id, "op")
			.await
			.unwrap();
		assert!(
			IssueNote::list_for_issue(&mut conn, issue_id, 10, false)
				.await
				.unwrap()
				.is_empty()
		);
		assert_eq!(
			IssueNote::list_for_issue(&mut conn, issue_id, 10, true)
				.await
				.unwrap()
				.len(),
			1
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn deleted_notes_are_not_counted() {
	commons_tests::db::TestDb::run(async |mut conn, url| {
		let (issue_id, incident_id) = seed(&mut conn).await;
		conn.batch_execute(&format!(
			"INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ('{incident_id}', '{issue_id}', NOW())"
		))
		.await
		.unwrap();
		let kept = IncidentNote::add(&mut conn, incident_id, "op", AuthorKind::Operator, "a")
			.await
			.unwrap();
		let gone = IssueNote::add(&mut conn, issue_id, "op", AuthorKind::Operator, "b")
			.await
			.unwrap();
		IssueNote::delete(&mut conn, gone.id, "op").await.unwrap();
		let _ = kept;
		drop(conn);

		let pool = database::init_to(&url);
		let stats = database::issues::Incident::stats_for(&pool, &[incident_id])
			.await
			.unwrap();
		assert_eq!(stats[&incident_id].note_count, 1);
	})
	.await
}
