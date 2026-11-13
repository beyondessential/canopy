//! Alert triage: what triage reads of an incident, what has changed since its
//! last verdict, recording verdicts against a revision, the write-up day, and
//! the self-alert for incidents left untriaged.

use database::{
	alert_triage::{self, Assessment, CheckKey, NewVerdict, Recorded, Severity, Verdict},
	memory::{self, Link, NewEntry},
	notes::AuthorKind,
	notes_log::Author,
	self_alerts,
};
use diesel_async::{AsyncPgConnection, SimpleAsyncConnection as _};
use jiff::Timestamp;
use uuid::Uuid;

/// A group with one box carrying one application at `rank`, the application
/// sharing the machine's id.
async fn group(conn: &mut AsyncPgConnection, name: &str, rank: &str) -> (Uuid, Uuid) {
	let group = Uuid::new_v4();
	let machine = Uuid::new_v4();
	let device = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO devices (id, role) VALUES ('{device}', 'machine'); \
		 INSERT INTO server_groups (id, name) VALUES ('{group}', '{name}'); \
		 INSERT INTO machines (name, id, group_id, device_id) VALUES ('{name}-box', '{machine}', '{group}', '{device}'); \
		 INSERT INTO applications (id, host, type, group_id, rank, machine_id) VALUES \
			('{machine}', 'https://{name}.example.com', 'tamanu-central', '{group}', '{rank}', '{machine}');"
	))
	.await
	.expect("seed group");
	(group, machine)
}

async fn incident(conn: &mut AsyncPgConnection, group: Uuid, rank: &str) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO incidents (id, server_group_id, rank, opened_at) VALUES ('{id}', '{group}', '{rank}', NOW());"
	))
	.await
	.expect("seed incident");
	id
}

/// A failing member of `incident` on `application`, its failure begun
/// `minutes_ago`.
async fn failing(
	conn: &mut AsyncPgConnection,
	incident: Uuid,
	application: Uuid,
	check: &str,
	escalates: bool,
	minutes_ago: i64,
) -> Uuid {
	member(
		conn,
		incident,
		application,
		check,
		"failed",
		escalates,
		minutes_ago,
	)
	.await
}

async fn member(
	conn: &mut AsyncPgConnection,
	incident: Uuid,
	application: Uuid,
	check: &str,
	result: &str,
	escalates: bool,
	minutes_ago: i64,
) -> Uuid {
	let issue = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO issues \
			(id, application_id, source, ref, check_name, observed_result, effective_result, \
			 message, active, first_seen, last_seen, escalates, degraded_since) \
		 VALUES ('{issue}', '{application}', 'alertd', '{check}-{issue}', '{check}', '{result}', '{result}', \
			'm', true, NOW(), NOW(), {escalates}, NOW() - INTERVAL '{minutes_ago} minutes'); \
		 INSERT INTO incident_issues (incident_id, issue_id, joined_at) VALUES ('{incident}', '{issue}', NOW());"
	))
	.await
	.expect("seed member");
	issue
}

/// Catalogue a check with documentation.
async fn documented(conn: &mut AsyncPgConnection, check: &str) {
	conn.batch_execute(&format!(
		"INSERT INTO check_policies (source, check_name, ceiling, escalates, documentation) \
		 VALUES ('alertd', '{check}', 'failed', false, 'What {check} means');"
	))
	.await
	.expect("seed policy");
}

async fn published(conn: &mut AsyncPgConnection, incident: Uuid, minutes_ago: i64) {
	conn.batch_execute(&format!(
		"INSERT INTO slack_outbox (kind, incident_id, payload, delivered_at) \
		 VALUES ('incident_open', '{incident}', '{{}}', NOW() - INTERVAL '{minutes_ago} minutes');"
	))
	.await
	.expect("seed notification");
}

fn verdict(a: &Assessment, verdict: Verdict, entries: Vec<Uuid>) -> NewVerdict {
	NewVerdict {
		incident_id: a.incident.id,
		revision: a.state.revision.clone(),
		verdict,
		reasoning: "because".into(),
		memory_entry_ids: entries,
	}
}

async fn record(conn: &mut AsyncPgConnection, new: &NewVerdict) -> Recorded {
	alert_triage::record(conn, new, "triage-bot", AuthorKind::Agent)
		.await
		.expect("record")
}

fn key(name: &str) -> CheckKey {
	CheckKey {
		source: "alertd".into(),
		name: name.into(),
	}
}

