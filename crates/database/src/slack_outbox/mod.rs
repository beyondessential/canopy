//! Slack delivery outbox.
//!
//! Each row is one Slack message we owe somebody. State-change call sites
//! ([`Incident::open_for`](crate::issues::Incident::open_for),
//! [`Incident::resolve`](crate::issues::Incident::resolve), and — in Phase B
//! — issue join/leave and `IncidentNote::add`) insert a row inside their own
//! transaction. The `slacker_outbox` job binary drains the table, posts to
//! Slack, and stamps `delivered_at` on success.
//!
//! Each row carries a `deliver_after` timestamp. Resolves enqueue with
//! `deliver_after = now`, so they ship on the next drain tick. Opens enqueue
//! with a small future delay (the per-group `slack_open_delay`) so a probe
//! that flaps open and resolved within that window can be cancelled before
//! we tell Slack anything happened: the cascade in
//! [`crate::issues::Incident::resolve`] and the auto-close path both call
//! [`SlackOutbox::cancel_pending_open`] before enqueueing the resolve,
//! which marks the open row given-up and skips the resolve when it
//! succeeds.
//!
//! A resolve or maintenance payload is rendered at enqueue time: it describes
//! one event, and is final once that event has happened. An opening,
//! escalation, or reminder instead summarises the incident, and the drainer
//! re-renders it when it claims the row (see [`SlackOutbox::render_current`]),
//! so issues that joined while it waited out the grace period appear in it.
//! The enqueue-time payload is a snapshot of the incident when the row was
//! queued; delivery overwrites it with what was actually sent.

use commons_errors::{AppError, Result};
use commons_types::backoff::Backoff;
use diesel::prelude::*;
use diesel_async::{AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use serde_json::Value as JsonValue;
use uuid::Uuid;

pub mod summary;
pub mod vars;

/// Newly-opened incident — top-level message. Also the escalation of an
/// incident whose opening has already shipped.
pub const KIND_INCIDENT_OPEN: &str = "incident_open";
/// A notified incident still open after another whole day. Posts to the open
/// workflow, but is its own kind so it never reads as the incident's opening:
/// [`SlackOutbox::cancel_pending_open`], [`SlackOutbox::delivered_open_ids`],
/// and [`SlackOutbox::pending_opens_until`] all mean the opening alone.
pub const KIND_INCIDENT_REMINDER: &str = "incident_reminder";
/// Incident resolved — Phase A: top-level; Phase B: reply in the incident thread.
pub const KIND_INCIDENT_RESOLVE: &str = "incident_resolve";
/// Legacy: direct self-alert notices, from before self-alerts flowed
/// through canopy-wide incidents. Nothing enqueues these anymore; the
/// constants remain so the drainer can drain straggler rows harmlessly.
/// A maintenance window declared over a server or a group.
pub const KIND_MAINTENANCE_DECLARED: &str = "maintenance_declared";
/// A maintenance window ended, whether an operator lifted it or its
/// expected end passed.
pub const KIND_MAINTENANCE_ENDED: &str = "maintenance_ended";
pub const KIND_SELF_ALERT_OPEN: &str = "self_alert_open";
/// Legacy: see [`KIND_SELF_ALERT_OPEN`].
pub const KIND_SELF_ALERT_RESOLVE: &str = "self_alert_resolve";

/// Redelivery schedule: 15s after the first failure, doubling, held at 15
/// minutes so the tail of a long outage is retried on a steady cadence rather
/// than an ever-lengthening one.
pub const RETRY_BACKOFF: Backoff =
	Backoff::new(SignedDuration::from_secs(15), SignedDuration::from_mins(15));

/// How long to hold a row back after its `attempts`th failed delivery.
///
/// With the drainer's 10-attempt budget this spends about an hour before
/// giving up (15s, 30s, 1m, 2m, 4m, 8m, then 15m a few times), which covers
/// a routine Slack incident. The naive "retry on the next tick" schedule
/// spent the same budget in well under two minutes.
pub fn retry_backoff(attempts: i32) -> SignedDuration {
	RETRY_BACKOFF.after(attempts.max(0) as u32)
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable)]
#[diesel(table_name = crate::schema::slack_outbox)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct SlackOutbox {
	pub id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	pub kind: String,
	/// `None` for legacy self-alert rows, which had no incident.
	pub incident_id: Option<Uuid>,
	pub issue_id: Option<Uuid>,
	pub note_id: Option<Uuid>,
	pub payload: JsonValue,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub delivered_at: Option<Timestamp>,
	pub attempts: i32,
	pub last_error: Option<String>,
	/// Last raw HTTP body Slack returned (whether 2xx or not). Slack
	/// Workflow Builder webhooks 2xx the *trigger acceptance* even when
	/// the workflow downstream did nothing, so a delivered row with an
	/// unexpected body is the only DB-side evidence of that failure mode.
	pub last_response: Option<String>,
	/// Set when the drainer has stopped retrying this row (max attempts
	/// hit). Distinct from `delivered_at` — gave-up rows never reached
	/// Slack. The pending index excludes both, so `claim_pending` won't
	/// pick them up again.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub gave_up_at: Option<Timestamp>,
	/// Earliest time `claim_pending` will return this row. Resolves are
	/// enqueued with `deliver_after = now` (ship immediately); opens use
	/// `now + group.slack_open_delay` so flap-and-resolve incidents can be
	/// cancelled before they hit Slack.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub deliver_after: Timestamp,
}

