//! The audit log's storage and model (spec `AUD`): the retention parts of a
//! week, settling an entry once, and what the table refuses.

use commons_types::server::rank::ServerRank;
use database::audit::{self, Action, Actor, Filter, Outcome, Record, RecordKind, Status};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::{AsyncPgConnection, RunQueryDsl, SimpleAsyncConnection};
use serde_json::json;
use uuid::Uuid;

fn operator() -> Actor {
	Actor::operator("a@example.org", None)
}

fn lifted() -> Action {
	Action::MaintenanceLifted {
		window: Record::new(RecordKind::MaintenanceWindow, Uuid::new_v4(), "window"),
		target: vec![Record::new(RecordKind::Machine, Uuid::new_v4(), "machine")],
		rank: None,
		already_ended: false,
	}
}

fn withdrawn() -> Action {
	Action::UpgradePlanWithdrawn {
		plan: Record::new(RecordKind::UpgradePlan, Uuid::new_v4(), "plan"),
		group: Record::new(RecordKind::Group, Uuid::new_v4(), "group"),
		rank: ServerRank::Production,
	}
}

async fn find(conn: &mut AsyncPgConnection) -> Vec<audit::Entry> {
	audit::find(
		conn,
		&Filter {
			include_routine: true,
			limit: 100,
			..Default::default()
		},
	)
	.await
	.unwrap()
}

