//! Close-side grace ("linger"): when an incident's last effective failure
//! recovers, the incident stays open for the group's `slack_close_delay`.
//! A failure returning within the window continues the same incident (no
//! new row, no new Slack open); the linger sweep closes it once the window
//! elapses, backdating the close to when the failure left. Operator-driven
//! leaves and zero-window groups close immediately, and the outbox drainer
//! won't ship an `incident_open` while its incident lingers.

use commons_types::status::CheckResult;
use database::issues::{Incident, NewEvent};
use database::slack_outbox::{KIND_INCIDENT_OPEN, KIND_INCIDENT_RESOLVE, SlackOutbox};
use diesel::prelude::*;
use diesel::{QueryableByName, sql_query, sql_types};
use diesel_async::RunQueryDsl;
use uuid::Uuid;

#[derive(QueryableByName)]
struct RowId {
	#[diesel(sql_type = sql_types::Uuid)]
	id: Uuid,
}

/// Application in a fresh group carrying the given linger window (as an SQL
/// interval literal, e.g. `'5 minutes'`).
async fn insert_grouped_server(
	conn: &mut diesel_async::AsyncPgConnection,
	linger: &str,
) -> (Uuid, Uuid) {
	let group: RowId = sql_query(format!(
		"INSERT INTO server_groups (name, slack_close_delay) \
		 VALUES ('linger-group', INTERVAL '{linger}') RETURNING id"
	))
	.get_result(conn)
	.await
	.expect("group");
	let row: RowId = sql_query(
		"WITH m AS (INSERT INTO machines (name, group_id) VALUES ('box', $1) RETURNING id) INSERT INTO applications (type, host, group_id, machine_id) SELECT 'tamanu-central', 'http://linger.invalid/', $1, m.id FROM m RETURNING id",
	)
	.bind::<sql_types::Uuid, _>(group.id)
	.get_result(conn)
	.await
	.expect("server");
	(group.id, row.id)
}

async fn save_event(
	conn: &mut diesel_async::AsyncPgConnection,
	server_id: Uuid,
	r#ref: &str,
	result: CheckResult,
	message: &str,
) {
	let active = matches!(
		result,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let stamp = database::issues::CheckStateStamp {
		check: r#ref.into(),
		observed: result,
		effective: result,
		escalates: false,
		detail: None,
		title: None,
		instanced: None,
	};
	NewEvent {
		source: "test".into(),
		r#ref: r#ref.into(),
		description: None,
		message: message.into(),
		active: Some(active),
		occurred_at: None,
	}
	.save_with_state(conn, server_id, None, Some(&stamp), false)
	.await
	.expect("save event");
}

async fn incident_row(conn: &mut diesel_async::AsyncPgConnection, id: Uuid) -> Incident {
	use database::schema::incidents::dsl;
	dsl::incidents
		.select(Incident::as_select())
		.filter(dsl::id.eq(id))
		.first(conn)
		.await
		.expect("incident row")
}

async fn incident_count(conn: &mut diesel_async::AsyncPgConnection, group_id: Uuid) -> i64 {
	use database::schema::incidents::dsl;
	dsl::incidents
		.filter(dsl::server_group_id.eq(group_id))
		.count()
		.get_result(conn)
		.await
		.expect("count incidents")
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

async fn outbox_rows(conn: &mut diesel_async::AsyncPgConnection) -> Vec<SlackOutbox> {
	use database::schema::slack_outbox::dsl;
	dsl::slack_outbox
		.select(SlackOutbox::as_select())
		.order(dsl::created_at.asc())
		.load(conn)
		.await
		.expect("load outbox")
}

async fn mark_open_delivered(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) {
	let row: RowId =
		sql_query("SELECT id FROM slack_outbox WHERE incident_id = $1 AND kind = $2 LIMIT 1")
			.bind::<sql_types::Uuid, _>(incident_id)
			.bind::<sql_types::Text, _>(KIND_INCIDENT_OPEN)
			.get_result(conn)
			.await
			.expect("pending open row exists");
	SlackOutbox::mark_delivered(conn, row.id, "ok")
		.await
		.expect("mark delivered");
}

/// Backdate `closing_at` past any realistic window, then sweep.
async fn expire_linger(conn: &mut diesel_async::AsyncPgConnection, incident_id: Uuid) -> usize {
	sql_query("UPDATE incidents SET closing_at = closing_at - INTERVAL '1 hour' WHERE id = $1")
		.bind::<sql_types::Uuid, _>(incident_id)
		.execute(conn)
		.await
		.expect("expire linger");
	database::issues::sweep_lingering_incidents(conn)
		.await
		.expect("linger sweep")
}

/// The headline behaviour: a red check that blips green and comes back
/// within the window stays one incident — one open, one resolve, however
/// many blips.
#[tokio::test(flavor = "multi_thread")]
async fn rejoin_within_linger_continues_the_same_incident() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group_id, server_id) = insert_grouped_server(&mut conn, "5 minutes").await;

		save_event(&mut conn, server_id, "flappy", CheckResult::Failed, "boom").await;
		let incident = the_open_incident(&mut conn, group_id).await;
		assert_eq!(incident.closing_at, None);
		mark_open_delivered(&mut conn, incident.id).await;

		// Blip green: the incident lingers instead of closing.
		save_event(&mut conn, server_id, "flappy", CheckResult::Passed, "ok?").await;
		let lingering = incident_row(&mut conn, incident.id).await;
		assert_eq!(lingering.closed_at, None, "incident stays open");
		assert!(lingering.closing_at.is_some(), "lingering is stamped");
		assert_eq!(
			outbox_rows(&mut conn).await.len(),
			1,
			"no resolve enqueued while lingering"
		);

		// Red again within the window: same incident, lingering cleared,
		// still no extra Slack traffic.
		save_event(&mut conn, server_id, "flappy", CheckResult::Failed, "boom").await;
		let resumed = incident_row(&mut conn, incident.id).await;
		assert_eq!(resumed.closing_at, None, "rejoin ends the lingering");
		assert_eq!(resumed.closed_at, None);
		assert_eq!(
			incident_count(&mut conn, group_id).await,
			1,
			"the blip did not fragment the trouble into a second incident"
		);
		assert_eq!(
			outbox_rows(&mut conn).await.len(),
			1,
			"no second open for the rejoin"
		);

		// Final recovery, window elapses: closed, backdated, one resolve.
		save_event(&mut conn, server_id, "flappy", CheckResult::Passed, "ok").await;
		let closed_count = expire_linger(&mut conn, incident.id).await;
		assert_eq!(closed_count, 1, "sweep closed exactly this incident");
		let closed = incident_row(&mut conn, incident.id).await;
		assert_eq!(
			closed.closed_at, closed.closing_at,
			"close is backdated to the (test-shifted) lingering stamp"
		);
		assert!(closed.closed_at.is_some());
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 2, "one open, one resolve: {rows:?}");
		assert_eq!(rows[1].kind, KIND_INCIDENT_RESOLVE);
	})
	.await
}

