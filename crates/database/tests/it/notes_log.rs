//! The revision log of a group's, machine's and application's notes: every
//! change is a revision, a save that changes nothing is not, and a write that
//! states a stale revision is refused.

use commons_types::server::TagMap;
use database::{
	machines::{Machine, MachineUpdate, NewMachine},
	notes_log::{self, Author, NotesRecord, Outcome, Via},
	server_groups::{NewServerGroup, PartialServerGroup, ServerGroup},
};
use diesel_async::SimpleAsyncConnection as _;

fn new_group(name: &str, notes: &str) -> NewServerGroup {
	NewServerGroup {
		name: name.into(),
		notes: notes.into(),
		tags: TagMap::default(),
		slack_open_delay: None,
		slack_close_delay: None,
	}
}

fn only_notes(notes: &str) -> PartialServerGroup {
	PartialServerGroup {
		name: None,
		notes: Some(notes.into()),
		tags: None,
		slack_open_delay: None,
		slack_close_delay: None,
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn notes_a_group_is_created_with_start_its_log() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let group = ServerGroup::create_by(
			&mut conn,
			new_group("g", "where the key is"),
			Some(Author::operator("alice")),
		)
		.await
		.unwrap();
		assert_eq!(group.notes, "where the key is");

		let log = notes_log::history(&mut conn, NotesRecord::Group(group.id))
			.await
			.unwrap();
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].revision, 1);
		assert_eq!(log[0].notes, "where the key is");
		assert_eq!(log[0].author.as_deref(), Some("alice"));
		assert_eq!(log[0].via.as_deref(), Some("operator"));

		let empty = ServerGroup::create(&mut conn, new_group("e", ""))
			.await
			.unwrap();
		let current = notes_log::current(&mut conn, NotesRecord::Group(empty.id))
			.await
			.unwrap();
		assert_eq!((current.notes.as_str(), current.revision), ("", 0));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_are_revisions_and_a_save_that_changes_nothing_is_not() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let group = ServerGroup::create(&mut conn, new_group("g", "one"))
			.await
			.unwrap();
		let record = NotesRecord::Group(group.id);

		let same = ServerGroup::update_by(
			&mut conn,
			group.id,
			only_notes("one"),
			Some(Author::operator("bob")),
		)
		.await
		.unwrap();
		assert_eq!(same.notes, "one");
		assert_eq!(
			notes_log::current(&mut conn, record)
				.await
				.unwrap()
				.revision,
			1
		);

		ServerGroup::update_by(
			&mut conn,
			group.id,
			only_notes("two"),
			Some(Author::operator("bob")),
		)
		.await
		.unwrap();
		// Clearing is a change like any other.
		ServerGroup::update_by(
			&mut conn,
			group.id,
			only_notes(""),
			Some(Author::ansible_import("bob")),
		)
		.await
		.unwrap();

		let log = notes_log::history(&mut conn, record).await.unwrap();
		let seen: Vec<_> = log
			.iter()
			.map(|r| (r.revision, r.notes.as_str(), r.via.as_deref()))
			.collect();
		assert_eq!(
			seen,
			vec![
				(3, "", Some("ansible_import")),
				(2, "two", Some("operator")),
				(1, "one", None),
			]
		);
		assert_eq!(
			ServerGroup::get_by_id(&mut conn, group.id)
				.await
				.unwrap()
				.notes,
			""
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_against_a_stale_revision_is_refused_with_what_is_current() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let record = NotesRecord::Machine(machine.id);
		let agent = Author::token("agent-1");

		let first = notes_log::set(&mut conn, record, "a", Some(agent), Some(0))
			.await
			.unwrap();
		assert_eq!(first, Outcome::Changed { revision: 1 });

		let stale = notes_log::set(&mut conn, record, "b", Some(agent), Some(0))
			.await
			.unwrap();
		match stale {
			Outcome::Stale(current) => {
				assert_eq!((current.notes.as_str(), current.revision), ("a", 1));
			}
			other => panic!("expected stale, got {other:?}"),
		}
		assert!(
			Outcome::Stale(notes_log::current(&mut conn, record).await.unwrap())
				.or_conflict()
				.is_err()
		);

		let again = notes_log::set(&mut conn, record, "b", Some(agent), Some(1))
			.await
			.unwrap();
		assert_eq!(again, Outcome::Changed { revision: 2 });
		let log = notes_log::history(&mut conn, record).await.unwrap();
		assert_eq!(log[0].via.as_deref(), Some(Via::Token.as_str()));
		assert_eq!(log[0].author.as_deref(), Some("agent-1"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_machine_edit_goes_through_the_log() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let updated = Machine::update_by(
			&mut conn,
			machine.id,
			MachineUpdate {
				notes: Some("rack 4".into()),
				..Default::default()
			},
			Some(Author::operator("carol")),
		)
		.await
		.unwrap();
		assert_eq!(updated.notes, "rack 4");
		let log = notes_log::history(&mut conn, NotesRecord::Machine(machine.id))
			.await
			.unwrap();
		assert_eq!(log.len(), 1);
		assert_eq!(log[0].author.as_deref(), Some("carol"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_revision_belongs_to_exactly_one_record() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let group = ServerGroup::create(&mut conn, new_group("g", ""))
			.await
			.unwrap();
		let machine = Machine::create(&mut conn, NewMachine::named("box"))
			.await
			.unwrap();
		let both = conn
			.batch_execute(&format!(
				"INSERT INTO notes_revisions (server_group_id, machine_id, revision, notes) \
				 VALUES ('{}', '{}', 1, 'x')",
				group.id, machine.id
			))
			.await;
		assert!(both.is_err(), "two grains in one revision is refused");
		let neither = conn
			.batch_execute("INSERT INTO notes_revisions (revision, notes) VALUES (1, 'x')")
			.await;
		assert!(neither.is_err(), "no grain is refused");
	})
	.await
}
