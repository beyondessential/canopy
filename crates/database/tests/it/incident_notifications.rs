//! Incident notifications summarise the incident as it stands when they are
//! sent, and a notified incident left open sends a reminder each whole day.

use commons_types::status::CheckResult;
use database::issues::{Incident, NewEvent};
use database::slack_outbox::{
	KIND_INCIDENT_OPEN, KIND_INCIDENT_REMINDER, KIND_INCIDENT_RESOLVE, SlackOutbox,
};
use diesel::prelude::*;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

/// An application in a fresh group with the given linger window (an SQL
/// interval literal).
async fn insert_grouped_server(
	conn: &mut diesel_async::AsyncPgConnection,
	linger: &str,
) -> (Uuid, Uuid) {
	let group: RowId = sql_query(format!(
		"INSERT INTO server_groups (name, slack_close_delay) \
		 VALUES ('site', INTERVAL '{linger}') RETURNING id"
	))
	.get_result(conn)
	.await
	.expect("group");
	let row: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id) \
		 INSERT INTO applications (type, name, group_id, machine_id) \
		 SELECT 'tamanu-central', 'central-1', $1, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group.id)
	.get_result(conn)
	.await
	.expect("server");
	(group.id, row.id)
}

async fn file(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	check: &str,
	description: Option<&str>,
	result: CheckResult,
	escalates: bool,
) {
	let stamp = database::issues::CheckStateStamp {
		check: check.into(),
		observed: result,
		effective: result,
		escalates,
		detail: None,
		title: None,
		instanced: None,
	};
	NewEvent {
		source: "test".into(),
		r#ref: check.into(),
		description: description.map(Into::into),
		message: format!("{check} body"),
		active: Some(!matches!(result, CheckResult::Passed)),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), false)
	.await
	.expect("save event");
}

async fn the_open_incident(conn: &mut diesel_async::AsyncPgConnection, group_id: Uuid) -> Incident {
	use database::schema::incidents::dsl;
	dsl::incidents
		.select(Incident::as_select())
		.filter(dsl::server_group_id.eq(group_id))
		.filter(dsl::closed_at.is_null())
		.first(conn)
		.await
		.expect("open incident")
}

async fn rows_of(conn: &mut diesel_async::AsyncPgConnection, kind: &str) -> Vec<SlackOutbox> {
	use database::schema::slack_outbox::dsl;
	dsl::slack_outbox
		.select(SlackOutbox::as_select())
		.filter(dsl::kind.eq(kind))
		.order(dsl::created_at.asc())
		.load(conn)
		.await
		.expect("load outbox")
}

async fn deliver_open(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) {
	sql_query("UPDATE slack_outbox SET delivered_at = NOW() WHERE incident_id = $1 AND kind = $2")
		.bind::<sql_types::Uuid, _>(incident_id)
		.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
		.execute(conn)
		.await
		.expect("deliver open");
}

/// Make the incident `hours` old.
async fn age(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid, hours: i32) {
	sql_query("UPDATE incidents SET opened_at = NOW() - make_interval(hours => $2) WHERE id = $1")
		.bind::<sql_types::Uuid, _>(incident_id)
		.bind::<sql_types::Integer, _>(hours)
		.execute(conn)
		.await
		.expect("age incident");
}

async fn sweep(conn: &mut diesel_async::AsyncPgConnection) -> usize {
	database::issues::enqueue_due_reminders(conn)
		.await
		.expect("reminder sweep")
}

/// Issues that joined while the opening waited out its grace appear in it, so
/// the channel hears about the incident as it is, not as it began.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn opening_is_rendered_from_the_incident_when_sent() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (_, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		file(
			&mut conn,
			server,
			"disk",
			Some("Disk nearly full"),
			CheckResult::Warning,
			false,
		)
		.await;
		file(
			&mut conn,
			server,
			"db-down",
			Some("Postgres down"),
			CheckResult::Failed,
			false,
		)
		.await;
		let [open] = rows_of(&mut conn, KIND_INCIDENT_OPEN)
			.await
			.try_into()
			.unwrap();
		assert_eq!(
			open.payload["source_ref"], "1 failed",
			"the queued snapshot only knew the issue that opened it"
		);

		let payload = open.render_current(&mut conn).await.expect("render");
		assert_eq!(payload["server"], "site");
		assert_eq!(payload["severity"], "Error");
		assert_eq!(payload["source_ref"], "2 failed, 1 warning");
		assert_eq!(
			payload["message"],
			"• Failed: Postgres down on central-1\n\
			 • Failed: app-down on central-1\n\
			 • Warning: Disk nearly full on central-1",
			"worst first, most recent first among equals, headline else check",
		);
		assert!(payload.get("link").is_none(), "the drainer adds the link");
	})
	.await
}

/// An issue that left before the opening was sent is not in it: the summary is
/// of live members.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn summary_lists_live_members_only() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (_, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		file(&mut conn, server, "flaky", None, CheckResult::Failed, false).await;
		file(&mut conn, server, "flaky", None, CheckResult::Passed, false).await;

		let [open] = rows_of(&mut conn, KIND_INCIDENT_OPEN)
			.await
			.try_into()
			.unwrap();
		let payload = open.render_current(&mut conn).await.expect("render");
		assert_eq!(payload["source_ref"], "1 failed");
		assert_eq!(payload["message"], "• Failed: app-down on central-1");
	})
	.await
}