/// A member that never left — a warning contributor — re-grading to an
/// effective failure while the incident lingers ends the lingering too.
#[tokio::test(flavor = "multi_thread")]
async fn member_regrade_to_failure_ends_lingering() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group_id, server_id) = insert_grouped_server(&mut conn, "5 minutes").await;

		save_event(&mut conn, server_id, "opener", CheckResult::Failed, "boom").await;
		let incident = the_open_incident(&mut conn, group_id).await;
		// A warning joins the open incident but can't hold (or re-open) it.
		save_event(
			&mut conn,
			server_id,
			"grumbler",
			CheckResult::Warning,
			"meh",
		)
		.await;

		save_event(&mut conn, server_id, "opener", CheckResult::Passed, "ok").await;
		assert!(
			incident_row(&mut conn, incident.id)
				.await
				.closing_at
				.is_some(),
			"warning member alone leaves the incident lingering"
		);

		// The warning escalates to a failure without ever leaving: the
		// lingering ends, the incident continues.
		save_event(&mut conn, server_id, "grumbler", CheckResult::Failed, "ow").await;
		let resumed = incident_row(&mut conn, incident.id).await;
		assert_eq!(resumed.closing_at, None, "regrade ends the lingering");
		assert_eq!(resumed.closed_at, None);
		assert_eq!(incident_count(&mut conn, group_id).await, 1);
	})
	.await
}

/// An operator resolving the last failure is not a flap: the incident
/// closes immediately and the resolve ships attributed to them.
#[tokio::test(flavor = "multi_thread")]
async fn operator_resolve_closes_immediately() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		use database::issues::Issue;

		let (group_id, server_id) = insert_grouped_server(&mut conn, "5 minutes").await;
		save_event(&mut conn, server_id, "handled", CheckResult::Failed, "boom").await;
		let incident = the_open_incident(&mut conn, group_id).await;
		mark_open_delivered(&mut conn, incident.id).await;

		let issue: Issue = {
			use database::schema::issues::dsl;
			dsl::issues
				.select(Issue::as_select())
				.filter(dsl::application_id.eq(server_id))
				.filter(dsl::ref_.eq("handled"))
				.first(&mut conn)
				.await
				.expect("issue")
		};
		Issue::resolve(
			&mut conn,
			issue.id,
			"op@example.com",
			commons_types::issue::ResolvedReason::Fixed,
		)
		.await
		.expect("operator resolve");

		let closed = incident_row(&mut conn, incident.id).await;
		assert!(
			closed.closed_at.is_some(),
			"operator-driven leave skips the linger"
		);
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(rows.len(), 2, "resolve shipped at once: {rows:?}");
		assert_eq!(rows[1].kind, KIND_INCIDENT_RESOLVE);
	})
	.await
}

