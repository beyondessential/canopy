//! Phase A: opening or closing an incident enqueues a `slack_outbox` row.
//!
//! Doesn't speak to Slack — the drainer is a separate binary and is tested
//! against a mock there. Here we just check the enqueue side: when an
//! incident transitions, an outbox row exists with the expected `kind` and
//! a non-empty `payload`.

use commons_types::{issue::ResolvedReason, status::CheckResult};
use database::{
	issues::{Incident, Issue, NewEvent},
	slack_outbox::{KIND_INCIDENT_OPEN, KIND_INCIDENT_RESOLVE, SlackOutbox},
};
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use jiff::SignedDuration;
use uuid::Uuid;

/// Migration default for `server_groups.slack_open_delay`. Tests that
/// don't override it via `insert_server_with_delay` get this value.
const DEFAULT_OPEN_DELAY: SignedDuration = SignedDuration::from_mins(3);

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

async fn insert_server(conn: &mut diesel_async::AsyncPgConnection, host: &str) -> Uuid {
	insert_server_with_delay(conn, host, None).await
}

/// Variant of [`insert_server`] that pins the group's `slack_open_delay`
/// to `delay_secs` instead of taking the migration default. Pass `None`
/// to keep the default.
async fn insert_server_with_delay(
	conn: &mut diesel_async::AsyncPgConnection,
	host: &str,
	delay_secs: Option<i64>,
) -> Uuid {
	// Group + server pair so events can open incidents (incidents are
	// group-keyed; ungrouped applications don't promote issues to incidents).
	let group: RowId = if let Some(secs) = delay_secs {
		sql_query(
			r#"
				INSERT INTO server_groups (name, slack_open_delay)
				VALUES ('test-group', make_interval(secs => $1))
				RETURNING id
			"#,
		)
		.bind::<sql_types::BigInt, _>(secs)
		.get_result(conn)
		.await
		.expect("insert group with delay")
	} else {
		sql_query(
			r#"
				INSERT INTO server_groups (name)
				VALUES ('test-group')
				RETURNING id
			"#,
		)
		.get_result(conn)
		.await
		.expect("insert group")
	};
	let row: RowId = sql_query(
		r#"
			WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $2) RETURNING id) INSERT INTO applications (type, host, group_id, rank, machine_id) SELECT 'tamanu-central', $1, $2, 'production', m.id FROM m
			RETURNING id
		"#,
	)
	.bind::<sql_types::Text, _>(host)
	.bind::<sql_types::Uuid, _>(group.id)
	.get_result(conn)
	.await
	.expect("insert server");
	row.id
}

async fn pending_for_incident(
	conn: &mut diesel_async::AsyncPgConnection,
	incident_id: Uuid,
) -> Vec<SlackOutbox> {
	use database::diesel_async::RunQueryDsl;
	use database::schema::slack_outbox::dsl;
	use diesel::prelude::*;
	dsl::slack_outbox
		.select(SlackOutbox::as_select())
		.filter(dsl::incident_id.eq(incident_id))
		.order(dsl::created_at.asc())
		.load(conn)
		.await
		.expect("load slack_outbox rows")
}

/// Force-deliver the open row for `incident_id` so a subsequent resolve
/// no longer treats it as cancellable. Used by tests that want to
/// exercise the post-delivery code path (where the resolve actually does
/// enqueue) without waiting `slack_open_delay` for the real drainer.
async fn mark_open_delivered(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) {
	use database::diesel_async::RunQueryDsl;
	use database::schema::slack_outbox::dsl;
	use diesel::prelude::*;
	let row_id: Uuid = dsl::slack_outbox
		.select(dsl::id)
		.filter(dsl::incident_id.eq(incident_id))
		.filter(dsl::kind.eq(KIND_INCIDENT_OPEN))
		.first(conn)
		.await
		.expect("open row exists");
	SlackOutbox::mark_delivered(conn, row_id, "ok")
		.await
		.expect("mark delivered");
}

