//! Alert triage: the escalations Canopy decides, recording verdicts against a
//! revision, the harmless-pattern exception, the write-up day, and the
//! self-alert for incidents left untriaged.

use database::{
	alert_triage::{self, Assessment, NewVerdict, Reason, Recorded, Severity, Verdict},
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

async fn canopy_incident(conn: &mut AsyncPgConnection) -> Uuid {
	let id = Uuid::new_v4();
	conn.batch_execute(&format!(
		"INSERT INTO incidents (id, opened_at) VALUES ('{id}', NOW());"
	))
	.await
	.expect("seed canopy incident");
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

/// Catalogue a check with documentation, so it is familiar.
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
		read_at: Timestamp::now(),
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

fn kinds(reasons: &[Reason]) -> Vec<&'static str> {
	reasons
		.iter()
		.map(|r| match r {
			Reason::ProductionOpening => "production_opening",
			Reason::CanopyOpening => "canopy_opening",
			Reason::EscalatingFailure { .. } => "escalating_failure",
			Reason::Worse { .. } => "worse",
			Reason::NewFailingCheck { .. } => "new_failing_check",
			Reason::SharedCheck { .. } => "shared_check",
			Reason::Unfamiliar { .. } => "unfamiliar",
		})
		.collect()
}

#[tokio::test(flavor = "multi_thread")]
async fn first_triage_escalates_production_and_canopy_but_not_a_lesser_environment() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		let (prod, prod_app) = group(&mut conn, "Pacific", "production").await;
		let (test, test_app) = group(&mut conn, "Highlands", "test").await;
		let prod_incident = incident(&mut conn, prod, "production").await;
		let test_incident = incident(&mut conn, test, "test").await;
		failing(&mut conn, prod_incident, prod_app, "disk", false, 5).await;
		failing(&mut conn, test_incident, test_app, "disk", false, 300).await;
		let canopy = canopy_incident(&mut conn).await;

		let a = alert_triage::assess(&mut conn, prod_incident)
			.await
			.unwrap();
		assert_eq!(kinds(&a.reasons), ["production_opening"]);
		assert!(a.exception_can_answer);
		assert_eq!(a.target, "Pacific");

		let a = alert_triage::assess(&mut conn, test_incident)
			.await
			.unwrap();
		assert!(a.reasons.is_empty(), "{:?}", a.reasons);
		assert_eq!(a.target, "Highlands test");

		let a = alert_triage::assess(&mut conn, canopy).await.unwrap();
		assert_eq!(kinds(&a.reasons), ["canopy_opening"]);
		assert!(!a.exception_can_answer);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn an_escalating_or_unfamiliar_check_requires_escalation() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let i = incident(&mut conn, g, "test").await;
		failing(&mut conn, i, app, "mystery", true, 5).await;

		let a = alert_triage::assess(&mut conn, i).await.unwrap();
		assert_eq!(kinds(&a.reasons), ["escalating_failure", "unfamiliar"]);
		assert_eq!(a.state.worst, Some(Severity::EscalatingFailure));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_closed_or_resolved_incident_has_no_reasons() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "production").await;
		let closed = incident(&mut conn, g, "production").await;
		failing(&mut conn, closed, app, "mystery", true, 5).await;
		conn.batch_execute(&format!(
			"UPDATE incidents SET closed_at = NOW() WHERE id = '{closed}';"
		))
		.await
		.unwrap();
		let a = alert_triage::assess(&mut conn, closed).await.unwrap();
		assert!(!a.state.open);
		assert!(a.reasons.is_empty());

		let resolved = incident(&mut conn, g, "production").await;
		conn.batch_execute(&format!(
			"UPDATE incidents SET resolved_at = NOW(), resolved_by = 'op' WHERE id = '{resolved}';"
		))
		.await
		.unwrap();
		let a = alert_triage::assess(&mut conn, resolved).await.unwrap();
		assert!(a.reasons.is_empty());
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_is_the_baseline_for_the_next_triage() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		documented(&mut conn, "cpu").await;
		let (g, app) = group(&mut conn, "Pacific", "production").await;
		let i = incident(&mut conn, g, "production").await;
		member(&mut conn, i, app, "disk", "warning", false, 5).await;
		failing(&mut conn, i, app, "cpu", false, 5).await;

		let first = alert_triage::assess(&mut conn, i).await.unwrap();
		assert_eq!(kinds(&first.reasons), ["production_opening"]);
		let Recorded::Done { verdict, note, .. } =
			record(&mut conn, &verdict(&first, Verdict::Escalated, vec![])).await
		else {
			panic!("expected the verdict to be recorded");
		};
		assert_eq!(verdict.worst_result, Some(Severity::Failure));
		assert_eq!(verdict.note_id, Some(note.id));
		assert!(note.body.contains("Alert triage: escalated"));

		// A repeated alert at the same state finds the verdict already given.
		let again = alert_triage::assess(&mut conn, i).await.unwrap();
		assert!(again.reasons.is_empty(), "{:?}", again.reasons);
		assert_eq!(again.already_triaged.map(|v| v.id), Some(verdict.id));

		// A newly failing check is a new reason, and so is the worst result
		// rising once it escalates.
		let disk = failing(&mut conn, i, app, "disk", false, 1).await;
		let worse = alert_triage::assess(&mut conn, i).await.unwrap();
		assert!(worse.already_triaged.is_none());
		assert_eq!(kinds(&worse.reasons), ["new_failing_check"]);

		conn.batch_execute(&format!(
			"UPDATE issues SET escalates = true WHERE id = '{disk}';"
		))
		.await
		.unwrap();
		let escalating = alert_triage::assess(&mut conn, i).await.unwrap();
		assert_eq!(
			kinds(&escalating.reasons),
			["escalating_failure", "worse", "new_failing_check"]
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_verdict_against_a_changed_incident_is_stale() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let i = incident(&mut conn, g, "test").await;
		failing(&mut conn, i, app, "disk", false, 5).await;
		let read = alert_triage::assess(&mut conn, i).await.unwrap();

		failing(&mut conn, i, app, "disk", false, 1).await;
		let Recorded::Stale { current } =
			record(&mut conn, &verdict(&read, Verdict::Logged, vec![])).await
		else {
			panic!("expected a stale revision to be refused");
		};
		assert_ne!(current, read.state.revision);
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
async fn logging_what_canopy_requires_escalated_is_refused() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		let (g, app) = group(&mut conn, "Pacific", "test").await;
		let i = incident(&mut conn, g, "test").await;
		failing(&mut conn, i, app, "mystery", false, 5).await;
		let a = alert_triage::assess(&mut conn, i).await.unwrap();
		let Recorded::Refused { reasons, .. } =
			record(&mut conn, &verdict(&a, Verdict::Logged, vec![])).await
		else {
			panic!("expected the logged verdict to be refused");
		};
		assert_eq!(kinds(&reasons), ["unfamiliar"]);
		assert!(matches!(
			record(&mut conn, &verdict(&a, Verdict::Escalated, vec![])).await,
			Recorded::Done { .. }
		));
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn the_harmless_pattern_exception_needs_a_current_entry_for_each_failing_check() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		documented(&mut conn, "cpu").await;
		let (g, app) = group(&mut conn, "Pacific", "production").await;
		let i = incident(&mut conn, g, "production").await;
		failing(&mut conn, i, app, "disk", false, 5).await;
		failing(&mut conn, i, app, "cpu", false, 5).await;
		let a = alert_triage::assess(&mut conn, i).await.unwrap();
		assert_eq!(kinds(&a.reasons), ["production_opening"]);

		let author = Author::token("triage-bot");
		let entry = |title: &str, link| NewEntry {
			title: title.into(),
			body: "harmless".into(),
			link: Some(link),
		};
		let check = |name: &str| Link::Check {
			source: "alertd".into(),
			name: name.into(),
		};
		let disk = memory::record(&mut conn, entry("disk", check("disk")), author)
			.await
			.unwrap();
		let cpu = memory::record(&mut conn, entry("cpu", check("cpu")), author)
			.await
			.unwrap();
		let site = memory::record(&mut conn, entry("site", Link::Group { id: g }), author)
			.await
			.unwrap();
		let a = alert_triage::assess(&mut conn, i).await.unwrap();

		for entries in [vec![], vec![disk.id], vec![disk.id, site.id]] {
			assert!(
				matches!(
					record(&mut conn, &verdict(&a, Verdict::Logged, entries.clone())).await,
					Recorded::Refused { .. }
				),
				"{entries:?} should not answer every failing check"
			);
		}

		memory::retire(&mut conn, cpu.id, "no longer true", author)
			.await
			.unwrap();
		assert!(matches!(
			record(
				&mut conn,
				&verdict(&a, Verdict::Logged, vec![disk.id, cpu.id])
			)
			.await,
			Recorded::Refused { .. }
		));

		let cpu = memory::record(&mut conn, entry("cpu again", check("cpu")), author)
			.await
			.unwrap();
		let a = alert_triage::assess(&mut conn, i).await.unwrap();
		let Recorded::Done {
			verdict: logged, ..
		} = record(
			&mut conn,
			&verdict(&a, Verdict::Logged, vec![disk.id, cpu.id]),
		)
		.await
		else {
			panic!("expected the exception to answer the production opening");
		};
		assert_eq!(logged.verdict, Verdict::Logged);
		assert_eq!(logged.memory_entry_ids, vec![disk.id, cpu.id]);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn a_shared_check_requires_escalation_only_when_the_failures_are_close_in_time() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		documented(&mut conn, "cpu").await;
		let (a_group, a_app) = group(&mut conn, "Pacific", "test").await;
		let (b_group, b_app) = group(&mut conn, "Highlands", "test").await;
		let (c_group, c_app) = group(&mut conn, "Islands", "test").await;
		let (d_group, d_app) = group(&mut conn, "Coast", "test").await;
		let here = incident(&mut conn, a_group, "test").await;
		let near = incident(&mut conn, b_group, "test").await;
		let chronic = incident(&mut conn, c_group, "test").await;
		let candidate = incident(&mut conn, d_group, "test").await;
		failing(&mut conn, here, a_app, "disk", false, 5).await;
		failing(&mut conn, near, b_app, "disk", false, 20).await;
		failing(&mut conn, chronic, c_app, "disk", false, 60 * 24 * 7).await;
		failing(&mut conn, candidate, d_app, "cpu", false, 10).await;

		let a = alert_triage::assess(&mut conn, here).await.unwrap();
		assert_eq!(kinds(&a.reasons), ["shared_check"]);
		let Reason::SharedCheck { incident_id, .. } = &a.reasons[0] else {
			unreachable!()
		};
		assert_eq!(*incident_id, near);

		let mut shared: Vec<Uuid> = a.shared.iter().map(|r| r.incident_id).collect();
		shared.sort();
		let mut expected = vec![near, chronic];
		expected.sort();
		assert_eq!(shared, expected, "both share the check; only one forces it");
		assert_eq!(
			a.candidates
				.iter()
				.map(|r| r.incident_id)
				.collect::<Vec<_>>(),
			vec![candidate]
		);
	})
	.await;
}