/// A zero `slack_close_delay` opts the group out of lingering entirely.
#[tokio::test(flavor = "multi_thread")]
async fn zero_window_closes_immediately() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group_id, server_id) = insert_grouped_server(&mut conn, "0").await;
		save_event(&mut conn, server_id, "quick", CheckResult::Failed, "boom").await;
		let incident = the_open_incident(&mut conn, group_id).await;

		save_event(&mut conn, server_id, "quick", CheckResult::Passed, "ok").await;
		let closed = incident_row(&mut conn, incident.id).await;
		assert!(closed.closed_at.is_some(), "zero window: no linger");
		assert_eq!(closed.closing_at, None);
	})
	.await
}

/// The drainer must not ship an `incident_open` while its incident
/// lingers: a one-off blip would otherwise notify purely because the
/// linger held the incident open past its `deliver_after`.
#[tokio::test(flavor = "multi_thread")]
async fn drainer_holds_opens_while_lingering() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group_id, server_id) = insert_grouped_server(&mut conn, "5 minutes").await;
		save_event(&mut conn, server_id, "blippy", CheckResult::Failed, "boom").await;
		let incident = the_open_incident(&mut conn, group_id).await;

		// Let the open-side grace elapse so the row is nominally claimable.
		sql_query(
			"UPDATE slack_outbox SET deliver_after = NOW() - INTERVAL '1 second' WHERE incident_id = $1",
		)
		.bind::<sql_types::Uuid, _>(incident.id)
		.execute(&mut conn)
		.await
		.expect("age the open row");

		// While red: claimable.
		let claimed = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert_eq!(
			claimed.len(),
			1,
			"open is claimable while a failure is live"
		);

		// Blip green: lingering, the open must be held back.
		save_event(&mut conn, server_id, "blippy", CheckResult::Passed, "ok?").await;
		let claimed = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert!(
			claimed.is_empty(),
			"open is not claimable while the incident lingers: {claimed:?}"
		);

		// Red returns: claimable again — trouble has outlived the grace.
		save_event(&mut conn, server_id, "blippy", CheckResult::Failed, "boom").await;
		let claimed = SlackOutbox::claim_pending(&mut conn, 10)
			.await
			.expect("claim");
		assert_eq!(claimed.len(), 1, "open claimable again after the rejoin");

		// And if instead the flap ends for good, expiry cancels the open:
		// Slack never hears about the blip.
		save_event(&mut conn, server_id, "blippy", CheckResult::Passed, "ok").await;
		expire_linger(&mut conn, incident.id).await;
		let rows = outbox_rows(&mut conn).await;
		assert_eq!(
			rows.len(),
			1,
			"no resolve for a never-shipped open: {rows:?}"
		);
		assert!(
			rows[0].gave_up_at.is_some(),
			"pending open cancelled at linger expiry"
		);
		assert_eq!(
			incident_row(&mut conn, incident.id).await.closed_at,
			incident_row(&mut conn, incident.id).await.closing_at,
			"close backdated to when the failure left"
		);
	})
	.await
}

/// Timestamps: `deliver_after` on the open row is untouched by lingering,
/// so an incident whose trouble keeps flapping still publishes once its
/// cumulative age passes the open grace — the fast flapper is no longer
/// permanently silent.
#[tokio::test(flavor = "multi_thread")]
async fn flapping_incident_still_publishes_after_grace() {
	commons_tests::db::TestDb::run(async |mut conn, _| {
		let (group_id, server_id) = insert_grouped_server(&mut conn, "5 minutes").await;
		save_event(&mut conn, server_id, "flappy", CheckResult::Failed, "boom").await;
		let incident = the_open_incident(&mut conn, group_id).await;
		let before = outbox_rows(&mut conn).await;
		assert_eq!(before[0].incident_id, Some(incident.id));
		let original_deliver_after = before[0].deliver_after;

		// A full flap cycle: green, red again.
		save_event(&mut conn, server_id, "flappy", CheckResult::Passed, "ok?").await;
		save_event(&mut conn, server_id, "flappy", CheckResult::Failed, "boom").await;

		let after = outbox_rows(&mut conn).await;
		assert_eq!(after.len(), 1, "still exactly one open row");
		assert_eq!(
			after[0].deliver_after, original_deliver_after,
			"the open's grace counts from the incident's first open, not the latest red"
		);
		assert!(after[0].gave_up_at.is_none());
	})
	.await
}