#[tokio::test(flavor = "multi_thread")]
async fn opening_incident_enqueues_slack_open_row() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://open.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-1".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-1".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		let issue = event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");

		// The save call should have opened an incident *and* enqueued an
		// `incident_open` outbox row.
		let incident: Incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("issue opened an incident");
		let _ = issue.id;

		let rows = pending_for_incident(&mut conn, incident.id).await;
		let opens: Vec<_> = rows
			.iter()
			.filter(|r| r.kind == KIND_INCIDENT_OPEN)
			.collect();
		assert_eq!(opens.len(), 1, "exactly one open row");
		let open = opens[0];
		assert_eq!(open.issue_id, Some(issue.id));
		assert!(open.delivered_at.is_none());
		assert_eq!(open.attempts, 0);
		// Open rows wait the group's `slack_open_delay` (defaults to 3
		// minutes) before the drainer is allowed to ship them.
		// `created_at` is set server-side by the migration default, so
		// the gap should equal the per-group delay give-or-take the
		// round-trip time of the enqueue itself.
		let target = open.created_at + DEFAULT_OPEN_DELAY;
		let drift = (open.deliver_after - target).get_seconds().unsigned_abs();
		assert!(
			drift <= 5,
			"deliver_after should sit ~slack_open_delay past created_at; drift={drift}s \
			 (created_at={}, deliver_after={})",
			open.created_at,
			open.deliver_after,
		);
		// Payload is a flat object matching the workflow trigger's variables.
		// `link` is intentionally absent here — the drainer injects it at
		// delivery time from PRIVATE_URL + row.incident_id.
		let payload = open.payload.as_object().expect("payload is a JSON object");
		assert!(payload.contains_key("server"));
		// The payload is the incident's summary: its counts in `source_ref`,
		// and one line per live issue, named by its check where it carries no
		// headline, with the application it is on. An application nobody has
		// named reads as its type, never its host or its id.
		// spec: FLT#naming
		assert_eq!(payload["server"].as_str(), Some("test-group"));
		assert_eq!(payload["severity"].as_str(), Some("Error"));
		assert_eq!(payload["source_ref"].as_str(), Some("1 failed"));
		let message = payload["message"].as_str().expect("message");
		assert!(
			message.starts_with("• Failed: ref-1 on Tamanu central"),
			"got: {message}"
		);
		assert!(!message.contains(&server_id.to_string()), "got: {message}");
		assert!(
			!payload.contains_key("link"),
			"link is injected by the drainer, not at enqueue"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn resolving_incident_after_open_delivered_enqueues_resolve_row() {
	// Once Slack has heard about the open, we owe a resolve. The
	// flap-suppression path only kicks in when the open hasn't shipped
	// yet; everything past that point is the historical behaviour.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://resolve.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-2".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-2".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("incident opened");
		mark_open_delivered(&mut conn, incident.id).await;

		Incident::resolve(
			&mut conn,
			incident.id,
			"operator@example.test",
			ResolvedReason::Fixed,
		)
		.await
		.expect("resolve");

		let rows = pending_for_incident(&mut conn, incident.id).await;
		let resolves: Vec<_> = rows
			.iter()
			.filter(|r| r.kind == KIND_INCIDENT_RESOLVE)
			.collect();
		assert_eq!(resolves.len(), 1, "exactly one resolve row");
		assert!(resolves[0].delivered_at.is_none());
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn resolving_before_open_ships_cancels_open_and_skips_resolve() {
	// The flap-suppression contract. If the incident comes and goes
	// inside the `deliver_after` window, the operator never sees a Slack
	// noise about either edge — but the open row stays in the database
	// (given-up, with a reason in `last_error`) for the audit trail.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://flap.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-flap".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-flap".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("incident opened");

		// Resolve before the open's deliver_after window passes.
		Incident::resolve(
			&mut conn,
			incident.id,
			"operator@example.test",
			ResolvedReason::Fixed,
		)
		.await
		.expect("resolve");

		let rows = pending_for_incident(&mut conn, incident.id).await;
		let opens: Vec<_> = rows
			.iter()
			.filter(|r| r.kind == KIND_INCIDENT_OPEN)
			.collect();
		assert_eq!(
			opens.len(),
			1,
			"open row stays in the table for historicity"
		);
		assert!(
			opens[0].gave_up_at.is_some(),
			"open row marked given-up so the drainer won't ship it"
		);
		assert!(opens[0].delivered_at.is_none(), "open never delivered");
		assert!(
			opens[0]
				.last_error
				.as_deref()
				.is_some_and(|e| e.contains("cancelled")),
			"reason is recorded for the audit trail; got: {:?}",
			opens[0].last_error
		);

		let resolves: Vec<_> = rows
			.iter()
			.filter(|r| r.kind == KIND_INCIDENT_RESOLVE)
			.collect();
		assert!(
			resolves.is_empty(),
			"no resolve row when the matching open never went to Slack"
		);
	})
	.await
}