#[tokio::test(flavor = "multi_thread")]
async fn related_escalations_since_the_read_are_returned_with_the_verdict() {
	commons_tests::db::TestDb::run(async |mut conn, _url| {
		documented(&mut conn, "disk").await;
		let (a_group, a_app) = group(&mut conn, "Pacific", "test").await;
		let (b_group, b_app) = group(&mut conn, "Highlands", "test").await;
		let here = incident(&mut conn, a_group, "test").await;
		let there = incident(&mut conn, b_group, "test").await;
		failing(&mut conn, here, a_app, "disk", false, 5).await;
		failing(&mut conn, there, b_app, "disk", false, 6).await;

		let read_here = alert_triage::assess(&mut conn, here).await.unwrap();
		let mut new_here = verdict(&read_here, Verdict::Escalated, vec![]);
		new_here.read_at = Timestamp::now();

		let read_there = alert_triage::assess(&mut conn, there).await.unwrap();
		record(&mut conn, &verdict(&read_there, Verdict::Escalated, vec![])).await;

		let Recorded::Done {
			escalated_since, ..
		} = record(&mut conn, &new_here).await
		else {
			panic!("expected the verdict to be recorded");
		};
		assert_eq!(
			escalated_since
				.iter()
				.map(|r| r.incident_id)
				.collect::<Vec<_>>(),
			vec![there]
		);
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

		// The two share a check failing close in time, so this one is escalated.
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