impl SlackOutbox {
	/// Insert a new outbox row. Caller is expected to be inside the
	/// transaction that's mutating the underlying incident/issue/note so
	/// the outbox row and the state change commit (or roll back) together.
	///
	/// `deliver_after` is the earliest time the drainer is allowed to ship
	/// this row. Most callers pass `Timestamp::now()`; the open path
	/// passes `now + group.slack_open_delay`.
	pub async fn enqueue(
		db: &mut AsyncPgConnection,
		kind: &str,
		incident_id: Option<Uuid>,
		issue_id: Option<Uuid>,
		note_id: Option<Uuid>,
		payload: JsonValue,
		deliver_after: Timestamp,
	) -> Result<Self> {
		use crate::schema::slack_outbox;
		diesel::insert_into(slack_outbox::table)
			.values((
				slack_outbox::kind.eq(kind),
				slack_outbox::incident_id.eq(incident_id),
				slack_outbox::issue_id.eq(issue_id),
				slack_outbox::note_id.eq(note_id),
				slack_outbox::payload.eq(payload),
				slack_outbox::deliver_after.eq(jiff_diesel::Timestamp::from(deliver_after)),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}

	/// Mark every still-pending `incident_open` row for this incident as
	/// given-up, with a reason that documents why the cancellation
	/// happened. Returns the number of rows affected — a non-zero result
	/// means the open never reached Slack and the caller should skip the
	/// resolve enqueue too (we don't want to "resolve" something Slack
	/// never heard about). Rows the drainer has already delivered, or has
	/// already given up on, are untouched.
	pub async fn cancel_pending_open(
		db: &mut AsyncPgConnection,
		incident_id: Uuid,
		reason: &str,
	) -> Result<usize> {
		use crate::schema::slack_outbox::dsl;
		let now = Timestamp::now();
		let rows = diesel::update(
			dsl::slack_outbox
				.filter(dsl::incident_id.eq(Some(incident_id)))
				.filter(dsl::kind.eq(KIND_INCIDENT_OPEN))
				.filter(dsl::delivered_at.is_null())
				.filter(dsl::gave_up_at.is_null()),
		)
		.set((
			dsl::gave_up_at.eq(jiff_diesel::Timestamp::from(now)),
			dsl::last_error.eq(reason),
		))
		.execute(db)
		.await
		.map_err(AppError::from)?;
		Ok(rows)
	}

	/// Mark every still-pending `incident_reminder` row for this incident as
	/// given-up. Called when the incident closes: a reminder that was waiting
	/// out lingering never ships. Unlike
	/// [`cancel_pending_open`](Self::cancel_pending_open) this says nothing
	/// about whether the resolve is owed, since a reminder is only ever queued
	/// after the opening was delivered.
	// spec: INC#notification
	pub async fn cancel_pending_reminders(
		db: &mut AsyncPgConnection,
		incident_id: Uuid,
	) -> Result<usize> {
		use crate::schema::slack_outbox::dsl;
		diesel::update(
			dsl::slack_outbox
				.filter(dsl::incident_id.eq(Some(incident_id)))
				.filter(dsl::kind.eq(KIND_INCIDENT_REMINDER))
				.filter(dsl::delivered_at.is_null())
				.filter(dsl::gave_up_at.is_null()),
		)
		.set((
			dsl::gave_up_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
			dsl::last_error.eq("cancelled: incident closed before the reminder was delivered"),
		))
		.execute(db)
		.await
		.map_err(AppError::from)
	}

	/// The payload this row should post if it were sent now. An opening,
	/// escalation, or reminder is re-rendered from the incident's current
	/// membership; every other kind posts what it was enqueued with.
	// spec: INC#notification
	pub async fn render_current(&self, db: &mut AsyncPgConnection) -> Result<JsonValue> {
		let reminder_at = match self.kind.as_str() {
			KIND_INCIDENT_OPEN => None,
			KIND_INCIDENT_REMINDER => Some(Timestamp::now()),
			_ => return Ok(self.payload.clone()),
		};
		let Some(incident_id) = self.incident_id else {
			return Ok(self.payload.clone());
		};
		summary::render(db, incident_id, reminder_at).await
	}

	/// Record the payload a row is about to be posted with, so the row shows
	/// what was sent rather than the snapshot it was queued with.
	pub async fn set_payload(
		db: &mut AsyncPgConnection,
		id: Uuid,
		payload: &JsonValue,
	) -> Result<()> {
		use crate::schema::slack_outbox::dsl;
		diesel::update(dsl::slack_outbox.filter(dsl::id.eq(id)))
			.set(dsl::payload.eq(payload))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// For each of `incident_ids`, return the `deliver_after` of its
	/// pending `incident_open` row — i.e. an incident that has been opened
	/// in the database but whose Slack notice hasn't been sent yet (and
	/// isn't given up on). Incidents whose open has already shipped, was
	/// given up, or whose `deliver_after` has elapsed are absent from the
	/// map. The UI uses this to distinguish "open and the world knows" from
	/// "open but still inside the flap-suppression window".
	pub async fn pending_opens_until(
		db: &mut AsyncPgConnection,
		incident_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Timestamp>> {
		use crate::schema::slack_outbox::dsl;
		use std::collections::HashMap;

		if incident_ids.is_empty() {
			return Ok(HashMap::new());
		}
		let now = Timestamp::now();
		let rows: Vec<(Uuid, jiff_diesel::Timestamp)> = dsl::slack_outbox
			.select((dsl::incident_id.assume_not_null(), dsl::deliver_after))
			.filter(dsl::kind.eq(KIND_INCIDENT_OPEN))
			.filter(dsl::incident_id.eq_any(incident_ids.iter().map(|i| Some(*i))))
			.filter(dsl::delivered_at.is_null())
			.filter(dsl::gave_up_at.is_null())
			.filter(dsl::deliver_after.gt(jiff_diesel::Timestamp::from(now)))
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().map(|(id, t)| (id, t.into())).collect())
	}

	/// Of `incident_ids`, those whose `incident_open` notice was actually
	/// delivered to Slack — i.e. operators were paged. This is the
	/// authoritative "the incident surfaced" signal: it excludes opens still
	/// held in the grace window and opens cancelled or given up before
	/// delivery. (Escalation produces a delivered open, so it's included.)
	pub async fn delivered_open_ids(
		db: &mut AsyncPgConnection,
		incident_ids: &[Uuid],
	) -> Result<std::collections::HashSet<Uuid>> {
		use crate::schema::slack_outbox::dsl;
		use std::collections::HashSet;

		if incident_ids.is_empty() {
			return Ok(HashSet::new());
		}
		let rows: Vec<Uuid> = dsl::slack_outbox
			.select(dsl::incident_id.assume_not_null())
			.filter(dsl::kind.eq(KIND_INCIDENT_OPEN))
			.filter(dsl::incident_id.eq_any(incident_ids.iter().map(|i| Some(*i))))
			.filter(dsl::delivered_at.is_not_null())
			.load(db)
			.await
			.map_err(AppError::from)?;
		Ok(rows.into_iter().collect())
	}

	/// Claim up to `limit` pending rows in insertion order. Uses
	/// `FOR UPDATE SKIP LOCKED` so multiple workers (or a worker plus a
	/// hand-run reprocess) won't fight over the same row. The caller must
	/// hold these inside its own transaction and call
	/// [`mark_delivered`](Self::mark_delivered),
	/// [`mark_failed`](Self::mark_failed), or
	/// [`mark_given_up`](Self::mark_given_up) before committing.
	pub async fn claim_pending(db: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Self>> {
		use crate::schema::{incidents, slack_outbox::dsl};
		use diesel::dsl::{exists, not};
		let now = Timestamp::now();
		dsl::slack_outbox
			.select(Self::as_select())
			.filter(dsl::delivered_at.is_null())
			.filter(dsl::gave_up_at.is_null())
			.filter(dsl::deliver_after.le(jiff_diesel::Timestamp::from(now)))
			// An `incident_open` or `incident_reminder` for a lingering
			// incident (its last effective failure has left, the linger window
			// hasn't elapsed) must not ship: a one-off blip would otherwise
			// notify purely because the linger held the incident open past its
			// `deliver_after`. The row stays pending — it ships when a failure
			// returns, or is cancelled when the linger expires and the
			// incident closes.
			// spec: INC#notification
			.filter(not(dsl::kind
				.eq_any([KIND_INCIDENT_OPEN, KIND_INCIDENT_REMINDER])
				.and(exists(
					incidents::table
						.filter(incidents::id.nullable().eq(dsl::incident_id))
						.filter(incidents::closed_at.is_null())
						.filter(incidents::closing_at.is_not_null()),
				))))
			.order(dsl::created_at.asc())
			.limit(limit)
			.for_update()
			.skip_locked()
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// `response` is the raw HTTP body Slack returned (may be empty). It's
	/// recorded so postmortems can tell a real delivery apart from
	/// "Slack 2xx'd the trigger and the workflow did nothing".
	pub async fn mark_delivered(
		db: &mut AsyncPgConnection,
		id: Uuid,
		response: &str,
	) -> Result<()> {
		use crate::schema::slack_outbox::dsl;
		diesel::update(dsl::slack_outbox.filter(dsl::id.eq(id)))
			.set((
				dsl::delivered_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
				dsl::last_error.eq(None::<String>),
				dsl::last_response.eq(response),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}

	/// Record a failed delivery attempt and hold the row back until its
	/// backoff has elapsed. Returns the new attempt count.
	///
	/// Advancing `deliver_after` is what makes the retry budget a *duration*
	/// rather than a handful of ticks: the drainer re-claims on a 5-second
	/// tick, so without it every attempt is burnt inside a minute and a
	/// routine Slack incident permanently drops every page enqueued during
	/// it.
	///
	/// `response` is the HTTP body Slack returned for this attempt, if we
	/// got one (network errors before any response leave this `None`).
	pub async fn mark_failed(
		db: &mut AsyncPgConnection,
		id: Uuid,
		error: &str,
		response: Option<&str>,
	) -> Result<i32> {
		use crate::schema::slack_outbox::dsl;
		let attempts: i32 = diesel::update(dsl::slack_outbox.filter(dsl::id.eq(id)))
			.set((
				dsl::attempts.eq(dsl::attempts + 1),
				dsl::last_error.eq(error),
				dsl::last_response.eq(response),
			))
			.returning(dsl::attempts)
			.get_result(db)
			.await
			.map_err(AppError::from)?;

		let next_try = Timestamp::now() + retry_backoff(attempts);
		diesel::update(dsl::slack_outbox.filter(dsl::id.eq(id)))
			.set(dsl::deliver_after.eq(jiff_diesel::Timestamp::from(next_try)))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(attempts)
	}

	/// Mark the row as terminally given-up — the drainer will not retry
	/// it. Sets `gave_up_at` so the pending index excludes it. Leaves
	/// `last_error` / `last_response` as the operator-visible record of
	/// why we stopped.
	pub async fn mark_given_up(db: &mut AsyncPgConnection, id: Uuid, error: &str) -> Result<()> {
		use crate::schema::slack_outbox::dsl;
		diesel::update(dsl::slack_outbox.filter(dsl::id.eq(id)))
			.set((
				dsl::gave_up_at.eq(jiff_diesel::Timestamp::from(Timestamp::now())),
				dsl::last_error.eq(error),
			))
			.execute(db)
			.await
			.map_err(AppError::from)?;
		Ok(())
	}
}
