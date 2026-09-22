//! How a reported application key correlates to the record Canopy holds: what
//! a held key answers, what a changed type does to it, and what carries a
//! machine's applications across the cutover from unified pushes.
//!
//! spec: STA#identifying-an-application

use commons_types::server::app_type::ApplicationType;
use database::{
	applications::Application,
	machines::{Machine, NewMachine},
};
use diesel_async::AsyncPgConnection;

async fn machine(conn: &mut AsyncPgConnection) -> Machine {
	Machine::create(conn, NewMachine::default()).await.unwrap()
}

/// The first push naming a key gets a record, and every push after it gets the
/// same one back.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_creates_once_and_answers_the_same_after() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = machine(&mut conn).await;

		let first = Application::from_report_key(
			&mut conn,
			&machine,
			"central",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.expect("a recording push creates what it names");
		assert_eq!(first.reported_key.as_deref(), Some("central"));

		let again = Application::from_report_key(
			&mut conn,
			&machine,
			"central",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.unwrap();
		assert_eq!(again.id, first.id);
	})
	.await
}

/// Two keys on one machine are two applications, which is the case the split
/// exists for: a box running two workloads reports both and neither is
/// attributed to the other.
#[tokio::test(flavor = "multi_thread")]
async fn two_keys_on_one_machine_are_two_applications() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = machine(&mut conn).await;

		let central = Application::from_report_key(
			&mut conn,
			&machine,
			"central",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.unwrap();
		let facility = Application::from_report_key(
			&mut conn,
			&machine,
			"facility",
			&ApplicationType::TamanuFacility,
			true,
		)
		.await
		.unwrap()
		.unwrap();

		assert_ne!(central.id, facility.id);
		assert_eq!(central.machine_id, Some(machine.id));
		assert_eq!(facility.machine_id, Some(machine.id));
	})
	.await
}

/// A key is the reporter's, so two machines may each name an application
/// `central` without colliding.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_is_unique_per_machine_not_per_fleet() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let one = machine(&mut conn).await;
		let two = machine(&mut conn).await;

		let a = Application::from_report_key(
			&mut conn,
			&one,
			"central",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.unwrap();
		let b = Application::from_report_key(
			&mut conn,
			&two,
			"central",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.unwrap();

		assert_ne!(a.id, b.id);
	})
	.await
}

/// Reporting a different type under a key already in use has stopped reporting
/// one application and started reporting another. The record that held the key
/// gives it up and stays as the application it is.
#[tokio::test(flavor = "multi_thread")]
async fn a_changed_type_under_a_held_key_is_a_different_application() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = machine(&mut conn).await;

		let was = Application::from_report_key(
			&mut conn,
			&machine,
			"app",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.unwrap();

		let now = Application::from_report_key(
			&mut conn,
			&machine,
			"app",
			&ApplicationType::Senaite,
			true,
		)
		.await
		.unwrap()
		.unwrap();

		assert_ne!(now.id, was.id);
		assert_eq!(now.r#type, ApplicationType::Senaite);
		assert_eq!(now.reported_key.as_deref(), Some("app"));

		let released = Application::get_by_id(&mut conn, was.id).await.unwrap();
		assert_eq!(released.r#type, ApplicationType::TamanuCentral);
		assert_eq!(
			released.reported_key, None,
			"the record that held the key gives it up rather than being rewritten"
		);
	})
	.await
}

/// The cutover: an application Canopy created from unified pushes has no key,
/// and the first split-shape push naming its type takes it over rather than
/// standing a duplicate up beside it.
#[tokio::test(flavor = "multi_thread")]
async fn a_key_claims_the_application_a_unified_push_left_unkeyed() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = machine(&mut conn).await;

		let unified =
			Application::from_report(&mut conn, &machine, &ApplicationType::TamanuCentral)
				.await
				.unwrap();
		assert_eq!(unified.reported_key, None);

		let claimed = Application::from_report_key(
			&mut conn,
			&machine,
			"central",
			&ApplicationType::TamanuCentral,
			true,
		)
		.await
		.unwrap()
		.unwrap();

		assert_eq!(
			claimed.id, unified.id,
			"the box is carried across, not doubled"
		);
		assert_eq!(claimed.reported_key.as_deref(), Some("central"));
	})
	.await
}

/// An ignored source reads what Canopy holds and changes nothing: no record is
/// created, and an application it could have claimed stays unclaimed.
#[tokio::test(flavor = "multi_thread")]
async fn an_ignored_source_neither_creates_nor_claims() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let machine = machine(&mut conn).await;

		let nothing = Application::from_report_key(
			&mut conn,
			&machine,
			"central",
			&ApplicationType::TamanuCentral,
			false,
		)
		.await
		.unwrap();
		assert!(nothing.is_none(), "nothing held, nothing created");

		let unified =
			Application::from_report(&mut conn, &machine, &ApplicationType::TamanuCentral)
				.await
				.unwrap();
		let read = Application::from_report_key(
			&mut conn,
			&machine,
			"central",
			&ApplicationType::TamanuCentral,
			false,
		)
		.await
		.unwrap()
		.expect("an application Canopy holds is answered for");
		assert_eq!(read.id, unified.id);

		let untouched = Application::get_by_id(&mut conn, unified.id).await.unwrap();
		assert_eq!(untouched.reported_key, None);
	})
	.await
}
