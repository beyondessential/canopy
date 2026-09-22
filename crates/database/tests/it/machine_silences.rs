//! A machine's checks are silenced against the machine, and that one silence
//! is read the same way everywhere.
//!
//! The three read-points have to agree: what the consolidated view presents as
//! skipped, what the reporting source is told not to run, and what an incident
//! counts. A silence that quiets a check in one and not the others is a defect
//! rather than a degree of silencing.
//!
//! spec: CHK#silences-follow-the-event

use commons_tests::db::TestDb;
use commons_types::status::CheckResult;
use database::{
	diesel_async::AsyncPgConnection,
	issues::{CheckFiling, Scope, file_check},
	silenced_refs::{MachineSilencedRef, is_silenced, silenced_health_checks_for_server},
};
use diesel_async::SimpleAsyncConnection;
use uuid::Uuid;

const SOURCE: &str = "alertd";
const CHECK: &str = "disk_free";
const REF: &str = "health/disk_free";

async fn seed(conn: &mut AsyncPgConnection) -> (Uuid, Uuid, Uuid) {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let application = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO server_groups (id, name) VALUES ('{group}', 'silence-group'); \
		 INSERT INTO machines (id, group_id) VALUES ('{machine}', '{group}'); \
		 INSERT INTO applications (id, host, type, group_id, machine_id) \
		 VALUES ('{application}', 'https://{application}.example', 'tamanu-central', \
		         '{group}', '{machine}')"
	))
	.await
	.expect("seed");
	(group, machine, application)
}

/// File a failing machine-subject check, as a report does.
async fn file_machine_failure(conn: &mut AsyncPgConnection, machine: Uuid) {
	file_check(
		conn,
		CheckFiling {
			source: SOURCE,
			scope: Scope::Machine(machine),
			device_id: None,
			check: CHECK,
			observed: CheckResult::Failed,
			title: None,
			message: "machine silence test",
			detail: None,
			default_ceiling: CheckResult::Failed,
			default_escalates: false,
			documentation: None,
		},
	)
	.await
	.expect("file the machine's check");
}

/// The same silence answers the same way at every point that reads one.
#[tokio::test(flavor = "multi_thread")]
async fn one_machine_silence_is_read_the_same_everywhere() {
	TestDb::run(async |mut conn, _url| {
		let (group, machine, application) = seed(&mut conn).await;
		file_machine_failure(&mut conn, machine).await;

		// Before: the check counts everywhere.
		let before = database::issues::consolidated_checks_latest_for_machine(
			&mut conn,
			machine,
			Some(group),
		)
		.await
		.expect("checks");
		assert!(
			before
				.checks
				.iter()
				.any(|c| c.check == CHECK && !c.silenced),
			"the check starts unsilenced: {:?}",
			before.checks
		);
		assert!(
			!is_silenced(&mut conn, Scope::Machine(machine), Some(group), SOURCE, REF)
				.await
				.expect("is_silenced"),
			"an unsilenced machine check counts toward an incident"
		);

		MachineSilencedRef::add(&mut conn, machine, SOURCE, REF, Some("op"))
			.await
			.expect("silence");

		// 1. The consolidated view presents it as skipped.
		let after = database::issues::consolidated_checks_latest_for_machine(
			&mut conn,
			machine,
			Some(group),
		)
		.await
		.expect("checks");
		let entry = after
			.checks
			.iter()
			.find(|c| c.check == CHECK)
			.expect("the check is still presented");
		assert!(entry.silenced, "presented as silenced");
		assert_eq!(entry.effective.to_string(), "skipped");

		// 2. The reporting source is told not to run it.
		let told = silenced_health_checks_for_server(
			&mut conn,
			Some(application),
			Some(machine),
			Some(group),
			SOURCE,
		)
		.await
		.expect("agent-facing set");
		assert!(
			told.contains(CHECK),
			"the agent is told to skip it: {told:?}"
		);

		// 3. An incident does not count it.
		assert!(
			is_silenced(&mut conn, Scope::Machine(machine), Some(group), SOURCE, REF)
				.await
				.expect("is_silenced"),
			"a machine silence keeps the check out of an incident"
		);
	})
	.await
}

/// A machine silence is the box's own. The applications on it are silenced
/// against themselves, so quieting a host does not quiet its workloads.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_silence_does_not_reach_the_applications_on_it() {
	TestDb::run(async |mut conn, _url| {
		let (group, machine, application) = seed(&mut conn).await;
		MachineSilencedRef::add(&mut conn, machine, SOURCE, REF, Some("op"))
			.await
			.expect("silence");

		assert!(
			!is_silenced(
				&mut conn,
				Scope::Application(application),
				Some(group),
				SOURCE,
				REF
			)
			.await
			.expect("is_silenced"),
			"the workload's own check is untouched by a silence on its box"
		);
	})
	.await
}

/// A group silence covers the machines in it, as it covers the applications.
#[tokio::test(flavor = "multi_thread")]
async fn a_group_silence_covers_a_machine_in_it() {
	TestDb::run(async |mut conn, _url| {
		let (group, machine, _) = seed(&mut conn).await;
		database::silenced_refs::ServerGroupSilencedRef::add(
			&mut conn,
			group,
			SOURCE,
			REF,
			// `disk_free` is the box's check, so there is no type to name.
			None,
			Some("op"),
		)
		.await
		.expect("group silence");

		assert!(
			is_silenced(&mut conn, Scope::Machine(machine), Some(group), SOURCE, REF)
				.await
				.expect("is_silenced"),
			"a group's silence reaches the boxes in it"
		);
	})
	.await
}

/// Listing a machine's silences returns its own, and unsilencing removes them.
#[tokio::test(flavor = "multi_thread")]
async fn a_machine_silence_lists_and_lifts() {
	TestDb::run(async |mut conn, _url| {
		let (_, machine, _) = seed(&mut conn).await;
		// A silence presents only for a check the catalog knows, so the check
		// has to have been reported once.
		file_machine_failure(&mut conn, machine).await;

		MachineSilencedRef::add(&mut conn, machine, SOURCE, REF, Some("op"))
			.await
			.expect("silence");
		let listed = MachineSilencedRef::list_for_machine(&mut conn, machine)
			.await
			.expect("list");
		assert_eq!(listed.len(), 1);
		assert_eq!(listed[0].machine_id, machine);
		assert_eq!(listed[0].r#ref, REF);
		assert_eq!(listed[0].created_by.as_deref(), Some("op"));

		MachineSilencedRef::remove(&mut conn, machine, SOURCE, REF)
			.await
			.expect("unsilence");
		assert!(
			MachineSilencedRef::list_for_machine(&mut conn, machine)
				.await
				.expect("list")
				.is_empty()
		);
	})
	.await
}