/// An escalation summarises the incident too, at critical severity.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn escalation_summarises_the_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		let incident = the_open_incident(&mut conn, group).await;
		deliver_open(&mut conn, incident.id).await;
		file(
			&mut conn,
			server,
			"data-loss",
			None,
			CheckResult::Failed,
			true,
		)
		.await;

		let opens = rows_of(&mut conn, KIND_INCIDENT_OPEN).await;
		assert_eq!(opens.len(), 2, "the opening, then the escalation");
		let payload = opens[1].render_current(&mut conn).await.expect("render");
		assert_eq!(payload["severity"], "Critical");
		assert_eq!(payload["source_ref"], "2 failed");
	})
	.await
}

/// A notified incident open a day gets a reminder, once per day however often
/// the sweep runs, leading with how long it has been open.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn reminder_is_queued_once_per_day() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		let incident = the_open_incident(&mut conn, group).await;
		deliver_open(&mut conn, incident.id).await;

		age(&mut conn, incident.id, 23).await;
		assert_eq!(sweep(&mut conn).await, 0, "not yet a day");

		age(&mut conn, incident.id, 25).await;
		assert_eq!(sweep(&mut conn).await, 1);
		assert_eq!(sweep(&mut conn).await, 0, "already reminded for day one");

		let [reminder] = rows_of(&mut conn, KIND_INCIDENT_REMINDER)
			.await
			.try_into()
			.unwrap();
		assert_eq!(reminder.incident_id, Some(incident.id));
		assert!(reminder.deliver_after <= jiff::Timestamp::now());
		let payload = reminder.render_current(&mut conn).await.expect("render");
		assert_eq!(payload["server"], "site");
		assert_eq!(payload["source_ref"], "1 failed");
		assert_eq!(
			payload["message"],
			"Open for 1 day\n\n• Failed: app-down on central-1"
		);

		age(&mut conn, incident.id, 49).await;
		assert_eq!(sweep(&mut conn).await, 1, "another day, another reminder");
		let reminders = rows_of(&mut conn, KIND_INCIDENT_REMINDER).await;
		let payload = reminders[1]
			.render_current(&mut conn)
			.await
			.expect("render");
		assert!(
			payload["message"]
				.as_str()
				.unwrap()
				.starts_with("Open for 2 days\n\n")
		);

		age(&mut conn, incident.id, 24 * 5 + 1).await;
		assert_eq!(
			sweep(&mut conn).await,
			1,
			"missed days catch up with one reminder, not a backlog"
		);
		assert_eq!(sweep(&mut conn).await, 0);

		assert!(
			rows_of(&mut conn, KIND_INCIDENT_OPEN).await.len() == 1,
			"a reminder is not an opening"
		);
	})
	.await
}

/// An incident whose opening was never delivered sends no reminders.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn unnotified_incident_is_not_reminded() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		let incident = the_open_incident(&mut conn, group).await;
		age(&mut conn, incident.id, 25).await;
		assert_eq!(sweep(&mut conn).await, 0);
		assert!(rows_of(&mut conn, KIND_INCIDENT_REMINDER).await.is_empty());
	})
	.await
}

/// A reminder falling due while the incident lingers waits: it ships if a
/// failure returns, and is cancelled if the incident closes. The resolve is
/// still owed, because the opening was delivered.
// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn reminder_waits_out_lingering() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		let incident = the_open_incident(&mut conn, group).await;
		deliver_open(&mut conn, incident.id).await;
		age(&mut conn, incident.id, 25).await;

		// Recovers, so the incident lingers; then the reminder falls due.
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Passed,
			false,
		)
		.await;
		assert_eq!(sweep(&mut conn).await, 1);
		let claimed = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert!(
			claimed.iter().all(|r| r.kind != KIND_INCIDENT_REMINDER),
			"a reminder doesn't ship while its incident lingers"
		);

		// The failure returns: the reminder can ship.
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		let claimed = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert!(claimed.iter().any(|r| r.kind == KIND_INCIDENT_REMINDER));
	})
	.await
}

// spec: INC#notification
#[tokio::test(flavor = "multi_thread")]
async fn closing_cancels_a_pending_reminder() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group, server) = insert_grouped_server(&mut conn, "5 minutes").await;
		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Failed,
			false,
		)
		.await;
		let incident = the_open_incident(&mut conn, group).await;
		deliver_open(&mut conn, incident.id).await;
		age(&mut conn, incident.id, 25).await;

		file(
			&mut conn,
			server,
			"app-down",
			None,
			CheckResult::Passed,
			false,
		)
		.await;
		assert_eq!(sweep(&mut conn).await, 1);

		sql_query("UPDATE incidents SET closing_at = closing_at - INTERVAL '1 hour' WHERE id = $1")
			.bind::<sql_types::Uuid, _>(incident.id)
			.execute(&mut conn)
			.await
			.expect("expire linger");
		assert_eq!(
			database::issues::sweep_lingering_incidents(&mut conn)
				.await
				.expect("linger sweep"),
			1
		);

		let [reminder] = rows_of(&mut conn, KIND_INCIDENT_REMINDER)
			.await
			.try_into()
			.unwrap();
		assert!(reminder.gave_up_at.is_some(), "the reminder never ships");
		assert!(reminder.delivered_at.is_none());
		assert_eq!(
			rows_of(&mut conn, KIND_INCIDENT_RESOLVE).await.len(),
			1,
			"the channel saw the opening, so it still hears the close"
		);
		assert_eq!(
			sweep(&mut conn).await,
			0,
			"a closed incident isn't reminded"
		);
	})
	.await
}