/// Escalation enqueues a *second* `incident_open` for the same incident. If
/// the incident resolves before the drainer ships that escalation row,
/// cancelling it is right — but skipping the resolve is not: Slack is still
/// showing the original open, and would show it as unresolved forever.
#[tokio::test(flavor = "multi_thread")]
async fn resolve_is_still_sent_when_only_the_escalation_open_was_pending() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://escalate.invalid/").await;

		// An Error-severity failure opens the incident.
		let stamp = database::issues::CheckStateStamp {
			check: "ref-error".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		NewEvent {
			source: "test".into(),
			r#ref: "ref-error".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		}
		.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
		.await
		.expect("save error issue");

		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("incident opened");

		// Slack has now seen the incident.
		mark_open_delivered(&mut conn, incident.id).await;

		// An escalating failure joins, enqueuing a second open row.
		let escalating = database::issues::CheckStateStamp {
			check: "ref-critical".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: true,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		NewEvent {
			source: "test".into(),
			r#ref: "ref-critical".into(),
			description: None,
			message: "worse".into(),
			active: Some(true),
			occurred_at: None,
		}
		.save_with_state(&mut conn, server_id, None, Some(&escalating), false)
		.await
		.expect("save escalating issue");

		let opens: Vec<_> = pending_for_incident(&mut conn, incident.id)
			.await
			.into_iter()
			.filter(|r| r.kind == KIND_INCIDENT_OPEN)
			.collect();
		assert_eq!(opens.len(), 2, "escalation enqueues a second open");
		assert!(
			opens.iter().any(|r| r.delivered_at.is_none()),
			"the escalation open is still pending",
		);

		// Resolving now cancels the pending escalation open — but Slack is
		// showing the delivered original, so it is owed a resolve.
		Incident::resolve(
			&mut conn,
			incident.id,
			"operator@example.test",
			ResolvedReason::Fixed,
		)
		.await
		.expect("resolve");

		let rows = pending_for_incident(&mut conn, incident.id).await;
		let resolves: Vec<_> = rows
			.iter()
			.filter(|r| r.kind == KIND_INCIDENT_RESOLVE)
			.collect();
		assert_eq!(
			resolves.len(),
			1,
			"Slack saw the open, so it must see the resolve: {rows:#?}",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn cascade_close_via_issue_resolve_attributes_to_operator() {
	// When the operator resolves the last live issue and the cascade
	// closes the incident, the resulting Slack resolve row must credit
	// the operator — not say "automation". Earlier behavior threaded
	// `None` through the cascade path and lost the attribution.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://attribute.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-only".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-only".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		let issue = event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("incident opened");
		// Pretend the drainer already shipped the open so the cascade
		// resolve actually enqueues — otherwise the open would be
		// cancelled and there'd be no resolve row to inspect.
		mark_open_delivered(&mut conn, incident.id).await;

		Issue::resolve(
			&mut conn,
			issue.id,
			"operator@example.test",
			ResolvedReason::Fixed,
		)
		.await
		.expect("resolve issue");

		let rows = pending_for_incident(&mut conn, incident.id).await;
		let resolve = rows
			.iter()
			.find(|r| r.kind == KIND_INCIDENT_RESOLVE)
			.expect("resolve row enqueued");
		assert_eq!(
			resolve.payload["by"].as_str(),
			Some("operator@example.test"),
			"cascade close must credit the operator, not automation"
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn nil_server_events_do_not_open_incidents() {
	// The meta/nil server hosts canopy's own self-monitoring events
	// (e.g. "Slack delivery failure"). It is intentionally **ungrouped**
	// in the new model, so events file as issues but never roll up to a
	// group-level incident — which means the Slack drainer can never
	// loop back into itself by re-firing on a canopy-self failure.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let stamp = database::issues::CheckStateStamp {
			check: "slack-delivery-failure".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "canopy".into(),
			r#ref: "slack-delivery-failure".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, Uuid::nil(), None, Some(&stamp), false)
			.await
			.expect("save");

		let incidents = Incident::list_for_server(&mut conn, Uuid::nil(), false, 10)
			.await
			.expect("list incidents");
		assert!(
			incidents.is_empty(),
			"nil/meta server is ungrouped — events don't open incidents",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn mark_given_up_removes_row_from_claim_pending() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://giveup.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-g".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-g".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");
		// `event.save` enqueues an open whose deliver_after sits
		// `slack_open_delay` in the future. Drop it back to the past so
		// claim_pending will return the row immediately.
		expire_deliver_after(&mut conn).await;
		let row = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim")
			.into_iter()
			.next()
			.expect("one pending row");

		SlackOutbox::mark_given_up(&mut conn, row.id, "deliberately abandoned")
			.await
			.expect("mark_given_up");

		let still_pending = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim again");
		assert!(
			still_pending.iter().all(|r| r.id != row.id),
			"gave-up row must not be reclaimed"
		);
	})
	.await
}

/// The retry budget has to be a duration, not a handful of ticks. The
/// drainer re-claims every 5 seconds, so a failed row that stays immediately
/// claimable burns all ten attempts inside a couple of minutes and is given
/// up permanently — any Slack outage longer than that silently drops every
/// page enqueued during it.
#[tokio::test(flavor = "multi_thread")]
async fn mark_failed_holds_the_row_back_for_its_backoff() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://backoff.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-b".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-b".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");
		expire_deliver_after(&mut conn).await;
		let row = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim")
			.into_iter()
			.next()
			.expect("one pending row");

		let attempts = SlackOutbox::mark_failed(&mut conn, row.id, "slack 503", Some("upstream"))
			.await
			.expect("mark_failed");
		assert_eq!(attempts, 1);

		let still_pending = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim again");
		assert!(
			still_pending.iter().all(|r| r.id != row.id),
			"a failed row must not be reclaimable on the very next tick",
		);

		// It comes back once the backoff has elapsed.
		expire_deliver_after(&mut conn).await;
		let after_backoff = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim after backoff");
		assert!(
			after_backoff.iter().any(|r| r.id == row.id),
			"the row is still owed once its backoff passes — failure is not give-up",
		);
	})
	.await
}

#[test]
fn retry_backoff_doubles_then_holds_at_the_cap() {
	use database::slack_outbox::{RETRY_BACKOFF, retry_backoff};

	assert_eq!(retry_backoff(1), SignedDuration::from_secs(15));
	assert_eq!(retry_backoff(2), SignedDuration::from_secs(30));
	assert_eq!(retry_backoff(3), SignedDuration::from_mins(1));
	assert_eq!(retry_backoff(6), SignedDuration::from_mins(8));
	assert_eq!(retry_backoff(7), RETRY_BACKOFF.cap());
	assert_eq!(retry_backoff(100), RETRY_BACKOFF.cap());

	// The drainer's 10-attempt budget must span a routine Slack incident,
	// not a couple of minutes' worth of ticks.
	let total: SignedDuration = (1..10)
		.map(retry_backoff)
		.fold(SignedDuration::ZERO, |acc, d| acc + d);
	assert!(
		total >= SignedDuration::from_mins(45),
		"ten attempts should span most of an hour, got {total}",
	);
}

#[tokio::test(flavor = "multi_thread")]
async fn claim_pending_skips_rows_whose_deliver_after_is_in_the_future() {
	// Direct check on the drainer's claim filter: an open row that's
	// still inside its `slack_open_delay` window must not be claimed,
	// but the moment its deliver_after slides into the past it becomes
	// claimable. This is the core flap-suppression mechanism.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://delayed.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-d".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-d".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");
		let pending_before = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert!(
			pending_before.is_empty(),
			"open row must wait the group delay before becoming claimable; got {} rows",
			pending_before.len(),
		);

		expire_deliver_after(&mut conn).await;

		let pending_after = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim again");
		assert_eq!(
			pending_after.len(),
			1,
			"row becomes claimable once deliver_after is in the past",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn open_delay_honours_per_group_slack_open_delay() {
	// A group with a custom `slack_open_delay` sets the new open's
	// `deliver_after` from that value, not the migration default. Zero
	// means "ship immediately" — the drainer's claim filter
	// (`deliver_after <= NOW()`) will see the row on the very next tick.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id =
			insert_server_with_delay(&mut conn, "http://nowait.invalid/", Some(0)).await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-zero".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-zero".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, server_id, None, Some(&stamp), false)
			.await
			.expect("save");

		let claimable = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert_eq!(
			claimable.len(),
			1,
			"zero-delay group ships its open immediately",
		);
		let open = &claimable[0];
		let drift = (open.deliver_after - open.created_at)
			.get_seconds()
			.unsigned_abs();
		assert!(
			drift <= 2,
			"zero delay should put deliver_after ≈ created_at; drift={drift}s",
		);
	})
	.await
}

#[tokio::test(flavor = "multi_thread")]
async fn pending_opens_until_filters_to_undelivered_in_window() {
	// `pending_opens_until` powers the UI's "held" state — only opens that
	// are still inside their `deliver_after` window AND haven't been
	// delivered or given up qualify.
	commons_tests::db::TestDb::run(async |mut conn, _| {
		// Held incident: default-delay group → open sits 3 min in the future.
		let held_server = insert_server(&mut conn, "http://held.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-held".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-held".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, held_server, None, Some(&stamp), false)
			.await
			.expect("save held");
		let held_incident = Incident::list_for_server(&mut conn, held_server, false, 10)
			.await
			.expect("list incidents")[0]
			.id;

		// Delivered incident: open already shipped → not held any more.
		let delivered_server = insert_server(&mut conn, "http://delivered.invalid/").await;
		let stamp = database::issues::CheckStateStamp {
			check: "ref-delivered".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event = NewEvent {
			source: "test".into(),
			r#ref: "ref-delivered".into(),
			description: None,
			message: "boom".into(),
			active: Some(true),
			occurred_at: None,
		};
		event
			.save_with_state(&mut conn, delivered_server, None, Some(&stamp), false)
			.await
			.expect("save delivered");
		let delivered_incident = Incident::list_for_server(&mut conn, delivered_server, false, 10)
			.await
			.expect("list incidents")[0]
			.id;
		mark_open_delivered(&mut conn, delivered_incident).await;

		let held =
			SlackOutbox::pending_opens_until(&mut conn, &[held_incident, delivered_incident])
				.await
				.expect("pending_opens_until");

		assert!(
			held.contains_key(&held_incident),
			"held incident must appear in the map",
		);
		assert!(
			!held.contains_key(&delivered_incident),
			"once shipped, the open is no longer held",
		);
	})
	.await
}

/// Backdate every pending row's `deliver_after` so the drainer can pick
/// them up without the test having to sleep through `slack_open_delay`.
async fn expire_deliver_after(conn: &mut diesel_async::AsyncPgConnection) {
	sql_query("UPDATE slack_outbox SET deliver_after = NOW() - INTERVAL '1 minute' WHERE delivered_at IS NULL AND gave_up_at IS NULL")
		.execute(conn)
		.await
		.expect("backdate deliver_after");
}

#[tokio::test(flavor = "multi_thread")]
async fn rejoining_open_incident_does_not_re_enqueue_open() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let server_id = insert_server(&mut conn, "http://rejoin.invalid/").await;
		let stamp_a = database::issues::CheckStateStamp {
			check: "ref-a".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event_a = NewEvent {
			source: "test".into(),
			r#ref: "ref-a".into(),
			description: None,
			message: "first".into(),
			active: Some(true),
			occurred_at: None,
		};
		event_a
			.save_with_state(&mut conn, server_id, None, Some(&stamp_a), false)
			.await
			.expect("save a");
		let incident = Incident::list_for_server(&mut conn, server_id, false, 10)
			.await
			.expect("list incidents")
			.into_iter()
			.next()
			.expect("incident");

		// Second active issue, same server: joins the existing incident.
		let stamp_b = database::issues::CheckStateStamp {
			check: "ref-b".into(),
			observed: CheckResult::Failed,
			effective: CheckResult::Failed,
			escalates: false,
			detail: None,
			title: None,
			instances: None,
			inputs: None,
		};
		let event_b = NewEvent {
			source: "test".into(),
			r#ref: "ref-b".into(),
			description: None,
			message: "second".into(),
			active: Some(true),
			occurred_at: None,
		};
		event_b
			.save_with_state(&mut conn, server_id, None, Some(&stamp_b), false)
			.await
			.expect("save b");

		let rows = pending_for_incident(&mut conn, incident.id).await;
		let opens = rows.iter().filter(|r| r.kind == KIND_INCIDENT_OPEN).count();
		assert_eq!(
			opens, 1,
			"only the first issue opens; the second just joins"
		);
	})
	.await
}