#[tokio::test(flavor = "multi_thread")]
async fn an_action_marked_for_a_year_lands_in_the_year_part_of_its_week() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		#[derive(QueryableByName)]
		struct Part {
			#[diesel(sql_type = sql_types::Text)]
			part: String,
		}

		audit::record(&mut conn, &operator(), &withdrawn())
			.await
			.unwrap();
		audit::record(&mut conn, &operator(), &lifted())
			.await
			.unwrap();

		let parts: Vec<Part> =
			sql_query("SELECT tableoid::regclass::text AS part FROM audit_entries ORDER BY action")
				.load(&mut conn)
				.await
				.unwrap();
		assert!(parts[0].part.ends_with("_standard"), "{}", parts[0].part);
		assert!(parts[1].part.ends_with("_year"), "{}", parts[1].part);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_pending_entry_is_settled_once_and_only_adds_to_its_detail() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let pending = audit::commit_ahead(&mut conn, &operator(), &lifted())
			.await
			.unwrap();
		assert!(
			find(&mut conn).await.is_empty(),
			"hidden while it can be settled"
		);

		audit::settle(
			&mut conn,
			&pending,
			Outcome::Done,
			Some(json!({ "learned": 1 })),
		)
		.await
		.unwrap();
		let entries = find(&mut conn).await;
		assert_eq!(entries.len(), 1);
		assert_eq!(entries[0].status, Status::Done);
		assert_eq!(entries[0].detail["learned"], 1);
		assert_eq!(entries[0].detail["already_ended"], false);

		assert!(
			audit::settle(&mut conn, &pending, Outcome::Failed, None)
				.await
				.is_err(),
			"an entry already settled cannot be settled again"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_lapsed_entry_cannot_be_settled_and_reads_as_unconfirmed() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		conn.batch_execute(
			"INSERT INTO audit_entries (id, recorded_at, actor_kind, actor_id, action, outcome) \
			 VALUES ('aaaaaaaa-0000-0000-0000-000000000001', \
			         GREATEST(now() - interval '2 hours', date_trunc('week', now())), \
			         'operator', 'a@example.org', 'maintenance_lifted', 'pending')",
		)
		.await
		.unwrap();
		let entries = find(&mut conn).await;
		// Within an hour of the week's start the row cannot be made old enough
		// without leaving the week provisioned; it is then still settleable.
		if let [entry, ..] = entries.as_slice() {
			assert_eq!(entry.status, Status::Unconfirmed);
			let pending = audit::Pending {
				id: entry.id,
				recorded_at: entry.recorded_at,
				retention: entry.retention,
			};
			assert!(
				audit::settle(&mut conn, &pending, Outcome::Done, None)
					.await
					.is_err()
			);
		}
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn the_table_refuses_deletes_and_changes_to_anything_but_settling() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		audit::record(&mut conn, &operator(), &lifted())
			.await
			.unwrap();
		let pending = audit::commit_ahead(&mut conn, &operator(), &lifted())
			.await
			.unwrap();

		for statement in [
			"DELETE FROM audit_entries",
			"UPDATE audit_entries SET action = 'other'",
			"UPDATE audit_entries SET outcome = 'done' WHERE outcome IS NULL",
			"UPDATE audit_entries SET outcome = 'pending'",
			"UPDATE audit_entries SET outcome = 'done', detail = '{}' WHERE outcome = 'pending'",
		] {
			assert!(
				conn.batch_execute(statement).await.is_err(),
				"{statement} should be refused"
			);
		}
		// Settling still works after all that.
		audit::settle(&mut conn, &pending, Outcome::Failed, None)
			.await
			.unwrap();
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn entries_are_found_by_record_and_detail_and_counted_by_a_detail_value() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let window = Uuid::new_v4();
		for (note, window) in [
			(Some("alpha"), window),
			(Some("alpha"), Uuid::new_v4()),
			(Some("beta"), Uuid::new_v4()),
			// Written as a null note, which is no value to count.
			(None, Uuid::new_v4()),
		] {
			audit::record(
				&mut conn,
				&operator(),
				&Action::MaintenanceAmended {
					window: Record::new(RecordKind::MaintenanceWindow, window, "w"),
					target: vec![Record::new(RecordKind::Machine, Uuid::new_v4(), "m")],
					moved_from: Vec::new(),
					rank: None,
					expected_end: jiff::Timestamp::now(),
					note: note.map(str::to_owned),
				},
			)
			.await
			.unwrap();
		}

		let by_record = audit::find(
			&mut conn,
			&Filter {
				acted_on: Some(window.to_string()),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(by_record.len(), 1);

		let by_detail = audit::find(
			&mut conn,
			&Filter {
				detail: Some(json!({ "note": "alpha" })),
				limit: 10,
				..Default::default()
			},
		)
		.await
		.unwrap();
		assert_eq!(by_detail.len(), 2);

		let counts = audit::count_by_detail(
			&mut conn,
			"maintenance_amended",
			"note",
			jiff::Timestamp::now() - jiff::SignedDuration::from_hours(1),
			jiff::Timestamp::now() + jiff::SignedDuration::from_hours(1),
		)
		.await
		.unwrap();
		assert_eq!(counts.get("alpha"), Some(&2));
		assert_eq!(counts.get("beta"), Some(&1));
		assert_eq!(counts.len(), 2, "{counts:?}");
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_job_records_as_canopy_through_the_same_recorder() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		audit::record(&mut conn, &Actor::canopy("sweeper"), &lifted())
			.await
			.unwrap();
		let entries = find(&mut conn).await;
		assert!(matches!(&entries[0].actor, Actor::Canopy { job } if job == "sweeper"));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn every_action_in_the_catalogue_is_unique_and_labelled() {
	let mut names: Vec<_> = audit::CATALOGUE.to_vec();
	names.sort();
	let count = names.len();
	names.dedup();
	assert_eq!(names.len(), count, "an action name is used once");
	for (name, label, _) in audit::catalogue() {
		assert!(!label.is_empty() && !label.contains('—'), "{name}");
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn every_kind_of_actor_is_presented_as_what_it_is() {
	use commons_types::device::DeviceRole;

	commons_tests::db::TestDb::run(async |mut conn, _| {
		let application = Uuid::new_v4();
		let device = |role| Actor::Device {
			id: Uuid::new_v4(),
			name: None,
			role,
			application: Some(application),
		};
		let token = Actor::Token {
			id: Uuid::new_v4(),
			name: "ops-assistant".into(),
		};
		for actor in [
			device(DeviceRole::Relay),
			device(DeviceRole::BackupRestore),
			token.clone(),
		] {
			audit::record(&mut conn, &actor, &lifted()).await.unwrap();
		}

		let entries = find(&mut conn).await;
		let roles: Vec<DeviceRole> = entries
			.iter()
			.filter_map(|entry| match &entry.actor {
				Actor::Device {
					role,
					application: found,
					..
				} => {
					assert_eq!(*found, Some(application), "the application it acted for");
					Some(*role)
				}
				_ => None,
			})
			.collect();
		assert!(roles.contains(&DeviceRole::Relay));
		assert!(roles.contains(&DeviceRole::BackupRestore));
		assert!(entries.iter().any(
			|entry| matches!(&entry.actor, Actor::Token { name, .. } if name == "ops-assistant")
		));
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn a_record_is_found_by_part_of_its_name_through_the_names_index() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		for (window, machine) in [
			("Kamaka central", "kamaka-1"),
			("Fiji 50%_off", "suva-2"),
			("Tonga", "nuku\nalofa"),
		] {
			audit::record(
				&mut conn,
				&operator(),
				&Action::MaintenanceLifted {
					window: Record::new(RecordKind::MaintenanceWindow, Uuid::new_v4(), window),
					target: vec![Record::new(RecordKind::Machine, Uuid::new_v4(), machine)],
					rank: None,
					already_ended: false,
				},
			)
			.await
			.unwrap();
		}

		let by_name = async |conn: &mut AsyncPgConnection, name: &str| {
			audit::find(
				conn,
				&Filter {
					acted_on: Some(name.to_owned()),
					limit: 10,
					..Default::default()
				},
			)
			.await
			.unwrap()
			.len()
		};
		assert_eq!(
			by_name(&mut conn, "KAMAKA").await,
			1,
			"case does not matter"
		);
		assert_eq!(
			by_name(&mut conn, "suva").await,
			1,
			"any of an entry's records"
		);
		assert_eq!(by_name(&mut conn, "50%_").await, 1, "taken literally");
		assert_eq!(by_name(&mut conn, "0%_x").await, 0);
		assert_eq!(
			by_name(&mut conn, "central\nkamaka").await,
			0,
			"a search never spans two names"
		);

		#[derive(QueryableByName)]
		struct Plan {
			#[diesel(sql_type = sql_types::Text, column_name = "QUERY PLAN")]
			line: String,
		}
		conn.batch_execute("SET enable_seqscan = off")
			.await
			.unwrap();
		let plan: Vec<Plan> = sql_query(
			"EXPLAIN SELECT id FROM audit_entries WHERE audit_record_names(records) ILIKE '%kamaka%'",
		)
		.load(&mut conn)
		.await
		.unwrap();
		assert!(
			plan.iter().any(|row| row.line.contains("record_names")),
			"{}",
			plan.iter()
				.map(|row| row.line.as_str())
				.collect::<Vec<_>>()
				.join("\n")
		);
	})
	.await
}