#[tokio::test(flavor = "multi_thread")]
async fn triage_reads_the_incidents_checks_documentation_and_memory() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let (other_group, _) = group(&mut conn, "Highlands", "test").await;
		let i = incident(&mut conn, g, "test").await;
		failing(&mut conn, i, app, "disk", true, 5).await;
		failing(&mut conn, i, app, "mystery", false, 5).await;
		member(&mut conn, i, app, "cpu", "warning", false, 5).await;

		let author = Author::token("triage-bot");
		let entry = |title: &str, link| NewEntry {
			title: title.into(),
			body: "body".into(),
			link: Some(link),
		};
		let on_check = memory::record(
			&mut conn,
			entry(
				"disk",
				Link::Check {
					source: "alertd".into(),
					name: "disk".into(),
				},
			),
			author,
		)
		.await
		.unwrap();
		conn.batch_execute(
			"INSERT INTO check_policies (source, check_name, ceiling, escalates) \
			 VALUES ('elsewhere', 'disk', 'failed', false);",
		)
		.await
		.unwrap();
		memory::record(
			&mut conn,
			entry(
				"other source",
				Link::Check {
					source: "elsewhere".into(),
					name: "disk".into(),
				},
			),
			author,
		)
		.await
		.unwrap();
		let on_group = memory::record(&mut conn, entry("site", Link::Group { id: g }), author)
			.await
			.unwrap();
		memory::record(
			&mut conn,
			entry("another site", Link::Group { id: other_group }),
			author,
		)
		.await
		.unwrap();
		let retired = memory::record(&mut conn, entry("old", Link::Incident { id: i }), author)
			.await
			.unwrap();
		memory::retire(&mut conn, retired.id, "stale", author)
			.await
			.unwrap();

		let a = alert_triage::assess(&mut conn, i).await.unwrap();
		assert_eq!(a.target, "Pacific test");
		assert_eq!(a.state.worst, Some(Severity::EscalatingFailure));
		assert!(a.changes.is_none(), "no verdict yet, so nothing to compare");
		let checks: Vec<(&str, bool, bool)> = a
			.checks
			.iter()
			.map(|c| {
				(
					c.check.name.as_str(),
					c.escalates,
					c.documentation.is_some(),
				)
			})
			.collect();
		assert_eq!(checks, [("disk", true, true), ("mystery", false, false)]);
		let mut memory: Vec<Uuid> = a.memory.iter().map(|e| e.id).collect();
		memory.sort();
		let mut expected = vec![on_check.id, on_group.id];
		expected.sort();
		assert_eq!(memory, expected);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn changes_since_the_last_verdict_are_read_as_facts() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "production").await;
		let i = incident(&mut conn, g, "production").await;
		let cpu = failing(&mut conn, i, app, "cpu", false, 5).await;
		member(&mut conn, i, app, "disk", "warning", false, 5).await;

		let first = alert_triage::assess(&mut conn, i).await.unwrap();
		let Recorded::Done { verdict, note } =
			record(&mut conn, &verdict(&first, Verdict::Logged, vec![])).await
		else {
			panic!("expected the verdict to be recorded");
		};
		assert_eq!(verdict.worst_result, Some(Severity::Failure));
		assert_eq!(verdict.note_id, Some(note.id));
		assert!(note.body.contains("Alert triage: logged"));

		// A repeated alert at the same state finds the verdict already given.
		let again = alert_triage::assess(&mut conn, i).await.unwrap();
		assert_eq!(again.already_triaged.map(|v| v.id), Some(verdict.id));
		let unchanged = again.changes.unwrap();
		assert!(unchanged.newly_failing.is_empty() && unchanged.no_longer_failing.is_empty());

		failing(&mut conn, i, app, "disk", true, 1).await;
		conn.batch_execute(&format!(
			"UPDATE issues SET effective_result = 'passed' WHERE id = '{cpu}';"
		))
		.await
		.unwrap();
		let moved = alert_triage::assess(&mut conn, i).await.unwrap();
		assert!(moved.already_triaged.is_none());
		let changes = moved.changes.unwrap();
		assert_eq!(changes.worst_then, Some(Severity::Failure));
		assert_eq!(changes.worst_now, Some(Severity::EscalatingFailure));
		assert_eq!(changes.newly_failing, [key("disk")]);
		assert_eq!(changes.newly_escalating, [key("disk")]);
		assert_eq!(changes.no_longer_failing, [key("cpu")]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn closing_an_incident_is_a_new_state() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "production").await;
		let i = incident(&mut conn, g, "production").await;
		failing(&mut conn, i, app, "disk", false, 5).await;
		let open = alert_triage::assess(&mut conn, i).await.unwrap();
		record(&mut conn, &verdict(&open, Verdict::Escalated, vec![])).await;

		conn.batch_execute(&format!(
			"UPDATE incidents SET closed_at = NOW() WHERE id = '{i}';"
		))
		.await
		.unwrap();
		let closed = alert_triage::assess(&mut conn, i).await.unwrap();
		assert!(!closed.state.open);
		assert!(
			closed.already_triaged.is_none(),
			"the close is news to report"
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_against_a_changed_incident_is_stale() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let i = incident(&mut conn, g, "test").await;
		failing(&mut conn, i, app, "disk", false, 5).await;
		let read = alert_triage::assess(&mut conn, i).await.unwrap();

		failing(&mut conn, i, app, "disk", false, 1).await;
		assert!(matches!(
			record(&mut conn, &verdict(&read, Verdict::Logged, vec![])).await,
			Recorded::Stale
		));
		assert!(
			alert_triage::verdicts_for(&mut conn, i)
				.await
				.unwrap()
				.is_empty()
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_names_the_memory_it_relies_on() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "production").await;
		let i = incident(&mut conn, g, "production").await;
		failing(&mut conn, i, app, "disk", false, 5).await;
		let entry = memory::record(
			&mut conn,
			NewEntry {
				title: "Disk fills during the weekly export".into(),
				body: "recovers on its own".into(),
				link: Some(Link::Group { id: g }),
			},
			Author::token("triage-bot"),
		)
		.await
		.unwrap();
		let a = alert_triage::assess(&mut conn, i).await.unwrap();

		assert!(
			alert_triage::record(
				&mut conn,
				&verdict(&a, Verdict::Logged, vec![Uuid::new_v4()]),
				"triage-bot",
				AuthorKind::Agent,
			)
			.await
			.is_err(),
			"an unknown memory entry is refused"
		);
		let Recorded::Done {
			verdict: logged, ..
		} = record(&mut conn, &verdict(&a, Verdict::Logged, vec![entry.id])).await
		else {
			panic!("expected the verdict to be recorded");
		};
		assert_eq!(logged.verdict, Verdict::Logged);
		assert_eq!(logged.memory_entry_ids, vec![entry.id]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_write_up_day_lists_its_verdicts_and_untriaged_published_incidents() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let triaged = incident(&mut conn, g, "test").await;
		failing(&mut conn, triaged, app, "disk", false, 5).await;
		published(&mut conn, triaged, 1).await;
		let (h, other) = group(&mut conn, "Highlands", "test").await;
		let untriaged = incident(&mut conn, h, "test").await;
		failing(&mut conn, untriaged, other, "disk", false, 5).await;
		published(&mut conn, untriaged, 1).await;

		let a = alert_triage::assess(&mut conn, triaged).await.unwrap();
		let Recorded::Done { verdict: v, .. } =
			record(&mut conn, &verdict(&a, Verdict::Escalated, vec![])).await
		else {
			panic!("expected the verdict to be recorded");
		};
		let reported = alert_triage::attach_report(&mut conn, v.id, "https://slack.example/p1")
			.await
			.unwrap();
		assert_eq!(
			reported.report_link.as_deref(),
			Some("https://slack.example/p1")
		);
		assert!(
			alert_triage::attach_report(&mut conn, v.id, "not a link")
				.await
				.is_err()
		);

		let day = alert_triage::write_up_date(Timestamp::now()).unwrap();
		let found = alert_triage::find_day(&mut conn, day).await.unwrap();
		assert_eq!(
			found
				.verdicts
				.iter()
				.map(|d| d.verdict.incident_id)
				.collect::<Vec<_>>(),
			vec![triaged]
		);
		assert_eq!(
			found
				.untriaged
				.iter()
				.map(|u| u.incident_id)
				.collect::<Vec<_>>(),
			vec![untriaged]
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_incident_left_untriaged_raises_a_self_alert_while_triage_is_in_use() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let left = incident(&mut conn, g, "test").await;
		failing(&mut conn, left, app, "disk", false, 180).await;
		published(&mut conn, left, 120).await;

		// Not in use yet: nothing has ever been triaged.
		self_alerts::sweep_untriaged_incidents(&mut conn)
			.await
			.unwrap();
		let alert = self_alerts::current(&mut conn, self_alerts::UNTRIAGED_INCIDENTS_REF)
			.await
			.unwrap();
		assert!(alert.is_none_or(|a| !a.active));

		// Triage of another incident puts it in use.
		let (h, other) = group(&mut conn, "Highlands", "test").await;
		let seen = incident(&mut conn, h, "test").await;
		failing(&mut conn, seen, other, "disk", false, 5).await;
		let a = alert_triage::assess(&mut conn, seen).await.unwrap();
		record(&mut conn, &verdict(&a, Verdict::Logged, vec![])).await;

		self_alerts::sweep_untriaged_incidents(&mut conn)
			.await
			.unwrap();
		let alert = self_alerts::current(&mut conn, self_alerts::UNTRIAGED_INCIDENTS_REF)
			.await
			.unwrap()
			.expect("raised");
		assert!(alert.active);
		assert!(alert.message.contains("Pacific test"), "{}", alert.message);

		let a = alert_triage::assess(&mut conn, left).await.unwrap();
		record(&mut conn, &verdict(&a, Verdict::Escalated, vec![])).await;
		self_alerts::sweep_untriaged_incidents(&mut conn)
			.await
			.unwrap();
		let alert = self_alerts::current(&mut conn, self_alerts::UNTRIAGED_INCIDENTS_REF)
			.await
			.unwrap()
			.expect("still recorded");
		assert!(!alert.active);
	})
	.await;
}
