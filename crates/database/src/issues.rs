//! Issues, events, incidents.

use commons_errors::{AppError, Result};
use commons_types::namespace::Namespace;
use commons_types::server::rank::ServerRank;
use commons_types::status::CheckResult;
use commons_types::subject::CheckSubject;
use diesel::prelude::*;
use diesel_async::{AsyncConnection, AsyncPgConnection, RunQueryDsl};
use jiff::{SignedDuration, Timestamp};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{applications::Application, devices::Device, server_groups::ServerGroup};

/// A tracked problem or condition on a server, or on a server group as a
/// whole. An issue is opened the first time its source reports it and stays
/// open (accumulating events) until the source reports it as no longer
/// active or an operator resolves it. Issues are the basic unit that drives
/// incidents, Slack notifications, and the fleet health view.
#[derive(
	Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Associations, utoipa::ToSchema,
)]
#[diesel(belongs_to(Application))]
#[diesel(belongs_to(Device))]
#[diesel(table_name = crate::schema::issues)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Issue {
	/// Unique identifier for this issue.
	pub id: Uuid,
	/// When this issue was first created.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	/// When this issue was last modified.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// The application this issue is attached to. `None` unless this is an
	/// application-scoped issue; at most one of the three scope columns is set,
	/// and all three null is canopy-wide.
	pub application_id: Option<Uuid>,
	/// The machine this issue is attached to, for a check that asserts
	/// something about the box rather than about a workload on it — disk,
	/// memory, clock, the tailnet. One degraded machine check is one issue at
	/// machine scope, not one per application on the host.
	pub machine_id: Option<Uuid>,
	/// The server group this issue is attached to, for a control-plane issue
	/// (e.g. backup corruption, a failed preflight check) that isn't tied to
	/// any single application or machine. Group-scoped issues are always
	/// considered even if an individual member has monitoring turned off.
	pub server_group_id: Option<Uuid>,
	/// The device that reported this issue, if it was raised by a device
	/// push. `None` for issues raised by an operator or by the platform
	/// itself.
	pub device_id: Option<Uuid>,
	/// Identifies what raised this issue — a healthcheck, a backup pipeline,
	/// an operator, etc. Used together with `ref` to detect repeat reports
	/// of the same underlying problem.
	pub source: String,
	/// A caller-chosen identifier for this issue within its `source`. The
	/// same `(source, ref)` pair reported again updates this issue instead
	/// of creating a new one.
	#[diesel(column_name = "ref_")]
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// A short, single-line title for the issue, shown as its headline in
	/// the UI and in Slack notifications. `None` if no title was given.
	pub description: Option<String>,
	/// The full body text describing the issue.
	pub message: String,
	/// Whether the condition behind this issue is still ongoing. Set to
	/// `false` when the source reports the condition has cleared.
	pub active: bool,
	/// When this issue was first reported.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub first_seen: Timestamp,
	/// When this issue was most recently reported or updated.
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub last_seen: Timestamp,
	/// When an operator marked this issue resolved. `None` if it hasn't
	/// been resolved.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub resolved_at: Option<Timestamp>,
	/// The operator who resolved this issue. `None` if it hasn't been
	/// resolved.
	pub resolved_by: Option<String>,
	/// The reason given when the issue was resolved (for example: fixed,
	/// false positive, won't fix). `None` if it hasn't been resolved.
	pub resolved_reason: Option<String>,
	/// If set, this issue is snoozed until this time: it's temporarily
	/// excluded from incidents and notifications even while still active.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub snoozed_until: Option<Timestamp>,
	/// The check this issue tracks, for issues that are check state (the
	/// ref minus its namespace prefix). `None` for issues that predate the
	/// check-state model or that no filing has stamped yet.
	pub check_name: Option<String>,
	/// The result the source reported on the latest filing, before policy.
	#[diesel(deserialize_as = MaybeCheckResult, serialize_as = Option<String>)]
	pub observed_result: Option<CheckResult>,
	/// What policy made of the latest observed result. This is the result
	/// canopy acts on; the transitional `severity` is derived from it.
	#[diesel(deserialize_as = MaybeCheckResult, serialize_as = Option<String>)]
	pub effective_result: Option<CheckResult>,
	/// The check's own fields from the latest report, verbatim (minus the
	/// reserved keys), for display alongside the state.
	pub detail: Option<serde_json::Value>,
	/// When the current degradation streak began, for state that is
	/// currently degraded (effective warning/failed/broken). `None` while
	/// healthy; a recovery clears it and a later degradation starts a
	/// fresh streak.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub degraded_since: Option<Timestamp>,
	/// When this state last filed degraded. Never cleared: distinguishes
	/// a recovered issue (worth listing) from always-healthy check state.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub last_degraded_at: Option<Timestamp>,
	/// Whether this check's policy escalates: an effective failure
	/// notifies immediately, bypassing incident grace. Stamped from the
	/// catalog on every filing.
	pub escalates: bool,
}

impl Issue {
	/// Does this state open an incident on its own? An effective failure
	/// does; anything less joins an already-open incident but doesn't
	/// create (or hold open) one.
	pub fn opens_incident(&self) -> bool {
		self.effective_result == Some(CheckResult::Failed)
	}

	/// Is this state an escalating failure right now — one whose
	/// notification bypasses the incident grace period?
	pub fn escalates_now(&self) -> bool {
		self.opens_incident() && self.escalates
	}
}

/// Diesel helper: a nullable text column read as an optional
/// [`CheckResult`], treating unparseable text as `None` rather than
/// failing the whole row load.
pub struct MaybeCheckResult(Option<CheckResult>);

impl From<MaybeCheckResult> for Option<CheckResult> {
	fn from(v: MaybeCheckResult) -> Self {
		v.0
	}
}

impl
	diesel::deserialize::Queryable<
		diesel::sql_types::Nullable<diesel::sql_types::Text>,
		diesel::pg::Pg,
	> for MaybeCheckResult
{
	type Row = Option<String>;

	fn build(row: Option<String>) -> diesel::deserialize::Result<Self> {
		Ok(Self(row.and_then(|s| s.parse().ok())))
	}
}

/// Which incidents a listing wants. `Open` and `Resolved` overlap: an
/// operator can resolve an incident that is still open.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum IncidentStatusFilter {
	#[default]
	All,
	/// Not yet closed.
	Open,
	/// Resolved by an operator.
	Resolved,
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Associations)]
#[diesel(belongs_to(ServerGroup, foreign_key = server_group_id))]
#[diesel(table_name = crate::schema::incidents)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct Incident {
	pub id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub created_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub updated_at: Timestamp,
	/// The group this incident targets, or `None` for a canopy-wide
	/// incident (aggregating canopy-wide issues — self-alerts).
	pub server_group_id: Option<Uuid>,
	/// Which of the group's environments this incident targets. `None` with a
	/// group is the group itself: its own checks, and the members of a group
	/// with no ranked application.
	// spec: INC#targets
	pub rank: Option<ServerRank>,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub opened_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub closed_at: Option<Timestamp>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub resolved_at: Option<Timestamp>,
	pub resolved_by: Option<String>,
	pub resolved_reason: Option<String>,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub escalated_at: Option<Timestamp>,
	/// When the incident's last effective failure left and lingering began.
	/// `None` while a failure is live. A failure returning clears it (the
	/// same incident continues); otherwise the linger sweep closes the
	/// incident once this is older than the target's linger window,
	/// backdating `closed_at` to it. Not cleared by the close itself.
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub closing_at: Option<Timestamp>,
}

#[derive(Clone, Debug, Serialize, Deserialize, Queryable, Selectable, Associations)]
#[diesel(belongs_to(Incident))]
#[diesel(belongs_to(Issue))]
#[diesel(table_name = crate::schema::incident_issues)]
#[diesel(check_for_backend(diesel::pg::Pg))]
pub struct IncidentIssue {
	pub incident_id: Uuid,
	pub issue_id: Uuid,
	#[diesel(deserialize_as = jiff_diesel::Timestamp, serialize_as = jiff_diesel::Timestamp)]
	pub joined_at: Timestamp,
	#[diesel(deserialize_as = jiff_diesel::NullableTimestamp, serialize_as = jiff_diesel::NullableTimestamp)]
	pub left_at: Option<Timestamp>,
}

/// A single occurrence to report against an issue, sent either by a device
/// or by an operator. If an issue with the same `source` and `ref` is
/// already open, this occurrence is folded into it (updating its state and
/// bumping its last-seen time); otherwise a new issue is created.
#[derive(Debug, Clone, Deserialize, utoipa::ToSchema)]
#[serde(rename_all = "camelCase")]
pub struct NewEvent {
	/// Identifies what is reporting this event — a healthcheck, a backup
	/// pipeline, an operator, etc.
	pub source: String,
	/// A caller-chosen identifier for this specific problem within `source`,
	/// used to detect repeated reports of the same underlying condition.
	/// Required — mint a UUID if deduplication isn't needed.
	#[serde(rename = "ref")]
	pub r#ref: String,
	/// A short, single-line title for this event, shown as the issue's
	/// headline in the UI and as the subject of any Slack notification.
	/// Must not contain newlines. Use `message` for the full body text.
	#[serde(default)]
	pub description: Option<String>,
	/// The full body text for this event. Free-form, multi-line text is
	/// fine. If `description` is omitted, the UI falls back to using the
	/// first line of this field as the headline.
	pub message: String,
	/// Whether the condition this event describes is still ongoing.
	/// Defaults to `true`. Sending `false` marks the condition as cleared,
	/// which can close the issue's contribution to any open incident.
	#[serde(default)]
	pub active: Option<bool>,
	/// When the event actually happened, if known and different from when
	/// it was received. Defaults to the time the event was received.
	#[serde(default)]
	pub occurred_at: Option<Timestamp>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum IssueFilter {
	#[default]
	ActiveOnly,
	All,
}

/// Multi-field filter for the cross-server issues list (global view).
/// Each field is opt-in: `Default` matches everything currently active.
#[derive(Debug, Clone, Default)]
pub struct IssueListFilters {
	pub active_only: bool,
	/// Restrict to issues whose latest effective result is one of these.
	pub results: Option<Vec<CheckResult>>,
	/// Restrict to issues whose target belongs to this group.
	pub server_group_id: Option<Uuid>,
	/// Restrict to issues raised against this application, together with those
	/// of the machine it runs on: a box's disk filling is the application's
	/// problem too, and is presented against it (see CHK, "Health rollup").
	pub application_id: Option<Uuid>,
	/// Restrict to issues raised against this machine, its own only.
	pub machine_id: Option<Uuid>,
	/// When `Some`, restrict to issues last seen at or after this time.
	pub since: Option<Timestamp>,
}

/// The check-state stamp accompanying a filing that is a check result:
/// which check, both sides of the policy transform, and the check's own
/// detail from the report. Filings that aren't check results (device
/// event pushes, producers not yet on the check-state model) carry no
/// stamp and leave the state columns null.
#[derive(Debug, Clone)]
pub struct CheckStateStamp {
	pub check: String,
	pub observed: CheckResult,
	pub effective: CheckResult,
	/// Whether the check's policy escalates (see [`Issue::escalates`]).
	pub escalates: bool,
	pub detail: Option<serde_json::Value>,
}

/// Write a filing's check-state stamp onto its issue row, maintaining the
/// degraded-streak timestamps: `degraded_since` holds while degraded
/// (starting a fresh streak on the healthy → degraded transition, using
/// `prior` — the row's value before this filing), clears on recovery;
/// `last_degraded_at` never clears. Returns the updated row.
async fn stamp_check_state(
	conn: &mut AsyncPgConnection,
	issue_id: Uuid,
	prior: Option<(Option<Timestamp>, Option<Timestamp>)>,
	stamp: &CheckStateStamp,
	at: Timestamp,
) -> Result<Issue> {
	use crate::schema::issues;

	let degraded = matches!(
		stamp.effective,
		CheckResult::Warning | CheckResult::Failed | CheckResult::Broken
	);
	let (prior_degraded_since, prior_last_degraded) = prior.unwrap_or((None, None));
	let degraded_since = if degraded {
		Some(jiff_diesel::Timestamp::from(
			prior_degraded_since.unwrap_or(at),
		))
	} else {
		None
	};
	let last_degraded_at = if degraded {
		Some(jiff_diesel::Timestamp::from(at))
	} else {
		prior_last_degraded.map(jiff_diesel::Timestamp::from)
	};
	let issue = diesel::update(issues::table.filter(issues::id.eq(issue_id)))
		.set((
			issues::check_name.eq(&stamp.check),
			issues::observed_result.eq(stamp.observed.to_string()),
			issues::effective_result.eq(stamp.effective.to_string()),
			issues::escalates.eq(stamp.escalates),
			issues::detail.eq(&stamp.detail),
			issues::degraded_since.eq(degraded_since),
			issues::last_degraded_at.eq(last_degraded_at),
		))
		.returning(Issue::as_select())
		.get_result(conn)
		.await
		.map_err(AppError::from)?;
	// Every stamped filing also feeds the state's stability record (from
	// the observed result, so policy never feeds back into it).
	crate::stability::record_observation(conn, issue_id, stamp.observed, at).await?;
	Ok(issue)
}

impl NewEvent {
	/// Persist this event push:
	/// 1. find-or-create the issue keyed by (application_id, source, ref),
	///    updating its state from this report,
	/// 2. if the server is in a group: (re)evaluate incident contribution.
	///
	/// `application_id` is the server the issue is attached to: derived from the
	/// device for public submissions, supplied by the operator for manual.
	/// `device_id` is `None` for manual events.
	///
	/// Issues from an **ungrouped** server are still recorded — the issue
	/// row goes in just like any other push — but the incident
	/// flow is skipped (`server_group_id` would have nowhere to point).
	/// When the operator later assigns the server to a group,
	/// [`Application::assign_to_group`] runs `reevaluate_open_issues_for_server`
	/// over the now-grouped issues so anything that should have opened an
	/// incident does so retroactively.
	pub async fn save(
		self,
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		device_id: Option<Uuid>,
	) -> Result<Issue> {
		self.save_with_state(db, application_id, device_id, None, false)
			.await
	}

	/// [`Self::save`], stamping the issue's check-state columns from a
	/// check result. Status ingestion passes the stamp; everything else
	/// files via [`Self::save`].
	pub async fn save_with_state(
		self,
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		device_id: Option<Uuid>,
		state: Option<&CheckStateStamp>,
		defer_incident_eval: bool,
	) -> Result<Issue> {
		use crate::schema::issues;

		// description is the single-line title; reject multi-line input
		// up front so the UI never has to fight with a multi-line
		// "headline".
		if let Some(d) = self.description.as_deref()
			&& d.contains('\n')
		{
			return Err(AppError::BadRequest(
				"description must be a single line (no newlines); use `message` for body text"
					.into(),
			));
		}

		let active = self.active.unwrap_or(true);
		let now = Timestamp::now();
		let effective_time = self.occurred_at.unwrap_or(now);
		let description = self.description.as_deref();

		// Look up the server's group up-front; needed by the incident-open
		// path. If `None`, we still record the issue/event but skip
		// incident logic — see `reevaluate_open_issues_for_server` for the
		// catch-up path. `monitored` gates the incident workflow the same
		// way: ungrouped *or* unmonitored → issue/event still recorded,
		// but no incident contribution.
		let server = Application::get_by_id(db, application_id).await?;
		let server_group_id = server.group_id;
		let server_rank = server.rank;
		let monitored = server.is_monitored;

		db.transaction::<_, AppError, _>(async |conn| {
			// 1. find-or-create issue (FOR UPDATE so concurrent pushes on
			//    the same (server, source, ref) serialize cleanly).
			let existing: Option<Issue> = issues::table
				.select(Issue::as_select())
				.filter(
					issues::application_id
						.eq(application_id)
						.and(issues::source.eq(&self.source))
						.and(issues::ref_.eq(&self.r#ref)),
				)
				.for_update()
				.first(conn)
				.await
				.optional()?;

			let issue: Issue = if let Some(existing) = existing {
				let new_last_seen = if effective_time > existing.last_seen {
					effective_time
				} else {
					existing.last_seen
				};
				// Sentry-style reopen: a device event with `active = true` against an
				// operator-resolved issue clears the resolved_* fields (issue is back
				// in unresolved state).
				let clear_resolved = active && existing.resolved_at.is_some();
				if let Some(stamp) = state {
					stamp_check_state(
						conn,
						existing.id,
						Some((existing.degraded_since, existing.last_degraded_at)),
						stamp,
						effective_time,
					)
					.await?;
				}
				diesel::update(issues::table.filter(issues::id.eq(existing.id)))
					.set((
						issues::device_id.eq(device_id),
						issues::description.eq(description),
						issues::message.eq(&self.message),
						issues::active.eq(active),
						issues::last_seen.eq(jiff_diesel::Timestamp::from(new_last_seen)),
						issues::resolved_at.eq(diesel::dsl::sql::<
							diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
						>(if clear_resolved {
							"NULL"
						} else {
							"issues.resolved_at"
						})),
						issues::resolved_by.eq(diesel::dsl::sql::<
							diesel::sql_types::Nullable<diesel::sql_types::Text>,
						>(if clear_resolved {
							"NULL"
						} else {
							"issues.resolved_by"
						})),
						issues::resolved_reason.eq(diesel::dsl::sql::<
							diesel::sql_types::Nullable<diesel::sql_types::Text>,
						>(if clear_resolved {
							"NULL"
						} else {
							"issues.resolved_reason"
						})),
					))
					.returning(Issue::as_select())
					.get_result(conn)
					.await?
			} else {
				let inserted: Issue = diesel::insert_into(issues::table)
					.values((
						issues::application_id.eq(application_id),
						issues::device_id.eq(device_id),
						issues::source.eq(&self.source),
						issues::ref_.eq(&self.r#ref),
						issues::description.eq(description),
						issues::message.eq(&self.message),
						issues::active.eq(active),
						issues::first_seen.eq(jiff_diesel::Timestamp::from(effective_time)),
						issues::last_seen.eq(jiff_diesel::Timestamp::from(effective_time)),
					))
					.returning(Issue::as_select())
					.get_result(conn)
					.await?;
				match state {
					Some(stamp) => {
						stamp_check_state(conn, inserted.id, None, stamp, effective_time).await?
					}
					None => inserted,
				}
			};

			// 2. (re-)evaluate incident contribution against the new issue state.
			//    `by = None`: this came from a device push, not an operator action.
			//    Skipped when the server is ungrouped: incidents are group-keyed.
			//    Skipped when `defer_incident_eval`: the high-frequency device
			//    ingest path enqueues the server for the reeval worker instead
			//    (see `enqueue_incident_reeval`), keeping the per-group lock off
			//    the request path.
			if let Some(gid) = server_group_id
				&& !defer_incident_eval
			{
				let target = IncidentTarget::of_member(gid, server_rank);
				re_evaluate_incident_membership(
					conn,
					&issue,
					target,
					monitored,
					effective_time,
					None,
				)
				.await?;
			}

			Ok(issue)
		})
		.await
	}
}

/// Open (or recover) a **group-scoped** issue, bypassing the per-server
/// `is_monitored` gate. This is the single entrypoint for control-plane
/// concerns that are not attributable to any one server — backup corruption,
/// maintenance failures, upstream preflight failures. A finding that names a
/// server belongs on that server even when detecting it took group-wide
/// state; scope is not the lever for making something page.
///
/// Mirrors [`NewEvent::save`] but keys the issue on
/// `(server_group_id, source, ref)` instead of `(application_id, …)`:
/// 1. find-or-create the group-scoped issue (application_id = NULL),
///    updating its state from this report,
/// 2. run incident membership evaluation with `monitored = true` so the
///    incident opens/pages regardless of any member server's monitored flag.
///
/// Recovery is the same `(source, ref)` with `active = false` at a lower
/// severity, which lets the issue leave the incident and auto-close —
/// identical lifecycle to the per-server path.
///
/// `description` is an optional single-line headline (rejected if multi-line);
/// `message` is the body.
pub async fn raise_group_event(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	r#ref: &str,
	description: Option<&str>,
	message: &str,
	active: bool,
) -> Result<Issue> {
	raise_group_event_with_state(conn, group_id, r#ref, description, message, active, None).await
}

/// [`raise_group_event`], stamping the issue's check-state columns.
pub async fn raise_group_event_with_state(
	conn: &mut AsyncPgConnection,
	group_id: Uuid,
	r#ref: &str,
	description: Option<&str>,
	message: &str,
	active: bool,
	state: Option<&CheckStateStamp>,
) -> Result<Issue> {
	use crate::schema::issues;

	if let Some(d) = description
		&& d.contains('\n')
	{
		return Err(AppError::BadRequest(
			"description must be a single line (no newlines); use `message` for body text".into(),
		));
	}

	let source = crate::statuses::CANOPY_SOURCE;
	let now = Timestamp::now();

	conn.transaction::<_, AppError, _>(async |conn| {
		// 1. find-or-create the group-scoped issue.
		let existing: Option<Issue> = issues::table
			.select(Issue::as_select())
			.filter(
				issues::server_group_id
					.eq(group_id)
					.and(issues::source.eq(source))
					.and(issues::ref_.eq(r#ref)),
			)
			.for_update()
			.first(conn)
			.await
			.optional()?;

		let prior_state = existing
			.as_ref()
			.map(|e| (e.degraded_since, e.last_degraded_at));
		let issue: Issue = if let Some(existing) = existing {
			let new_last_seen = if now > existing.last_seen {
				now
			} else {
				existing.last_seen
			};
			let clear_resolved = active && existing.resolved_at.is_some();
			diesel::update(issues::table.filter(issues::id.eq(existing.id)))
				.set((
					issues::description.eq(description),
					issues::message.eq(message),
					issues::active.eq(active),
					issues::last_seen.eq(jiff_diesel::Timestamp::from(new_last_seen)),
					issues::resolved_at.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_at"
					})),
					issues::resolved_by.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Text>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_by"
					})),
					issues::resolved_reason.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Text>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_reason"
					})),
				))
				.returning(Issue::as_select())
				.get_result(conn)
				.await?
		} else {
			diesel::insert_into(issues::table)
				.values((
					issues::server_group_id.eq(group_id),
					issues::source.eq(source),
					issues::ref_.eq(r#ref),
					issues::description.eq(description),
					issues::message.eq(message),
					issues::active.eq(active),
					issues::first_seen.eq(jiff_diesel::Timestamp::from(now)),
					issues::last_seen.eq(jiff_diesel::Timestamp::from(now)),
				))
				.returning(Issue::as_select())
				.get_result(conn)
				.await?
		};

		let issue = match state {
			Some(stamp) => stamp_check_state(conn, issue.id, prior_state, stamp, now).await?,
			None => issue,
		};

		// 2. group-aware incident evaluation — monitored = true unconditionally.
		re_evaluate_incident_membership(
			conn,
			&issue,
			IncidentTarget::Group(group_id),
			true,
			now,
			None,
		)
		.await?;

		Ok(issue)
	})
	.await
}

/// Open (or recover) a **machine-scoped** issue: one keyed
/// `(machine_id, source = canopy, ref)` under the machine partial unique
/// index.
///
/// A degraded machine check is one issue at machine scope, not one per
/// application on the box. The applications present it — it is theirs to
/// show, not theirs to own — so a two-workload host raises a single disk
/// failure rather than two.
///
/// Unlike its group and canopy-wide siblings, this takes the `source` rather
/// than assuming `canopy`. Those two file conditions canopy determines for
/// itself; a machine's checks arrive from a reporter like `alertd`, and
/// recording them under `canopy` would break per-source silences, the
/// `check_severities` a push answers with, source staleness, and the rule that
/// a source's push only recovers its own checks.
///
/// Mirrors [`raise_group_event_with_state`]'s find-or-create. Incident
/// evaluation resolves through the machine's group and carries the machine's
/// own `is_monitored`, so excusing a box from monitoring does not quiet the
/// applications on it.
// spec: CHK
#[allow(clippy::too_many_arguments)]
pub async fn raise_machine_event_with_state(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	source: &str,
	device_id: Option<Uuid>,
	r#ref: &str,
	description: Option<&str>,
	message: &str,
	active: bool,
	state: Option<&CheckStateStamp>,
) -> Result<Issue> {
	use crate::schema::issues;

	if let Some(d) = description
		&& d.contains('\n')
	{
		return Err(AppError::BadRequest(
			"description must be a single line (no newlines); use `message` for body text".into(),
		));
	}

	let now = Timestamp::now();

	conn.transaction::<_, AppError, _>(async |conn| {
		let existing: Option<Issue> = issues::table
			.select(Issue::as_select())
			.filter(
				issues::machine_id
					.eq(machine_id)
					.and(issues::source.eq(source))
					.and(issues::ref_.eq(r#ref)),
			)
			.for_update()
			.first(conn)
			.await
			.optional()?;

		let prior_state = existing
			.as_ref()
			.map(|e| (e.degraded_since, e.last_degraded_at));
		let issue: Issue = if let Some(existing) = existing {
			let new_last_seen = if now > existing.last_seen {
				now
			} else {
				existing.last_seen
			};
			let clear_resolved = active && existing.resolved_at.is_some();
			diesel::update(issues::table.filter(issues::id.eq(existing.id)))
				.set((
					issues::description.eq(description),
					issues::message.eq(message),
					issues::active.eq(active),
					issues::last_seen.eq(jiff_diesel::Timestamp::from(new_last_seen)),
					issues::resolved_at.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_at"
					})),
					issues::resolved_by.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Text>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_by"
					})),
					issues::resolved_reason.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Text>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_reason"
					})),
				))
				.returning(Issue::as_select())
				.get_result(conn)
				.await?
		} else {
			diesel::insert_into(issues::table)
				.values((
					issues::machine_id.eq(machine_id),
					issues::device_id.eq(device_id),
					issues::source.eq(source),
					issues::ref_.eq(r#ref),
					issues::description.eq(description),
					issues::message.eq(message),
					issues::active.eq(active),
					issues::first_seen.eq(jiff_diesel::Timestamp::from(now)),
					issues::last_seen.eq(jiff_diesel::Timestamp::from(now)),
				))
				.returning(Issue::as_select())
				.get_result(conn)
				.await?
		};

		let issue = match state {
			Some(stamp) => stamp_check_state(conn, issue.id, prior_state, stamp, now).await?,
			None => issue,
		};

		// An ungrouped machine has no incident target, exactly as an ungrouped
		// application has none.
		if let Some((target, monitored)) = Scope::Machine(machine_id)
			.resolve_incident_target(conn)
			.await?
		{
			re_evaluate_incident_membership(conn, &issue, target, monitored, now, None).await?;
		}

		Ok(issue)
	})
	.await
}

/// Open (or recover) a **canopy-wide** issue: one scoped to neither a
/// server nor a group, keyed `(source = canopy, ref)` under the global
/// partial unique index. This is the state store for canopy monitoring
/// its own operation (self-alerts).
///
/// Mirrors [`raise_group_event`]'s find-or-create; incident evaluation
/// runs against the global target, so canopy-wide issues get the full
/// incident lifecycle (grace, escalation, resolution) like any other.
pub async fn raise_global_event(
	conn: &mut AsyncPgConnection,
	r#ref: &str,
	description: Option<&str>,
	message: &str,
	active: bool,
) -> Result<Issue> {
	raise_global_event_with_state(conn, r#ref, description, message, active, None).await
}

/// [`raise_global_event`], stamping the issue's check-state columns.
pub async fn raise_global_event_with_state(
	conn: &mut AsyncPgConnection,
	r#ref: &str,
	description: Option<&str>,
	message: &str,
	active: bool,
	state: Option<&CheckStateStamp>,
) -> Result<Issue> {
	use crate::schema::issues;

	if let Some(d) = description
		&& d.contains('\n')
	{
		return Err(AppError::BadRequest(
			"description must be a single line (no newlines); use `message` for body text".into(),
		));
	}

	let source = crate::statuses::CANOPY_SOURCE;
	let now = Timestamp::now();

	conn.transaction::<_, AppError, _>(async |conn| {
		let existing: Option<Issue> = issues::table
			.select(Issue::as_select())
			.filter(
				issues::application_id
					.is_null()
					.and(issues::server_group_id.is_null())
					.and(issues::source.eq(source))
					.and(issues::ref_.eq(r#ref)),
			)
			.for_update()
			.first(conn)
			.await
			.optional()?;

		let prior_state = existing
			.as_ref()
			.map(|e| (e.degraded_since, e.last_degraded_at));
		let issue: Issue = if let Some(existing) = existing {
			let new_last_seen = if now > existing.last_seen {
				now
			} else {
				existing.last_seen
			};
			let clear_resolved = active && existing.resolved_at.is_some();
			diesel::update(issues::table.filter(issues::id.eq(existing.id)))
				.set((
					issues::description.eq(description),
					issues::message.eq(message),
					issues::active.eq(active),
					issues::last_seen.eq(jiff_diesel::Timestamp::from(new_last_seen)),
					issues::resolved_at.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Timestamptz>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_at"
					})),
					issues::resolved_by.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Text>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_by"
					})),
					issues::resolved_reason.eq(diesel::dsl::sql::<
						diesel::sql_types::Nullable<diesel::sql_types::Text>,
					>(if clear_resolved {
						"NULL"
					} else {
						"issues.resolved_reason"
					})),
				))
				.returning(Issue::as_select())
				.get_result(conn)
				.await?
		} else {
			diesel::insert_into(issues::table)
				.values((
					issues::source.eq(source),
					issues::ref_.eq(r#ref),
					issues::description.eq(description),
					issues::message.eq(message),
					issues::active.eq(active),
					issues::first_seen.eq(jiff_diesel::Timestamp::from(now)),
					issues::last_seen.eq(jiff_diesel::Timestamp::from(now)),
				))
				.returning(Issue::as_select())
				.get_result(conn)
				.await?
		};

		let issue = match state {
			Some(stamp) => stamp_check_state(conn, issue.id, prior_state, stamp, now).await?,
			None => issue,
		};

		// Global incident evaluation — always monitored.
		re_evaluate_incident_membership(conn, &issue, IncidentTarget::Global, true, now, None)
			.await?;

		Ok(issue)
	})
	.await
}

/// The scope a check-state, issue, or scoped policy attaches to: one
/// application, one machine, a server group, or canopy as a whole. This is the
/// single scope vocabulary for check state — do not reintroduce a parallel
/// scope enum or an ad-hoc `match (application_id, machine_id,
/// server_group_id)`; map through the methods here. Provenance (which device
/// reported a filing) is a separate concern (see `CheckFiling::device_id`), not
/// part of scope.
///
/// A machine is a target in its own right because a host's disk, memory and
/// clock assert something about the box, not about any one workload on it.
// spec: CHK
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scope {
	Application(Uuid),
	Machine(Uuid),
	Group(Uuid),
	Global,
}

impl Scope {
	/// The `(application_id, machine_id, server_group_id)` storage columns for
	/// this scope.
	pub fn to_columns(self) -> (Option<Uuid>, Option<Uuid>, Option<Uuid>) {
		match self {
			Scope::Application(id) => (Some(id), None, None),
			Scope::Machine(id) => (None, Some(id), None),
			Scope::Group(id) => (None, None, Some(id)),
			Scope::Global => (None, None, None),
		}
	}

	/// The scope encoded by an `(application_id, machine_id, server_group_id)`
	/// triple. The storage CHECK allows at most one to be set, so the arms are
	/// mutually exclusive in practice; the order below only decides what a row
	/// that violated the CHECK would read as, and all-null is canopy-wide.
	pub fn from_columns(
		application_id: Option<Uuid>,
		machine_id: Option<Uuid>,
		server_group_id: Option<Uuid>,
	) -> Self {
		match (application_id, machine_id, server_group_id) {
			(_, _, Some(gid)) => Scope::Group(gid),
			(_, Some(mid), None) => Scope::Machine(mid),
			(Some(sid), None, None) => Scope::Application(sid),
			(None, None, None) => Scope::Global,
		}
	}

	/// Resolve this scope to the incident target it contributes to and
	/// whether that contribution is monitored. An application or a machine
	/// maps to the environment it is in, carrying that target's own
	/// `is_monitored`; a group targets itself and a canopy-wide scope the
	/// global target, both always monitored. An ungrouped application or
	/// machine has no target and no incident path.
	///
	/// A machine carries its own monitoring switch, so excusing a box from
	/// monitoring does not quiet the applications on it, and vice versa.
	// spec: INC#targets
	pub async fn resolve_incident_target(
		self,
		conn: &mut AsyncPgConnection,
	) -> Result<Option<(IncidentTarget, bool)>> {
		match self {
			Scope::Group(gid) => Ok(Some((IncidentTarget::Group(gid), true))),
			Scope::Application(sid) => {
				let server = Application::get_by_id(conn, sid).await?;
				let Some(gid) = server.group_id else {
					return Ok(None);
				};
				let target = IncidentTarget::of_member(gid, server.rank);
				Ok(Some((target, server.is_monitored)))
			}
			Scope::Machine(mid) => {
				let machine = crate::machines::Machine::get_by_id(conn, mid).await?;
				let Some(gid) = machine.group_id else {
					return Ok(None);
				};
				// A box's rank is the highest of the workloads on it: a check
				// on the box is trouble for the most important thing it runs.
				let rank = crate::machines::Machine::rank(conn, mid).await?;
				let target = IncidentTarget::of_member(gid, rank);
				Ok(Some((target, machine.is_monitored)))
			}
			Scope::Global => Ok(Some((IncidentTarget::Global, true))),
		}
	}
}

/// The source operator-raised manual conditions file under.
///
/// Defined with the check namespace, since being curated by canopy is what
/// makes this source's names unqualified.
pub use commons_types::namespace::MANUAL_SOURCE;

/// One canopy-determined or operator-raised check result to file:
/// reachability, backup health, key expiry, self-monitoring, manual
/// conditions, and the like.
#[derive(Debug, Clone)]
pub struct CheckFiling<'a> {
	/// The reserved source this filing belongs to: [`MANUAL_SOURCE`] for
	/// operator-raised conditions (server scope only), `canopy` for
	/// canopy's own determinations.
	pub source: &'a str,
	pub scope: Scope,
	/// The reporting device, for server-scoped filings — provenance, not
	/// scope. `None` for operator/platform filings and non-server scopes.
	pub device_id: Option<Uuid>,
	/// The check's stable name (doubles as the issue ref under the
	/// source): a contract with stored silences.
	pub check: &'a str,
	/// What was observed this pass. Policy grades it from there.
	pub observed: CheckResult,
	/// Single-line headline for degraded filings.
	pub title: Option<&'a str>,
	pub message: &'a str,
	/// The check's own fields, available to policy rules as `check.*`
	/// and displayed alongside the state.
	pub detail: Option<serde_json::Value>,
	/// The policy this check registers with on first sight — the ceiling
	/// and escalation its condition warrants. Operator edits stick;
	/// these only seed the catalog row.
	pub default_ceiling: CheckResult,
	pub default_escalates: bool,
	/// The documentation canopy's own checks ship with, seeded into the
	/// catalog on first sight (never overwriting operator edits). See
	/// the CHK spec's Documentation section for the convention.
	pub documentation: Option<&'a str>,
}

/// File one canopy-determined or operator-raised check result: register
/// its catalog entry (first sight only), grade the observation through
/// the operator's policy, and upsert the check state at the right scope
/// — driving incident membership exactly like a device-reported check.
///
/// Group- and canopy-wide scopes are canopy's own (their raise paths
/// file under the `canopy` source); manual conditions are server-scoped.
///
/// Until issues themselves carry results, the effective result maps to
/// the issue severity the same way status ingestion does: failed →
/// error (critical when the policy escalates), warning/broken →
/// warning; passed and skipped record healthy state and close.
pub async fn file_check(conn: &mut AsyncPgConnection, filing: CheckFiling<'_>) -> Result<Issue> {
	let instance = CheckInstance {
		label: String::new(),
		observed: filing.observed,
		detail: filing.detail.clone(),
	};
	let message = filing.message;
	file_check_instances(
		conn,
		InstancedCheckFiling {
			source: filing.source,
			scope: filing.scope,
			device_id: filing.device_id,
			check: filing.check,
			title: filing.title,
			default_ceiling: filing.default_ceiling,
			default_escalates: filing.default_escalates,
			documentation: filing.documentation,
			instances: vec![instance],
		},
		&|_| message.to_string(),
	)
	.await
}

/// One instance of a check the target has several of: one of a server's
/// backup types, one of its restore intents.
///
/// Instances exist so a check can stay a single named category — see the
/// Names section of the CHK spec. Never spell an instance into the check
/// name.
#[derive(Debug, Clone)]
pub struct CheckInstance {
	/// How this instance is named in the check's message, e.g. the backup
	/// type. Not read by policy — the fields policy matches on live in
	/// `detail`.
	pub label: String,
	/// What this instance observed this pass.
	pub observed: CheckResult,
	/// This instance's own fields. Policy rules reach them as `check.*`, so
	/// whatever identifies the instance (`type`, `intent`, …) must be in
	/// here for an operator to be able to grade or silence it on its own.
	pub detail: Option<serde_json::Value>,
}

/// What grading an instance produced, for building the check's message.
#[derive(Debug, Clone)]
pub struct GradedInstance {
	pub label: String,
	pub observed: CheckResult,
	pub effective: CheckResult,
	pub detail: Option<serde_json::Value>,
}

/// A check filed from several instances. Everything except the per-instance
/// fields is shared, because there is one check and one catalog entry
/// however many instances the target has.
pub struct InstancedCheckFiling<'a> {
	pub source: &'a str,
	pub scope: Scope,
	pub device_id: Option<Uuid>,
	pub check: &'a str,
	pub title: Option<&'a str>,
	pub instances: Vec<CheckInstance>,
	pub default_ceiling: CheckResult,
	pub default_escalates: bool,
	pub documentation: Option<&'a str>,
}

/// File one check from several instances of its condition, grading each
/// instance on its own and settling on the most urgent of them.
///
/// This is what keeps a check a category instead of a name per instance: one
/// catalog entry, one state, one thing for an operator to configure, however
/// many backup types or restore intents the target has. Because each instance
/// is graded separately against its own detail, a rule or silence written for
/// one instance still applies to only that instance.
///
/// Skipped instances (a silence matched) drop out of the aggregate entirely
/// rather than counting as healthy. If every instance is skipped, so is the
/// check.
///
/// `message` is called with the instances that are not passing, most urgent
/// first, and is what the operator reads; it gets an empty slice when
/// everything is healthy, which is the recovery message.
pub async fn file_check_instances(
	conn: &mut AsyncPgConnection,
	filing: InstancedCheckFiling<'_>,
	message: &(dyn Fn(&[GradedInstance]) -> String + Sync),
) -> Result<Issue> {
	use crate::check_policies::{CheckPolicy, EvaluationContext, FilingScope, ScopedCheckPolicy};

	let source = filing.source;
	debug_assert!(
		matches!(filing.scope, Scope::Application(_) | Scope::Machine(_))
			|| source == crate::statuses::CANOPY_SOURCE,
		"group- and canopy-wide filings are canopy's own; a machine's come from its reporter",
	);
	debug_assert!(
		!filing.instances.is_empty(),
		"a filing with no instances says nothing; skip the call instead",
	);
	let status_extra = serde_json::Map::new();
	// The application type comes out of the same load the tags do, because the
	// check's namespace needs it: an application-subject check from a structured
	// source is one catalog entry per type.
	let mut application_type = None;
	let (tags, scope): (
		std::collections::HashMap<String, serde_json::Value>,
		FilingScope,
	) = match filing.scope {
		Scope::Application(application_id) => {
			let server = Application::get_by_id(conn, application_id).await?;
			let group_id = server.group_id;
			application_type = Some(server.r#type.clone());
			let tags = server
				.tags_merged_with_group(conn)
				.await?
				.0
				.into_iter()
				.map(|(k, v)| (k, serde_json::Value::String(v)))
				.collect();
			(
				tags,
				FilingScope {
					application_id: Some(application_id),
					group_id,
					// An application is covered by the window over its box.
					..Default::default()
				},
			)
		}
		// Graded against the machine's own tags, not against any application
		// that happens to run on it.
		Scope::Machine(machine_id) => {
			let machine = crate::machines::Machine::get_by_id(conn, machine_id).await?;
			let group_id = machine.group_id;
			let tags = machine
				.tags_merged_with_group(conn)
				.await?
				.0
				.into_iter()
				.map(|(k, v)| (k, serde_json::Value::String(v)))
				.collect();
			(
				tags,
				FilingScope {
					machine_id: Some(machine_id),
					group_id,
					..Default::default()
				},
			)
		}
		Scope::Group(group_id) => (
			Default::default(),
			FilingScope {
				group_id: Some(group_id),
				..Default::default()
			},
		),
		Scope::Global => (Default::default(), FilingScope::default()),
	};

	let namespace =
		Namespace::of(source, filing.check, application_type.as_ref()).ok_or_else(|| {
			AppError::Custom(format!(
				"{} from {source} is an application check, so it cannot be filed against {:?}",
				filing.check, filing.scope
			))
		})?;

	CheckPolicy::register(
		conn,
		source,
		&namespace,
		filing.check,
		filing.default_ceiling,
		filing.default_escalates,
		filing.documentation,
	)
	.await?;

	// The catalog entry and the scoped chain are properties of the check, not
	// of any one instance: read them once and grade purely from there, so a
	// server with a dozen backup types still costs two queries.
	let fleet = CheckPolicy::fleet_grading(conn, source, &namespace, filing.check).await?;
	let chain = ScopedCheckPolicy::chain_for(conn, source, &namespace, filing.check, scope).await?;

	let mut graded_instances: Vec<GradedInstance> = Vec::with_capacity(filing.instances.len());
	let mut escalates = false;
	for instance in &filing.instances {
		// Rule-evaluation context: the instance's detail (with the normalised
		// result injected, mirroring status ingestion), no report-wide extras,
		// and the server's tags where there is a server.
		let mut check_extra = instance
			.detail
			.as_ref()
			.and_then(|d| d.as_object().cloned())
			.unwrap_or_default();
		check_extra.insert(
			"result".into(),
			serde_json::Value::String(instance.observed.to_string()),
		);
		let ctx = EvaluationContext {
			status_extra: &status_extra,
			check_extra: &check_extra,
			tags: &tags,
		};
		let fleet_graded = CheckPolicy::grade(
			fleet.as_ref(),
			source,
			filing.check,
			instance.observed,
			&ctx,
		);
		let graded = CheckPolicy::chain_scoped(fleet_graded, &chain, &ctx);
		escalates |= graded.escalates;
		graded_instances.push(GradedInstance {
			label: instance.label.clone(),
			observed: instance.observed,
			effective: graded.effective,
			detail: instance.detail.clone(),
		});
	}

	// The check settles on its most urgent instance. A skipped instance is
	// silenced, not healthy, so it neither drags the check down nor props it
	// up; a check whose instances are all skipped is itself skipped.
	let considered: Vec<&GradedInstance> = graded_instances
		.iter()
		.filter(|i| i.effective != CheckResult::Skipped)
		.collect();
	let effective = considered
		.iter()
		.map(|i| i.effective)
		.min_by_key(|r| r.urgency_rank())
		.unwrap_or(CheckResult::Skipped);

	// The observed result is always recorded as observed, so it spans every
	// instance including the silenced ones: silencing a check changes what
	// Canopy acts on, never what it saw.
	let observed = graded_instances
		.iter()
		.map(|i| i.observed)
		.min_by_key(|r| r.urgency_rank())
		.unwrap_or(CheckResult::Skipped);

	let mut degraded: Vec<GradedInstance> = considered
		.iter()
		.filter(|i| i.effective != CheckResult::Passed)
		.map(|i| (*i).clone())
		.collect();
	degraded.sort_by_key(|i| i.effective.urgency_rank());

	let detail = instance_detail(&graded_instances, &degraded);
	let rendered = message(&degraded);

	let active = matches!(
		effective,
		CheckResult::Failed | CheckResult::Warning | CheckResult::Broken
	);
	let description = if active { filing.title } else { None };
	let stamp = CheckStateStamp {
		check: filing.check.to_string(),
		observed,
		effective,
		escalates,
		detail: detail.clone(),
	};
	let filing = CheckFiling {
		source,
		scope: filing.scope,
		device_id: filing.device_id,
		check: filing.check,
		observed,
		title: filing.title,
		message: &rendered,
		detail,
		default_ceiling: filing.default_ceiling,
		default_escalates: filing.default_escalates,
		documentation: filing.documentation,
	};

	match filing.scope {
		Scope::Application(application_id) => {
			NewEvent {
				source: source.to_string(),
				r#ref: filing.check.to_string(),
				description: description.map(str::to_string),
				message: filing.message.to_string(),
				active: Some(active),
				occurred_at: None,
			}
			.save_with_state(conn, application_id, filing.device_id, Some(&stamp), false)
			.await
		}
		Scope::Machine(machine_id) => {
			raise_machine_event_with_state(
				conn,
				machine_id,
				source,
				filing.device_id,
				filing.check,
				description,
				filing.message,
				active,
				Some(&stamp),
			)
			.await
		}
		Scope::Group(gid) => {
			raise_group_event_with_state(
				conn,
				gid,
				filing.check,
				description,
				filing.message,
				active,
				Some(&stamp),
			)
			.await
		}
		Scope::Global => {
			raise_global_event_with_state(
				conn,
				filing.check,
				description,
				filing.message,
				active,
				Some(&stamp),
			)
			.await
		}
	}
}

/// The detail an instanced check stores: every instance that is not passing,
/// each with its own result, plus how many there were in total. That is what
/// lets an operator see which backup types a server is stale for without
/// opening anything else — the thing a check name per type used to convey.
///
/// A single unlabelled instance is an ordinary check filed through this path,
/// so its detail passes straight through unchanged.
fn instance_detail(
	all: &[GradedInstance],
	degraded: &[GradedInstance],
) -> Option<serde_json::Value> {
	if let [only] = all
		&& only.label.is_empty()
	{
		return only.detail.clone();
	}
	if degraded.is_empty() {
		return None;
	}
	let instances: Vec<serde_json::Value> = degraded
		.iter()
		.map(|i| {
			let mut obj = i
				.detail
				.as_ref()
				.and_then(|d| d.as_object().cloned())
				.unwrap_or_default();
			obj.insert(
				"result".into(),
				serde_json::Value::String(i.effective.to_string()),
			);
			obj.entry("instance")
				.or_insert_with(|| serde_json::Value::String(i.label.clone()));
			serde_json::Value::Object(obj)
		})
		.collect();
	Some(serde_json::json!({
		"instances": instances,
		"degraded": degraded.len(),
		"total": all.len(),
	}))
}

/// Whether a `(source, check)` pair is Canopy's own reachability determination.
///
/// Reachability is the one check every grain determines for itself, so it is
/// the one check an application does not take from its machine.
// spec: CHK#reachability
fn is_reachability(source: &str, check: &str) -> bool {
	source == crate::statuses::CANOPY_SOURCE && check == crate::statuses::REACHABILITY_REF
}

/// One rollup input row: `(application_id, source, check_name, effective_result)`.
type HealthCheckRow = (Option<Uuid>, String, Option<String>, Option<String>);

/// Per-server health rollup from current check state: the worst
/// effective result across every source's checks on the server — any
/// failure ⇒ unhealthy, otherwise any warning or brokenness ⇒ warning,
/// otherwise healthy. Silenced checks are skipped. Servers with no
/// check state are absent from the map (callers default them healthy,
/// matching the "no signal" semantics).
pub async fn health_from_check_state(
	conn: &mut AsyncPgConnection,
	applications: &[(Uuid, Option<Uuid>)],
) -> Result<std::collections::HashMap<Uuid, commons_types::status::HealthState>> {
	use crate::schema::{issues, scoped_check_policies};
	use commons_types::status::HealthState;
	use std::collections::{HashMap, HashSet};

	if applications.is_empty() {
		return Ok(HashMap::new());
	}
	let server_ids: Vec<Uuid> = applications.iter().map(|(id, _)| *id).collect();
	let group_of: HashMap<Uuid, Option<Uuid>> = applications.iter().copied().collect();

	let rows: Vec<HealthCheckRow> = issues::table
		.select((
			issues::application_id,
			issues::source,
			issues::check_name,
			issues::effective_result,
		))
		.filter(issues::application_id.eq_any(&server_ids))
		.filter(issues::check_name.is_not_null())
		.filter(issues::effective_result.is_not_null())
		// Only states currently contributing count: an operator-resolved
		// or inactive state has stopped dragging the server down, even if
		// its last effective result was a failure (a source that went
		// stale mid-failure never re-graded it down).
		.filter(issues::active.eq(true))
		.filter(issues::resolved_at.is_null())
		.load(conn)
		.await?;

	// A check-state only contributes to health if a live catalog policy
	// backs it: this skips decommissioned checks and orphaned check-states
	// (no catalog row at all) fleet-wide. See `live_cataloged_pairs`.
	let cataloged = crate::check_policies::CheckPolicy::live_cataloged_pairs(conn).await?;
	// Which catalog row backs a state follows the application's type, so a check
	// decommissioned for one type stops counting there and nowhere else.
	let types = Application::types_by_id(conn, &server_ids).await?;

	let group_ids: Vec<Uuid> = group_of.values().filter_map(|g| *g).collect();
	let silence_rows: Vec<(
		Option<Uuid>,
		Option<Uuid>,
		Option<String>,
		Option<String>,
		String,
		String,
	)> = scoped_check_policies::table
		.select((
			scoped_check_policies::application_id,
			scoped_check_policies::server_group_id,
			scoped_check_policies::subject,
			scoped_check_policies::application_type,
			scoped_check_policies::source,
			scoped_check_policies::check_name,
		))
		.filter(scoped_check_policies::ceiling.eq("skipped"))
		.filter(
			scoped_check_policies::application_id
				.eq_any(&server_ids)
				.or(scoped_check_policies::server_group_id.eq_any(&group_ids)),
		)
		.load(conn)
		.await?;
	// The namespace is part of the key: a group spans several application
	// types, so a silence on one type's `version` leaves another type's alone.
	let mut server_silences: HashSet<(Uuid, Namespace, String, String)> = HashSet::new();
	let mut group_silences: HashSet<(Uuid, Namespace, String, String)> = HashSet::new();
	for (application_id, group_id, subject, ty, source, check) in silence_rows {
		let Ok(ns) = Namespace::from_columns(subject.as_deref(), ty.as_deref()) else {
			continue;
		};
		if let Some(sid) = application_id {
			server_silences.insert((sid, ns, source, check));
		} else if let Some(gid) = group_id {
			group_silences.insert((gid, ns, source, check));
		}
	}

	// Collect each server's surviving effective results, then roll up
	// through the one shared classifier — the same rules everywhere.
	let mut contributing: HashMap<Uuid, Vec<CheckResult>> = HashMap::new();
	for (application_id, source, check_name, effective) in rows {
		let Some(application_id) = application_id else {
			continue;
		};
		let Some(check_name) = check_name else {
			continue;
		};
		let key = (application_id, source, check_name);
		if !crate::check_policies::CheckPolicy::live_for(
			&cataloged,
			&key.1,
			&key.2,
			types.get(&application_id),
		) {
			continue;
		}
		let Some(ns) = Namespace::of(&key.1, &key.2, types.get(&application_id)) else {
			continue;
		};
		if server_silences.contains(&(key.0, ns.clone(), key.1.clone(), key.2.clone())) {
			continue;
		}
		if let Some(Some(gid)) = group_of.get(&application_id)
			&& group_silences.contains(&(*gid, ns, key.1.clone(), key.2.clone()))
		{
			continue;
		}
		let Some(result) = effective
			.as_deref()
			.and_then(|e| e.parse::<CheckResult>().ok())
		else {
			continue;
		};
		contributing.entry(application_id).or_default().push(result);
	}

	let mut health: HashMap<Uuid, HealthState> = contributing
		.into_iter()
		.map(|(application_id, results)| (application_id, HealthState::from_results(results)))
		.collect();

	// An application's contributing checks include its machine's, so a box
	// whose disk is filling makes every application on it degraded.
	//
	// The two rollups combine by taking the worse of the pair, which is the
	// same answer as classifying the union: `HealthState::from_results` is the
	// worst result over its input, and worst-of is associative. So the box's
	// checks are graded once for the box and read from each application on it,
	// rather than counted once per workload.
	// spec: CHK#health-rollup
	let machine_of = Application::machines_by_id(conn, &server_ids).await?;
	let mut machine_groups: Vec<(Uuid, Option<Uuid>)> = Vec::new();
	if !machine_of.is_empty() {
		let machine_ids: Vec<Uuid> = {
			let mut ids: Vec<Uuid> = machine_of.values().copied().collect();
			ids.sort_unstable();
			ids.dedup();
			ids
		};
		// A machine carries its own group, which is the group its silences are
		// read against; an application's need not be the same one.
		let rows: Vec<(Uuid, Option<Uuid>)> = crate::schema::machines::table
			.select((
				crate::schema::machines::id,
				crate::schema::machines::group_id,
			))
			.filter(crate::schema::machines::id.eq_any(&machine_ids))
			.load(conn)
			.await?;
		machine_groups = rows;
	}
	let machine_health = machine_health_rollup(conn, &machine_groups, false).await?;
	for (application_id, machine_id) in machine_of {
		let Some(from_machine) = machine_health.get(&machine_id).copied() else {
			continue;
		};
		health
			.entry(application_id)
			.and_modify(|held| *held = held.worse_of(from_machine))
			.or_insert(from_machine);
	}

	Ok(health)
}

/// The same rollup for machines, from the checks filed at machine scope.
///
/// A sibling of [`health_from_check_state`] rather than a generalisation of it:
/// the query differs only in which column names the target, and both end at
/// `HealthState::from_results`, so the two grains classify identically. A
/// machine's own health is its own checks; what an application makes of its
/// machine's checks is the application's rollup (see CHK, "Health rollup").
// spec: CHK#health-rollup
pub async fn machine_health_from_check_state(
	conn: &mut AsyncPgConnection,
	machines: &[(Uuid, Option<Uuid>)],
) -> Result<std::collections::HashMap<Uuid, commons_types::status::HealthState>> {
	machine_health_rollup(conn, machines, true).await
}

/// The machine rollup, with a say over whether the box's own reachability
/// counts.
///
/// It counts when the box is being graded, and does not when an application on
/// it is: each grain has its own reachability, and a box that goes quiet has
/// already made every application on it unreachable on its own account. Adding
/// the box's to theirs would say the same thing twice from the one silence
/// control. See CHK, "A machine's checks present on its applications".
// spec: CHK#a-machines-checks-present-on-its-applications
async fn machine_health_rollup(
	conn: &mut AsyncPgConnection,
	machines: &[(Uuid, Option<Uuid>)],
	include_reachability: bool,
) -> Result<std::collections::HashMap<Uuid, commons_types::status::HealthState>> {
	use crate::schema::{issues, scoped_check_policies};
	use commons_types::status::HealthState;
	use std::collections::{HashMap, HashSet};

	if machines.is_empty() {
		return Ok(HashMap::new());
	}
	let machine_ids: Vec<Uuid> = machines.iter().map(|(id, _)| *id).collect();
	let group_of: HashMap<Uuid, Option<Uuid>> = machines.iter().copied().collect();

	let rows: Vec<HealthCheckRow> = issues::table
		.select((
			issues::machine_id,
			issues::source,
			issues::check_name,
			issues::effective_result,
		))
		.filter(issues::machine_id.eq_any(&machine_ids))
		.filter(issues::check_name.is_not_null())
		.filter(issues::effective_result.is_not_null())
		.filter(issues::active.eq(true))
		.filter(issues::resolved_at.is_null())
		.load(conn)
		.await?;

	let cataloged = crate::check_policies::CheckPolicy::live_cataloged_pairs(conn).await?;

	let group_ids: Vec<Uuid> = group_of.values().filter_map(|g| *g).collect();
	let silence_rows: Vec<(Option<Uuid>, Option<Uuid>, String, String)> =
		scoped_check_policies::table
			.select((
				scoped_check_policies::machine_id,
				scoped_check_policies::server_group_id,
				scoped_check_policies::source,
				scoped_check_policies::check_name,
			))
			.filter(scoped_check_policies::ceiling.eq("skipped"))
			.filter(
				scoped_check_policies::machine_id
					.eq_any(&machine_ids)
					.or(scoped_check_policies::server_group_id.eq_any(&group_ids)),
			)
			.load(conn)
			.await?;
	let mut machine_silences: HashSet<(Uuid, String, String)> = HashSet::new();
	let mut group_silences: HashSet<(Uuid, String, String)> = HashSet::new();
	for (machine_id, group_id, source, check) in silence_rows {
		if let Some(mid) = machine_id {
			machine_silences.insert((mid, source, check));
		} else if let Some(gid) = group_id {
			group_silences.insert((gid, source, check));
		}
	}

	let mut contributing: HashMap<Uuid, Vec<CheckResult>> = HashMap::new();
	for (machine_id, source, check_name, effective) in rows {
		let Some(machine_id) = machine_id else {
			continue;
		};
		let Some(check_name) = check_name else {
			continue;
		};
		let key = (machine_id, source, check_name);
		if !include_reachability && is_reachability(&key.1, &key.2) {
			continue;
		}
		// A machine's checks are in the machine namespace or a curated source's
		// flat one; no application type bears on either.
		if !crate::check_policies::CheckPolicy::live_for(&cataloged, &key.1, &key.2, None) {
			continue;
		}
		if machine_silences.contains(&key) {
			continue;
		}
		if let Some(Some(gid)) = group_of.get(&machine_id)
			&& group_silences.contains(&(*gid, key.1.clone(), key.2.clone()))
		{
			continue;
		}
		let Some(result) = effective
			.as_deref()
			.and_then(|e| e.parse::<CheckResult>().ok())
		else {
			continue;
		};
		contributing.entry(machine_id).or_default().push(result);
	}

	Ok(contributing
		.into_iter()
		.map(|(machine_id, results)| (machine_id, HealthState::from_results(results)))
		.collect())
}

/// One fleet check-detail row: `(application_id, source, check_name,
/// observed_result, effective_result, detail)`.
type FleetCheckRow = (
	Option<Uuid>,
	String,
	Option<String>,
	Option<String>,
	Option<String>,
	Option<serde_json::Value>,
);

/// Every named check's current state across the given applications, as one JSON
/// object per server keyed by check name.
///
/// Each check's value is the detail its source attached, with `result` and
/// `observed` set from the graded state — so a field a check reports is
/// addressable as `check.field`, and the check's own grade as
/// `check.result`. Silenced and decommissioned checks are treated exactly
/// as the server's own check list treats them: silenced reads `skipped`,
/// decommissioned and orphaned states don't present at all.
///
/// A check's identity is `(source, check)`, but the fleet addresses it by
/// name alone: two sources reporting the same check name merge, the more
/// recently updated state winning field by field, mirroring how a server's
/// figures resolve across sources.
// spec: FIG#fleet-spread
pub async fn check_detail_by_server(
	conn: &mut AsyncPgConnection,
	applications: &[(Uuid, Option<Uuid>)],
) -> Result<std::collections::HashMap<Uuid, serde_json::Map<String, serde_json::Value>>> {
	check_detail_at_grain(conn, applications, CheckSubject::Application).await
}

/// The same, for machines: every named check filed against the box itself.
///
/// The fleet counts a box once however many workloads it carries, so a
/// machine-subject check's fields spread over machines and are read from
/// here rather than from the applications on it.
// spec: FIG#fleet-spread
pub async fn check_detail_by_machine(
	conn: &mut AsyncPgConnection,
	machines: &[(Uuid, Option<Uuid>)],
) -> Result<std::collections::HashMap<Uuid, serde_json::Map<String, serde_json::Value>>> {
	check_detail_at_grain(conn, machines, CheckSubject::Machine).await
}

/// Both of the above. `targets` is `(target id, its group)`; `grain` decides
/// which column the filings are keyed by, and so which namespace a check's
/// identity resolves in — only an application has a type.
async fn check_detail_at_grain(
	conn: &mut AsyncPgConnection,
	targets: &[(Uuid, Option<Uuid>)],
	grain: CheckSubject,
) -> Result<std::collections::HashMap<Uuid, serde_json::Map<String, serde_json::Value>>> {
	use crate::schema::{issues, scoped_check_policies};
	use std::collections::{HashMap, HashSet};

	if targets.is_empty() {
		return Ok(HashMap::new());
	}
	let target_ids: Vec<Uuid> = targets.iter().map(|(id, _)| *id).collect();
	let group_of: HashMap<Uuid, Option<Uuid>> = targets.iter().copied().collect();

	// Oldest first, so a later source's fields overwrite an earlier one's
	// when both report the same check name.
	let rows: Vec<FleetCheckRow> = match grain {
		CheckSubject::Application => {
			issues::table
				.select((
					issues::application_id,
					issues::source,
					issues::check_name,
					issues::observed_result,
					issues::effective_result,
					issues::detail,
				))
				.filter(issues::application_id.eq_any(&target_ids))
				.filter(issues::check_name.is_not_null())
				.filter(issues::effective_result.is_not_null())
				.order(issues::updated_at.asc())
				.load(conn)
				.await?
		}
		CheckSubject::Machine => {
			issues::table
				.select((
					issues::machine_id,
					issues::source,
					issues::check_name,
					issues::observed_result,
					issues::effective_result,
					issues::detail,
				))
				.filter(issues::machine_id.eq_any(&target_ids))
				.filter(issues::check_name.is_not_null())
				.filter(issues::effective_result.is_not_null())
				.order(issues::updated_at.asc())
				.load(conn)
				.await?
		}
	};

	let cataloged = crate::check_policies::CheckPolicy::live_cataloged_pairs(conn).await?;
	// Only an application has a type; a machine's checks live in the machine
	// namespace or a curated source's flat one.
	let types = match grain {
		CheckSubject::Application => Application::types_by_id(conn, &target_ids).await?,
		CheckSubject::Machine => HashMap::new(),
	};

	let group_ids: Vec<Uuid> = group_of.values().filter_map(|g| *g).collect();
	let silence_rows: Vec<(
		Option<Uuid>,
		Option<Uuid>,
		Option<String>,
		Option<String>,
		String,
		String,
	)> = match grain {
		CheckSubject::Application => {
			scoped_check_policies::table
				.select((
					scoped_check_policies::application_id,
					scoped_check_policies::server_group_id,
					scoped_check_policies::subject,
					scoped_check_policies::application_type,
					scoped_check_policies::source,
					scoped_check_policies::check_name,
				))
				.filter(scoped_check_policies::ceiling.eq("skipped"))
				.filter(
					scoped_check_policies::application_id
						.eq_any(&target_ids)
						.or(scoped_check_policies::server_group_id.eq_any(&group_ids)),
				)
				.load(conn)
				.await?
		}
		CheckSubject::Machine => {
			scoped_check_policies::table
				.select((
					scoped_check_policies::machine_id,
					scoped_check_policies::server_group_id,
					scoped_check_policies::subject,
					scoped_check_policies::application_type,
					scoped_check_policies::source,
					scoped_check_policies::check_name,
				))
				.filter(scoped_check_policies::ceiling.eq("skipped"))
				.filter(
					scoped_check_policies::machine_id
						.eq_any(&target_ids)
						.or(scoped_check_policies::server_group_id.eq_any(&group_ids)),
				)
				.load(conn)
				.await?
		}
	};
	// The namespace is part of the key: a group spans several application
	// types, so a silence on one type's `version` leaves another type's alone.
	let mut target_silences: HashSet<(Uuid, Namespace, String, String)> = HashSet::new();
	let mut group_silences: HashSet<(Uuid, Namespace, String, String)> = HashSet::new();
	for (target_id, group_id, subject, ty, source, check) in silence_rows {
		let Ok(ns) = Namespace::from_columns(subject.as_deref(), ty.as_deref()) else {
			continue;
		};
		if let Some(tid) = target_id {
			target_silences.insert((tid, ns, source, check));
		} else if let Some(gid) = group_id {
			group_silences.insert((gid, ns, source, check));
		}
	}

	let mut by_target: HashMap<Uuid, serde_json::Map<String, serde_json::Value>> = HashMap::new();
	for (target_id, source, check_name, observed, effective, detail) in rows {
		let (Some(target_id), Some(check)) = (target_id, check_name) else {
			continue;
		};
		if !crate::check_policies::CheckPolicy::live_for(
			&cataloged,
			&source,
			&check,
			types.get(&target_id),
		) {
			continue;
		}
		let Some(stored) = effective
			.as_deref()
			.and_then(|e| e.parse::<CheckResult>().ok())
		else {
			continue;
		};
		let silenced = match Namespace::of(&source, &check, types.get(&target_id)) {
			Some(ns) => {
				target_silences.contains(&(target_id, ns.clone(), source.clone(), check.clone()))
					|| matches!(group_of.get(&target_id), Some(Some(gid))
					if group_silences.contains(&(*gid, ns.clone(), source.clone(), check.clone())))
			}
			None => false,
		};
		let effective = if silenced {
			CheckResult::Skipped
		} else {
			stored
		};

		let entry = by_target
			.entry(target_id)
			.or_default()
			.entry(check)
			.or_insert_with(|| serde_json::Value::Object(Default::default()));
		let Some(fields) = entry.as_object_mut() else {
			continue;
		};
		if let Some(serde_json::Value::Object(reported)) = detail {
			fields.extend(reported);
		}
		// The graded state is authoritative over anything of the same name
		// in the reported detail: `result` there is what the source said,
		// which is `observed` here once policy has had its say.
		fields.insert("result".into(), effective.to_string().into());
		if let Some(observed) = observed {
			fields.insert("observed".into(), observed.into());
		}
	}

	Ok(by_target)
}

/// One consolidated check row:
/// `(source, check_name, observed_result, effective_result, detail)`.
type ConsolidatedRow = (
	String,
	Option<String>,
	Option<String>,
	Option<String>,
	Option<serde_json::Value>,
);

/// A server's current checks across every source, graded, for presentation.
/// The health rollup uses [`health_from_check_state`] (so it matches the
/// headline exactly); the check list is every non-decommissioned reported
/// `(source, check)` with its observed/effective results, detail, and
/// whether it's silenced — most urgent first. This is the live side of the
/// consolidated checks view; the point-in-time side reconstructs the same
/// shape from status history.
/// One application's live consolidated checks.
pub async fn consolidated_checks_latest(
	conn: &mut AsyncPgConnection,
	application_id: Uuid,
	group_id: Option<Uuid>,
) -> Result<commons_types::status::ConsolidatedChecks> {
	consolidated_checks_for(conn, Scope::Application(application_id), group_id).await
}

/// One machine's live consolidated checks — the box's own, not those of the
/// applications on it.
///
/// A machine hosting two workloads has one set of these, which is the whole
/// point of the grain: a full disk is a fact about the box, and reading it
/// twice would say it twice.
// spec: CHK#targets
pub async fn consolidated_checks_latest_for_machine(
	conn: &mut AsyncPgConnection,
	machine_id: Uuid,
	group_id: Option<Uuid>,
) -> Result<commons_types::status::ConsolidatedChecks> {
	consolidated_checks_for(conn, Scope::Machine(machine_id), group_id).await
}

/// The live consolidated checks filed against one target, at whichever grain
/// it is.
///
/// The two grains differ only in which column identifies the target and which
/// rollup answers for it; everything after that — the catalog gate, the
/// silence pass, the reachability fill-in, the ordering — is the same work.
/// Keeping it one function is what stops the pair drifting apart.
async fn consolidated_checks_for(
	conn: &mut AsyncPgConnection,
	target: Scope,
	group_id: Option<Uuid>,
) -> Result<commons_types::status::ConsolidatedChecks> {
	use commons_types::status::ConsolidatedChecks;

	let target_id = match target {
		Scope::Application(id) | Scope::Machine(id) => id,
		// A group or canopy-wide rollup is a different question, answered by
		// its own reader; nothing calls this with one.
		Scope::Group(_) | Scope::Global => {
			return Err(AppError::BadRequest(
				"consolidated checks are read for an application or a machine".into(),
			));
		}
	};

	let health_state = match target {
		Scope::Machine(id) => machine_health_from_check_state(conn, &[(id, group_id)])
			.await?
			.get(&id)
			.copied()
			.unwrap_or_default(),
		_ => health_from_check_state(conn, &[(target_id, group_id)])
			.await?
			.get(&target_id)
			.copied()
			.unwrap_or_default(),
	};

	// A check-state only presents if a live catalog policy backs it: this
	// excludes decommissioned checks and orphaned check-states (no catalog
	// row at all — invisible in settings and unmanageable, so never a
	// phantom failure here). See `live_cataloged_pairs`. Read once and shared
	// across both grains, the catalog being fleet-wide.
	let cataloged = crate::check_policies::CheckPolicy::live_cataloged_pairs(conn).await?;

	let mut checks = checks_at_scope(conn, target, group_id, &cataloged, true).await?;

	// An operator triaging an application sees every check bearing on it, its
	// own and its host's, in one list. The box's checks are read from where
	// they were filed rather than copied per workload, so two applications on
	// one box present one filing twice rather than two filings.
	//
	// Reachability is left behind: each grain determines its own, and the box
	// going quiet has already made every application on it unreachable in its
	// own right.
	// spec: CHK#a-machines-checks-present-on-its-applications
	if let Scope::Application(id) = target {
		let machine_id = Application::get_by_id(conn, id).await?.machine_id;
		let machine = crate::machines::Machine::get_by_id(conn, machine_id).await?;
		checks.extend(
			checks_at_scope(
				conn,
				Scope::Machine(machine_id),
				machine.group_id,
				&cataloged,
				false,
			)
			.await?,
		);
	}

	checks.sort_by(|a, b| {
		a.effective
			.urgency_rank()
			.cmp(&b.effective.urgency_rank())
			.then_with(|| a.source.cmp(&b.source))
			.then_with(|| a.check.cmp(&b.check))
	});

	Ok(ConsolidatedChecks {
		health_state,
		checks,
	})
}

/// The checks filed against one target, graded and ready to present, without
/// the rollup: what a target contributes to a consolidated view, whether it is
/// the target being read or the machine under one.
///
/// `include_reachability` fills in a passing reachability where the target has
/// no state row for it. That belongs to the grain being read and not to a
/// machine seen from an application on it.
async fn checks_at_scope(
	conn: &mut AsyncPgConnection,
	target: Scope,
	group_id: Option<Uuid>,
	cataloged: &std::collections::HashSet<(String, Namespace, String)>,
	include_reachability: bool,
) -> Result<Vec<commons_types::status::ConsolidatedCheck>> {
	use crate::schema::{issues, scoped_check_policies};
	use commons_types::status::ConsolidatedCheck;
	use commons_types::subject::CheckSubject;
	use std::collections::HashSet;

	let (target_application, target_machine, _) = target.to_columns();
	let subject = match target {
		Scope::Machine(_) => CheckSubject::Machine,
		_ => CheckSubject::Application,
	};

	let rows: Vec<ConsolidatedRow> = issues::table
		.select((
			issues::source,
			issues::check_name,
			issues::observed_result,
			issues::effective_result,
			issues::detail,
		))
		.filter(issues::application_id.is_not_distinct_from(target_application))
		.filter(issues::machine_id.is_not_distinct_from(target_machine))
		.filter(issues::check_name.is_not_null())
		.filter(issues::effective_result.is_not_null())
		.load(conn)
		.await?;

	// Only an application has a type; a machine's checks are in the machine
	// namespace or a curated source's flat one.
	let application_type = match target_application {
		Some(id) => Some(Application::get_by_id(conn, id).await?.r#type),
		None => None,
	};

	let group_ids: Vec<Uuid> = group_id.into_iter().collect();
	let silence_rows: Vec<(Option<String>, Option<String>, String, String)> =
		scoped_check_policies::table
			.select((
				scoped_check_policies::subject,
				scoped_check_policies::application_type,
				scoped_check_policies::source,
				scoped_check_policies::check_name,
			))
			.filter(scoped_check_policies::ceiling.eq("skipped"))
			.filter(
				scoped_check_policies::application_id
					.is_not_distinct_from(target_application)
					.and(scoped_check_policies::machine_id.is_not_distinct_from(target_machine))
					.or(scoped_check_policies::server_group_id.eq_any(&group_ids)),
			)
			.load(conn)
			.await?;
	// This is one target, so a matching check at either its own scope or its
	// group's means silenced here. The namespace has to match too: a group
	// spans several application types, so a silence on one type's `version` is
	// not a silence on another type's.
	let silenced: HashSet<(Namespace, String, String)> = silence_rows
		.into_iter()
		.filter_map(|(subject, ty, source, check)| {
			let ns = Namespace::from_columns(subject.as_deref(), ty.as_deref()).ok()?;
			Some((ns, source, check))
		})
		.collect();

	let mut checks: Vec<ConsolidatedCheck> = rows
		.into_iter()
		.filter_map(|(source, check_name, observed, effective, detail)| {
			let check = check_name?;
			if !include_reachability && is_reachability(&source, &check) {
				return None;
			}
			let namespace = Namespace::of(&source, &check, application_type.as_ref())?;
			if !crate::check_policies::CheckPolicy::live_in(cataloged, &source, &namespace, &check)
			{
				return None;
			}
			let stored: CheckResult = effective.as_deref().and_then(|e| e.parse().ok())?;
			let is_silenced =
				silenced.contains(&(namespace.clone(), source.clone(), check.clone()));
			// A silence is a scoped ceiling of `skipped`: cap the effective
			// result here so the live view matches both the health rollup
			// (which excludes silenced checks) and the snapshot path (which
			// re-grades through `apply_scoped`). The stored effective may
			// still read failed/warning if the silence post-dates the last
			// push — the observed result keeps what was reported.
			let effective = if is_silenced {
				CheckResult::Skipped
			} else {
				stored
			};
			Some(ConsolidatedCheck {
				silenced: is_silenced,
				observed: observed.as_deref().and_then(|o| o.parse().ok()),
				qualified_name: namespace.qualified_name(&check),
				namespace: (&namespace).into(),
				source,
				check,
				effective,
				detail: detail.unwrap_or_else(|| serde_json::json!({})),
				subject,
			})
		})
		.collect();
	// Reachability presents for every target, whether or not a reporter has
	// ever gone quiet. The sweep only files this check while it's degraded
	// (or when it has a degradation to close), so a server that has never
	// had a problem carries no state row for it — fill that in as passing,
	// so the check and its silence control are there before anything is
	// red. Gated on a live catalog row so decommissioning still removes it,
	// and graded through the same silence pass as everything else.
	// spec: CHK#reachability
	let reachability = (
		crate::statuses::CANOPY_SOURCE.to_string(),
		crate::statuses::REACHABILITY_REF.to_string(),
	);
	if include_reachability
		&& crate::check_policies::CheckPolicy::live_for(
			cataloged,
			&reachability.0,
			&reachability.1,
			application_type.as_ref(),
		) && !checks
		.iter()
		.any(|c| c.source == reachability.0 && c.check == reachability.1)
	{
		// Reachability is canopy's own, so it is flat whatever the target.
		let is_silenced = silenced.contains(&(
			Namespace::Flat,
			reachability.0.clone(),
			reachability.1.clone(),
		));
		checks.push(ConsolidatedCheck {
			silenced: is_silenced,
			observed: Some(CheckResult::Passed),
			namespace: (&Namespace::Flat).into(),
			qualified_name: reachability.1.clone(),
			source: reachability.0,
			check: reachability.1,
			effective: if is_silenced {
				CheckResult::Skipped
			} else {
				CheckResult::Passed
			},
			detail: serde_json::json!({}),
			subject,
		});
	}

	Ok(checks)
}

impl Issue {
	/// Check state for one (source, check), across every scope — server,
	/// group, and canopy-wide. The check detail page's data source: rows
	/// carry the observed/effective results, the check's detail, and the
	/// degraded-streak timestamps. A check's identity is the pair — a
	/// same-named check from another source is a different check.
	/// Every check-state row for one catalog entry.
	///
	/// `namespace` is what makes this one entry rather than a name: an
	/// application-namespaced check is narrowed to applications of its type,
	/// and a machine-namespaced one to machines, so two types reporting one
	/// name do not pool their states onto each other's page. A curated
	/// source's flat entry is one check filed at whatever grain suits it, so
	/// it takes every row.
	pub async fn check_state_for_check(
		conn: &mut AsyncPgConnection,
		source: &str,
		namespace: &Namespace,
		check_name: &str,
	) -> Result<Vec<Issue>> {
		use crate::schema::issues::dsl;

		let mut query = dsl::issues
			.select(Issue::as_select())
			.filter(dsl::source.eq(source))
			.filter(dsl::check_name.eq(check_name))
			.filter(dsl::observed_result.is_not_null())
			.into_boxed();

		match namespace {
			Namespace::Flat => {}
			Namespace::Machine => query = query.filter(dsl::machine_id.is_not_null()),
			Namespace::Application(ty) => {
				query = query.filter(
					dsl::application_id.nullable().eq_any(
						crate::schema::applications::dsl::applications
							.select(crate::schema::applications::dsl::id)
							.filter(crate::schema::applications::dsl::type_.eq(ty.to_string()))
							.nullable(),
					),
				)
			}
		}

		query.load(conn).await.map_err(AppError::from)
	}
}

/// The canopy-wide issue at `(canopy, ref)`, if it has ever been raised.
pub async fn get_global_issue(conn: &mut AsyncPgConnection, r#ref: &str) -> Result<Option<Issue>> {
	use crate::schema::issues::dsl;

	dsl::issues
		.select(Issue::as_select())
		.filter(
			dsl::application_id
				.is_null()
				.and(dsl::server_group_id.is_null())
				.and(dsl::source.eq(crate::statuses::CANOPY_SOURCE))
				.and(dsl::ref_.eq(r#ref)),
		)
		.first(conn)
		.await
		.optional()
		.map_err(AppError::from)
}

/// Compute whether the issue *should* currently be contributing to an
/// open incident, and apply join/leave accordingly. The rules:
///
/// - **Leave**: `!active || resolved || snoozed || silenced || maintained ||
///   !monitored`.
///   A result downgrade alone does *not* remove an issue — once
///   contributing, it stays until it's actually gone or explicitly
///   suppressed. Flipping the server to unmonitored *does* remove it: the
///   operator has said they're not watching this server, so its issues
///   stop counting. The same applies if the check is silenced at server
///   or group scope — see [`crate::silenced_refs`] — or if a maintenance
///   window suspends the target, see [`crate::maintenance_windows`].
///   An issue holding a live membership on another target's incident also
///   leaves that one, whatever else it does: a rank or group change moves
///   which target it answers to, and it cannot be in two.
/// - **Join**: not leaving, AND one of:
///   - the state opens incidents on its own — an effective failure (see
///     [`Issue::opens_incident`]); or
///   - the target already has an open incident — then any active issue,
///     warnings included, joins it. The threshold only governs incident
///     *creation*; once an incident is in progress everything else piles
///     in for context.
/// - **Close**: when the last effective-failure contributor leaves, the
///   incident starts **lingering** (`closing_at` stamped) rather than
///   closing: a failure returning within the target's linger window clears
///   the stamp and the same incident continues, so a red check that blips
///   green doesn't turn one span of trouble into a resolve/re-open pair.
///   The linger sweep ([`sweep_lingering_incidents`]) closes it once the
///   window elapses, backdating the close to when the failure left. Only a
///   leave caused by the check actually recovering lingers: resolution,
///   snooze, silence, maintenance, and monitoring-off are explicit operator
///   actions, not flaps, and close immediately — as does a zero linger window.
///   Lesser contributors that joined because
///   the target had an open incident stay attached (so the audit trail and
///   Slack thread retain them) but **do not** hold the incident open (or
///   lingering) by themselves. Without this asymmetry, a check that's
///   stuck firing at warning could keep an incident open indefinitely
///   after the failure that opened it has long since gone away.
///
/// `monitored` reflects the server's `is_monitored()` at call time. When
/// `false`, the issue is treated as a "leave": this is what makes
/// `unmonitored` an opt-out of the incident workflow without losing the
/// issue rows themselves.
///
/// `by` is the operator login when this re-evaluation was triggered by a
/// human action (e.g. resolving an issue or incident). It's threaded
/// through so a cascade close caused by that action can attribute the
/// resulting Slack `incident_resolve` row to the operator rather than
/// "the healthcheck recovering". `None` for device-driven flows (event
/// push with `active:false`).
async fn re_evaluate_incident_membership(
	conn: &mut AsyncPgConnection,
	issue: &Issue,
	target: IncidentTarget,
	monitored: bool,
	transition_time: Timestamp,
	by: Option<&str>,
) -> Result<()> {
	use crate::schema::{incident_issues, incidents};

	let held = open_incident_holding(conn, issue.id).await?;
	let snoozed = issue.snoozed_until.is_some_and(|t| t > Timestamp::now());
	// An event is silenced at its own scope or at its group's, and its own
	// scope is whichever grain it was filed against. Reading the application
	// id alone left a machine's checks silenceable in the consolidated view and
	// in what the agent is told, but still able to open an incident.
	// spec: CHK#silences-follow-the-event
	let silenced = crate::silenced_refs::is_silenced(
		conn,
		Scope::from_columns(
			issue.application_id,
			issue.machine_id,
			issue.server_group_id,
		),
		target.group_id(),
		&issue.source,
		&issue.r#ref,
	)
	.await?;
	// A maintenance window over the target suspends everything on it, so its
	// issues leave exactly as a silenced one does. A window is over a machine,
	// and an application's issues are covered by the window over the box it
	// runs on, so an application-scoped issue resolves through to its machine.
	let cover_machine = match (issue.machine_id, issue.application_id) {
		(Some(mid), _) => Some(mid),
		(None, Some(aid)) => Some(Application::get_by_id(conn, aid).await?.machine_id),
		(None, None) => None,
	};
	let maintained = crate::maintenance_windows::MaintenanceWindow::suspends(
		conn,
		cover_machine,
		target.group_id(),
	)
	.await?;
	// Lock the target before reading whether it has an open incident: for a
	// sub-Error issue, that read is the entire justification for joining, and
	// a concurrent close landing between here and `find_or_open_incident`
	// would turn "join the open incident" into "open a new one" — a Slack page
	// for a Warning.
	lock_target(conn, target).await?;
	// An issue whose target changed (a rank set, or a move between groups) is
	// still a live member of the incident on the target it has left, and comes
	// out of that one before it can join the one it now belongs to.
	// spec: INC#membership
	let mut was_in = held.is_some();
	if let Some(held) = held.as_ref().map(IncidentTarget::of_incident)
		&& held != target
	{
		lock_target(conn, held).await?;
		leave_open_incident(conn, issue, transition_time, by, false).await?;
		was_in = false;
	}
	let target_open = target_has_open_incident(conn, target).await?;

	let should_leave = !issue.active
		|| issue.resolved_at.is_some()
		|| snoozed
		|| silenced
		|| maintained
		|| !monitored;
	let should_join = monitored
		&& !silenced
		&& !maintained
		&& issue.active
		&& issue.resolved_at.is_none()
		&& !snoozed
		&& (issue.opens_incident() || target_open);

	match (was_in, should_join, should_leave) {
		(false, true, _) => {
			let (incident_id, newly_opened) =
				find_or_open_incident(conn, target, transition_time).await?;
			diesel::insert_into(incident_issues::table)
				.values((
					incident_issues::incident_id.eq(incident_id),
					incident_issues::issue_id.eq(issue.id),
					incident_issues::joined_at.eq(jiff_diesel::Timestamp::from(transition_time)),
				))
				.execute(conn)
				.await?;
			// An effective failure joining a lingering incident ends the
			// lingering: the trouble is back, the same incident continues.
			if !newly_opened && issue.opens_incident() {
				diesel::update(
					incidents::table
						.filter(incidents::id.eq(incident_id))
						.filter(incidents::closing_at.is_not_null()),
				)
				.set(incidents::closing_at.eq(None::<jiff_diesel::Timestamp>))
				.execute(conn)
				.await?;
			}
			if newly_opened {
				enqueue_slack_open(conn, incident_id, target, issue).await?;
			} else if issue.escalates_now() {
				// Two sub-cases when an escalating failure joins an existing incident:
				//  - The original open is still pending in the outbox →
				//    accelerate so the "incident opened" message lands
				//    immediately. No second message: the open hasn't been
				//    seen yet, so a fresh open would be redundant noise.
				//  - The original open has already shipped → enqueue a
				//    fresh open at Critical severity as the escalation
				//    signal. Gated on incidents.escalated_at IS NULL so
				//    repeated Critical joins (or a Critical leaving and
				//    rejoining) don't re-fire the message.
				let accelerated = accelerate_pending_open(conn, incident_id).await?;
				if !accelerated {
					let escalated: Option<Incident> = diesel::update(
						incidents::table
							.filter(incidents::id.eq(incident_id))
							.filter(incidents::escalated_at.is_null()),
					)
					.set(incidents::escalated_at.eq(jiff_diesel::Timestamp::from(transition_time)))
					.returning(Incident::as_select())
					.get_result(conn)
					.await
					.optional()?;
					if escalated.is_some() {
						enqueue_slack_open(conn, incident_id, target, issue).await?;
					}
				}
			}
		}
		(true, _, true) => {
			// Linger damps *reporter* flapping: only a leave caused by the
			// check actually recovering (inactive, with no operator
			// suppression in play) waits out the window. Resolution, snooze,
			// silence, and monitoring-off are explicit operator actions — not
			// flaps — and close immediately, Slack resolve attributed where
			// `by` is known.
			let check_recovery =
				!issue.active && issue.resolved_at.is_none() && !snoozed && !silenced && monitored;
			leave_open_incident(conn, issue, transition_time, by, check_recovery).await?;
		}
		_ => {
			// A member issue re-filing as an effective failure while its
			// incident lingers ends the lingering — the trouble is back. A
			// leave-and-rejoin lands in the join arm above; this catches the
			// member that never left, e.g. a warning contributor re-graded
			// to failed.
			if was_in && should_join && !should_leave && issue.opens_incident() {
				let member_of: Vec<Uuid> = incident_issues::table
					.select(incident_issues::incident_id)
					.filter(incident_issues::issue_id.eq(issue.id))
					.filter(incident_issues::left_at.is_null())
					.load(conn)
					.await?;
				diesel::update(
					incidents::table
						.filter(incidents::id.eq_any(member_of))
						.filter(incidents::closed_at.is_null())
						.filter(incidents::closing_at.is_not_null()),
				)
				.set(incidents::closing_at.eq(None::<jiff_diesel::Timestamp>))
				.execute(conn)
				.await?;
			}
		}
	}
	Ok(())
}

/// Stamp this issue out of whatever open incident holds it, and close or
/// linger that incident once its last effective failure has left.
///
/// `check_recovery` is true only where the leave is the check itself
/// recovering with no operator suppression in play, which is the one case
/// lingering damps. Resolution, snooze, silence, monitoring off, and the
/// target moving are operator actions, so they close immediately.
async fn leave_open_incident(
	conn: &mut AsyncPgConnection,
	issue: &Issue,
	transition_time: Timestamp,
	by: Option<&str>,
	check_recovery: bool,
) -> Result<()> {
	use crate::schema::{incident_issues, incidents, issues};

	// Must match `open_incident_holding`'s definition of live
	// membership, incident `closed_at` included: an issue can hold an
	// unstamped row on a *closed* incident (a close doesn't stamp the
	// members that never left) at the same time as a live row on an
	// open one. Filtering on `left_at` alone lets this pick the
	// stranded row, and the leave then stamps the wrong membership and
	// weighs `remaining_open` against a long-closed incident —
	// abandoning the open one with no live members and no Slack
	// resolve.
	let open_link: IncidentIssue = incident_issues::table
		.inner_join(incidents::table.on(incidents::id.eq(incident_issues::incident_id)))
		.select(IncidentIssue::as_select())
		.filter(
			incident_issues::issue_id
				.eq(issue.id)
				.and(incident_issues::left_at.is_null())
				.and(incidents::closed_at.is_null()),
		)
		.for_update()
		.first(conn)
		.await?;

	// Serialize against concurrent leaves of *other* issues on
	// the same incident. Without this incident-row lock, two
	// transactions each removing one of the last two live
	// issues can each observe remaining_open >= 1 (each sees
	// its own in-flight left_at update but not the other's)
	// and skip the close, leaving the incident in "no live
	// issues but closed_at IS NULL" with no Slack fired.
	let incident: Incident = incidents::table
		.select(Incident::as_select())
		.filter(incidents::id.eq(open_link.incident_id))
		.for_update()
		.first(conn)
		.await?;

	diesel::update(
		incident_issues::table.filter(
			incident_issues::incident_id
				.eq(open_link.incident_id)
				.and(incident_issues::issue_id.eq(open_link.issue_id))
				.and(
					incident_issues::joined_at
						.eq(jiff_diesel::Timestamp::from(open_link.joined_at)),
				),
		),
	)
	.set(incident_issues::left_at.eq(jiff_diesel::Timestamp::from(transition_time)))
	.execute(conn)
	.await?;

	// Only count contributors that *currently* open an incident
	// (effective failures). Lesser contributors stay attached for
	// context but don't hold the incident open on their own.
	let remaining_open: i64 = incident_issues::table
		.inner_join(issues::table.on(issues::id.eq(incident_issues::issue_id)))
		.filter(
			incident_issues::incident_id
				.eq(open_link.incident_id)
				.and(incident_issues::left_at.is_null())
				.and(issues::effective_result.eq("failed")),
		)
		.count()
		.get_result(conn)
		.await?;
	if remaining_open == 0 {
		// A zero linger window is the operator opting out of lingering.
		let window = linger_window(conn, IncidentTarget::of_incident(&incident)).await?;
		if by.is_some() || !check_recovery || window.is_zero() {
			// Filter on `closed_at IS NULL` so that when a stranded
			// lesser contributor eventually leaves an already-closed
			// incident (because the failure-filter close above already
			// retired it), we skip both the no-op update and the
			// double Slack resolve.
			let closed: Option<Incident> = diesel::update(
				incidents::table
					.filter(incidents::id.eq(open_link.incident_id))
					.filter(incidents::closed_at.is_null()),
			)
			.set(incidents::closed_at.eq(jiff_diesel::Timestamp::from(transition_time)))
			.returning(Incident::as_select())
			.get_result(conn)
			.await
			.optional()?;
			if let Some(closed) = closed {
				release_remaining_members(conn, closed.id, transition_time).await?;
				enqueue_slack_resolve_inner(conn, &closed, by).await?;
			}
		} else {
			// Start lingering: record when the last effective failure
			// left. Stamped once — a lesser contributor leaving an
			// already-lingering incident doesn't move the mark — and
			// the linger sweep closes the incident when the stamp
			// outlives the window (see `sweep_lingering_incidents`).
			diesel::update(
				incidents::table
					.filter(incidents::id.eq(open_link.incident_id))
					.filter(incidents::closed_at.is_null())
					.filter(incidents::closing_at.is_null()),
			)
			.set(incidents::closing_at.eq(jiff_diesel::Timestamp::from(transition_time)))
			.execute(conn)
			.await?;
		}
	}
	Ok(())
}

/// Resolve the `(target, monitored)` pair an issue should be re-evaluated
/// against, handling all three scopes.
///
/// - Application-scoped (`application_id = Some`): look the server up; its `group_id`
///   and `is_monitored` drive the evaluation. `None` group → ungrouped, no
///   incident path.
/// - Group-scoped (`server_group_id = Some`): use the group directly and force
///   `monitored = true` so the per-server gate never silences a control-plane
///   issue.
/// - Canopy-wide (neither): the global target, always monitored.
async fn issue_target_and_monitored(
	conn: &mut AsyncPgConnection,
	issue: &Issue,
) -> Result<Option<(IncidentTarget, bool)>> {
	Scope::from_columns(
		issue.application_id,
		issue.machine_id,
		issue.server_group_id,
	)
	.resolve_incident_target(conn)
	.await
}

/// Re-evaluate every currently-open issue on `application_id` against incident
/// membership. Used as a catch-up when the operator flips a property of
/// the server that changes its incident eligibility — currently:
/// ungrouped→grouped (via [`Application::assign_to_group`]) and monitored
/// toggles (via the private-server's server update handler). On flips
/// that should *remove* issues from an incident (e.g. monitored→
/// unmonitored), `re_evaluate_incident_membership` will leave them and
/// close the incident if nothing else props it up.
pub async fn reevaluate_open_issues_for_server(
	db: &mut AsyncPgConnection,
	application_id: Uuid,
) -> Result<()> {
	use crate::schema::issues::dsl;

	let server = Application::get_by_id(db, application_id).await?;
	let Some(gid) = server.group_id else {
		return Ok(());
	};
	let monitored = server.is_monitored;
	let target = IncidentTarget::of_member(gid, server.rank);

	let open_issues: Vec<Issue> = dsl::issues
		.select(Issue::as_select())
		.filter(dsl::application_id.eq(application_id))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.load(db)
		.await?;

	let now = Timestamp::now();
	for issue in open_issues {
		re_evaluate_incident_membership(db, &issue, target, monitored, now, None).await?;
	}
	Ok(())
}

/// Enqueue `application_id` for deferred incident (re-)evaluation.
///
/// The device status-ingest path calls this in place of the inline
/// `re_evaluate_incident_membership` it used to run: recording the issue
/// state is kept synchronous, but the incident work — which takes the
/// per-group `server_groups` lock — is handed to the queue worker so
/// request traffic never contends on that lock.
///
/// Idempotent: the primary key coalesces a burst of pushes into one queued
/// unit. A push arriving while a re-evaluation is already pending is a
/// no-op — the pending run reads current state when it fires, so it already
/// covers the later push.
pub async fn enqueue_incident_reeval(
	conn: &mut AsyncPgConnection,
	application_id: Uuid,
) -> Result<()> {
	use crate::schema::incident_reeval_queue::dsl;
	diesel::insert_into(dsl::incident_reeval_queue)
		.values(dsl::application_id.eq(application_id))
		.on_conflict(dsl::application_id)
		.do_nothing()
		.execute(conn)
		.await?;
	Ok(())
}

/// Re-evaluate incident membership for every issue on `application_id` that can
/// currently affect an incident: issues that are active-and-unresolved
/// (candidates to *join*), plus issues still linked to an open incident
/// (candidates to *leave* — e.g. a check that just recovered and went
/// inactive). [`reevaluate_open_issues_for_server`] deliberately only walks
/// active issues, so it can't drive the leave transitions the ingest path
/// needs; this is the ingest-equivalent set.
///
/// Active issues are evaluated before inactive ones so that when a failure
/// replaces another within a single push, the incoming failure (re)joins
/// the incident before the outgoing one leaves — otherwise the incident
/// would briefly close and reopen. This mirrors the ingest-time filing
/// order.
pub async fn reevaluate_incidents_for_server(
	conn: &mut AsyncPgConnection,
	application_id: Uuid,
) -> Result<()> {
	use crate::schema::{incident_issues, incidents, issues};

	let server = Application::get_by_id(conn, application_id).await?;
	let Some(gid) = server.group_id else {
		return Ok(());
	};
	let monitored = server.is_monitored;
	let target = IncidentTarget::of_member(gid, server.rank);

	let mut candidates: Vec<Issue> = issues::table
		.select(Issue::as_select())
		.filter(issues::application_id.eq(application_id))
		.filter(
			issues::active
				.eq(true)
				.and(issues::resolved_at.is_null())
				.or(issues::id.eq_any(
					incident_issues::table
						.inner_join(
							incidents::table.on(incidents::id.eq(incident_issues::incident_id)),
						)
						.filter(incident_issues::left_at.is_null())
						.filter(incidents::closed_at.is_null())
						.select(incident_issues::issue_id),
				)),
		)
		.load(conn)
		.await?;

	// Active (potential joiners) before inactive (leavers).
	candidates.sort_by_key(|issue| !issue.active);

	let now = Timestamp::now();
	for issue in candidates {
		re_evaluate_incident_membership(conn, &issue, target, monitored, now, None).await?;
	}
	Ok(())
}

/// Drain the incident re-evaluation queue, running
/// [`reevaluate_incidents_for_server`] for up to `limit` queued applications and
/// removing each once handled. Returns the number of applications processed.
///
/// Each server is claimed and processed in its own transaction with
/// `FOR UPDATE SKIP LOCKED`, so a slow re-evaluation doesn't hold a batch of
/// queue rows locked, overlapping ticks don't double-process, and a failure
/// on one server doesn't roll back the others.
pub async fn process_incident_reeval_queue(
	conn: &mut AsyncPgConnection,
	limit: i64,
) -> Result<usize> {
	use crate::schema::incident_reeval_queue::dsl;

	let mut processed = 0usize;
	while (processed as i64) < limit {
		let claimed: Option<Uuid> = conn
			.transaction::<_, AppError, _>(async |tx| {
				let Some(application_id): Option<Uuid> = dsl::incident_reeval_queue
					.select(dsl::application_id)
					.order(dsl::enqueued_at.asc())
					.for_update()
					.skip_locked()
					.first(tx)
					.await
					.optional()?
				else {
					return Ok(None);
				};
				reevaluate_incidents_for_server(tx, application_id).await?;
				diesel::delete(
					dsl::incident_reeval_queue.filter(dsl::application_id.eq(application_id)),
				)
				.execute(tx)
				.await?;
				Ok(Some(application_id))
			})
			.await?;
		match claimed {
			Some(_) => processed += 1,
			None => break,
		}
	}
	Ok(processed)
}

/// Re-evaluate every currently-open issue on `application_id` with the given
/// `(source, ref)`. Narrower variant of [`reevaluate_open_issues_for_server`]
/// used after a server-scoped silence is added or removed: only the matching
/// refs need to revisit their incident membership.
pub async fn reevaluate_open_issues_for_server_ref(
	db: &mut AsyncPgConnection,
	application_id: Uuid,
	source: &str,
	r#ref: &str,
) -> Result<()> {
	use crate::schema::issues::dsl;

	let server = Application::get_by_id(db, application_id).await?;
	let Some(gid) = server.group_id else {
		return Ok(());
	};
	let monitored = server.is_monitored;
	let target = IncidentTarget::of_member(gid, server.rank);

	let open_issues: Vec<Issue> = dsl::issues
		.select(Issue::as_select())
		.filter(dsl::application_id.eq(application_id))
		.filter(dsl::source.eq(source))
		.filter(dsl::ref_.eq(r#ref))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.load(db)
		.await?;

	let now = Timestamp::now();
	for issue in open_issues {
		re_evaluate_incident_membership(db, &issue, target, monitored, now, None).await?;
	}
	Ok(())
}

/// Re-evaluate every currently-open issue filed against this machine with the
/// given `(source, ref)`. Used after a machine-scoped silence is added or
/// removed.
///
/// Only the box's own issues: the applications on it are silenced against
/// themselves, so a silence here does not reach them.
// spec: CHK#silences-follow-the-event
pub async fn reevaluate_open_issues_for_machine_ref(
	db: &mut AsyncPgConnection,
	machine_id: Uuid,
	source: &str,
	r#ref: &str,
) -> Result<()> {
	use crate::schema::issues::dsl;

	let machine = crate::machines::Machine::get_by_id(db, machine_id).await?;
	let Some(gid) = machine.group_id else {
		return Ok(());
	};
	let monitored = machine.is_monitored;
	let rank = crate::machines::Machine::rank(db, machine_id).await?;
	let target = IncidentTarget::of_member(gid, rank);

	let open_issues: Vec<Issue> = dsl::issues
		.select(Issue::as_select())
		.filter(dsl::machine_id.eq(machine_id))
		.filter(dsl::source.eq(source))
		.filter(dsl::ref_.eq(r#ref))
		.filter(dsl::active.eq(true))
		.filter(dsl::resolved_at.is_null())
		.load(db)
		.await?;

	let now = Timestamp::now();
	for issue in open_issues {
		re_evaluate_incident_membership(db, &issue, target, monitored, now, None).await?;
	}
	Ok(())
}

/// Re-evaluate every currently-open issue in the group with the given
/// `(source, ref)`. Used after a group-scoped silence is added or removed.
pub async fn reevaluate_open_issues_for_group_ref(
	db: &mut AsyncPgConnection,
	server_group_id: Uuid,
	source: &str,
	r#ref: &str,
) -> Result<()> {
	use crate::schema::{applications, issues};

	let server_ids: Vec<Uuid> = applications::table
		.select(applications::id)
		.filter(applications::group_id.eq(server_group_id))
		.load(db)
		.await?;

	// Both the group's server-scoped issues and any group-scoped issues
	// (server_group_id = this group) that match the silenced (source, ref).
	let open_issues: Vec<Issue> = issues::table
		.select(Issue::as_select())
		.filter(
			issues::application_id
				.eq_any(&server_ids)
				.or(issues::server_group_id.eq(server_group_id)),
		)
		.filter(issues::source.eq(source))
		.filter(issues::ref_.eq(r#ref))
		.filter(issues::active.eq(true))
		.filter(issues::resolved_at.is_null())
		.load(db)
		.await?;

	let now = Timestamp::now();
	for issue in open_issues {
		let Some((target, monitored)) = issue_target_and_monitored(db, &issue).await? else {
			continue;
		};
		re_evaluate_incident_membership(db, &issue, target, monitored, now, None).await?;
	}
	Ok(())
}

/// Re-evaluate every currently-open issue on a scope against incident
/// membership: one server's, or a whole group's plus the group's own. Used
/// when something about the target changes what counts, rather than
/// something about one check: a maintenance window declared over it (see
/// [`crate::maintenance_windows`]), or a rank change moving which
/// environment its issues belong to.
///
/// `by` attributes an incident that closes as a result, so the notice says
/// what happened instead of reading as the trouble having passed.
pub async fn reevaluate_open_issues_for_scope(
	db: &mut AsyncPgConnection,
	scope: Scope,
	by: Option<&str>,
) -> Result<()> {
	use crate::schema::{applications, issues};

	// The applications whose issues are re-evaluated, and the group they answer
	// to. A machine's window covers the workloads on it, so re-evaluating a
	// machine means re-evaluating every application it hosts, not just its own
	// machine-scoped issues.
	let (gid, application_ids, machine_ids, include_group_scoped) = match scope {
		Scope::Application(aid) => {
			let application = Application::get_by_id(db, aid).await?;
			let Some(gid) = application.group_id else {
				return Ok(());
			};
			(gid, vec![aid], Vec::new(), false)
		}
		Scope::Machine(mid) => {
			let machine = crate::machines::Machine::get_by_id(db, mid).await?;
			let Some(gid) = machine.group_id else {
				return Ok(());
			};
			let ids: Vec<Uuid> = applications::table
				.select(applications::id)
				.filter(applications::machine_id.eq(mid))
				.load(db)
				.await?;
			(gid, ids, vec![mid], false)
		}
		Scope::Group(gid) => {
			let ids: Vec<Uuid> = applications::table
				.select(applications::id)
				.filter(applications::group_id.eq(gid))
				.load(db)
				.await?;
			let machines: Vec<Uuid> = crate::schema::machines::table
				.select(crate::schema::machines::id)
				.filter(crate::schema::machines::group_id.eq(gid))
				.load(db)
				.await?;
			(gid, ids, machines, true)
		}
		Scope::Global => return Ok(()),
	};

	let mut query = issues::table
		.select(Issue::as_select())
		.filter(issues::active.eq(true))
		.filter(issues::resolved_at.is_null())
		.into_boxed();
	let on_target = issues::application_id
		.eq_any(application_ids.clone())
		.or(issues::machine_id.eq_any(machine_ids.clone()));
	query = if include_group_scoped {
		query.filter(on_target.or(issues::server_group_id.eq(gid)))
	} else {
		query.filter(on_target)
	};
	let open_issues: Vec<Issue> = query.load(db).await?;

	let now = Timestamp::now();
	for issue in open_issues {
		let Some((target, monitored)) = issue_target_and_monitored(db, &issue).await? else {
			continue;
		};
		re_evaluate_incident_membership(db, &issue, target, monitored, now, by).await?;
	}
	Ok(())
}

/// Re-evaluate every currently-open issue in the database against incident
/// membership. Intended as a startup pass for
/// background workers: if the rules around incident eligibility change
/// in code (or a migration silently bumped state the running code now
/// reads differently — see PR #170), this drives the actual incident
/// rows into agreement with what the current code says they should be.
///
/// Idempotent. Cheap when the database is already consistent — each
/// issue's `re_evaluate_incident_membership` call short-circuits in the
/// `_ => {}` arm when the incident state already matches.
///
/// Runs as a single transaction so a crash mid-reconcile doesn't leave
/// the incident state half-fixed. The per-issue FOR UPDATE locks held
/// inside `re_evaluate_incident_membership` accumulate across the
/// transaction's lifetime — fine at canopy's scale (hundreds of open
/// issues at most) but worth keeping in mind if the corpus grows.
///
/// Returns the number of issues evaluated, for the caller to log.
pub async fn reconcile_open_incidents(db: &mut AsyncPgConnection) -> Result<usize> {
	use crate::schema::issues::dsl;

	db.transaction::<_, AppError, _>(async |conn| {
		let open_issues: Vec<Issue> = dsl::issues
			.select(Issue::as_select())
			.filter(dsl::active.eq(true))
			.filter(dsl::resolved_at.is_null())
			.load(conn)
			.await?;

		let now = Timestamp::now();
		let mut evaluated = 0usize;
		for issue in open_issues {
			// An ungrouped application or machine has no target, so there is
			// nothing to reconcile for it.
			let Some((target, monitored)) = issue_target_and_monitored(conn, &issue).await? else {
				continue;
			};
			re_evaluate_incident_membership(conn, &issue, target, monitored, now, None).await?;
			evaluated += 1;
		}
		Ok(evaluated)
	})
	.await
}
/// Close incidents whose linger window has expired: `closing_at` (when the
/// last effective failure left) has outlived the target's window without a
/// failure returning. The close is backdated to `closing_at` — the linger
/// is damping machinery, not part of the incident's span — and the Slack
/// cancel-or-resolve runs exactly as an immediate close would have:
/// a never-shipped open is cancelled (the flap stays silent), a shipped
/// open gets its resolve.
///
/// Runs on the monitor pod's minute cadence. Each close is its own
/// transaction, re-checked under a row lock: a failure rejoining (which
/// clears `closing_at`) or an operator resolve (which sets `closed_at`)
/// between the scan and the lock wins. Belt-and-braces, a lingering
/// incident that somehow still has a live effective-failure member is
/// un-lingered instead of closed.
///
/// Returns the number of incidents closed.
pub async fn sweep_lingering_incidents(db: &mut AsyncPgConnection) -> Result<usize> {
	use crate::schema::{incident_issues, incidents, issues};

	let now = Timestamp::now();
	let candidates: Vec<Incident> = incidents::table
		.select(Incident::as_select())
		.filter(incidents::closed_at.is_null())
		.filter(incidents::closing_at.is_not_null())
		.load(db)
		.await?;

	let mut closed = 0usize;
	for candidate in candidates {
		let did_close = db
			.transaction::<_, AppError, _>(async |conn| {
				// Target lock first, before the incident row, matching the
				// order the open path takes them in: an incident's target
				// never changes, so `candidate` is a safe source for it, and
				// this is what stops a close landing inside the open path's
				// join decision.
				lock_target(conn, IncidentTarget::of_incident(&candidate)).await?;
				let incident: Option<Incident> = incidents::table
					.select(Incident::as_select())
					.filter(incidents::id.eq(candidate.id))
					.filter(incidents::closed_at.is_null())
					.for_update()
					.first(conn)
					.await
					.optional()?;
				let Some(incident) = incident else {
					return Ok(false);
				};
				let Some(closing_at) = incident.closing_at else {
					return Ok(false);
				};
				let target = IncidentTarget::of_incident(&incident);
				if closing_at + linger_window(conn, target).await? > now {
					return Ok(false);
				}
				let live_failures: i64 = incident_issues::table
					.inner_join(issues::table.on(issues::id.eq(incident_issues::issue_id)))
					.filter(incident_issues::incident_id.eq(incident.id))
					.filter(incident_issues::left_at.is_null())
					.filter(issues::effective_result.eq("failed"))
					.count()
					.get_result(conn)
					.await?;
				if live_failures > 0 {
					diesel::update(incidents::table.filter(incidents::id.eq(incident.id)))
						.set(incidents::closing_at.eq(None::<jiff_diesel::Timestamp>))
						.execute(conn)
						.await?;
					return Ok(false);
				}
				let closed_incident: Incident =
					diesel::update(incidents::table.filter(incidents::id.eq(incident.id)))
						.set(incidents::closed_at.eq(jiff_diesel::Timestamp::from(closing_at)))
						.returning(Incident::as_select())
						.get_result(conn)
						.await?;
				// Backdated to the close, matching `closed_at`: the linger is
				// damping machinery, not part of the incident's span.
				release_remaining_members(conn, closed_incident.id, closing_at).await?;
				enqueue_slack_resolve_inner(conn, &closed_incident, None).await?;
				Ok(true)
			})
			.await?;
		if did_close {
			closed += 1;
		}
	}
	Ok(closed)
}

/// Stamp `left_at` on every issue still attached to `incident_id`, so no
/// membership row outlives the incident it names.
///
/// A close only retires the `incidents` row: the current issue's own leave is
/// stamped by the leave arm, but sub-failure contributors are held attached
/// for context and nothing ever released them. Their rows then claimed a
/// membership that had ended, which is both untrue — the membership ran from
/// `joined_at` to the close, and `left_at` is where that end is recorded — and
/// the thing that used to strand an issue permanently outside the incident
/// workflow.
///
/// Call after the close, with the same timestamp the close was recorded at, so
/// the membership ends exactly when the incident did. Idempotent: a row that
/// already left keeps its own earlier stamp.
async fn release_remaining_members(
	conn: &mut AsyncPgConnection,
	incident_id: Uuid,
	at: Timestamp,
) -> Result<()> {
	use crate::schema::incident_issues;

	diesel::update(
		incident_issues::table
			.filter(incident_issues::incident_id.eq(incident_id))
			.filter(incident_issues::left_at.is_null()),
	)
	.set(incident_issues::left_at.eq(jiff_diesel::Timestamp::from(at)))
	.execute(conn)
	.await?;
	Ok(())
}

/// The still-open incident this issue is currently a live member of, if any.
///
/// Both halves of the filter matter. `left_at IS NULL` alone is not membership
/// in an *open* incident: closing an incident doesn't stamp `left_at` on the
/// members that never left (a sub-failure contributor is held attached for
/// context, and the close paths only touch the `incidents` row), so those rows
/// outlive their incident. Counting them made a stranded issue look
/// permanently attached, and `re_evaluate_incident_membership` then read it as
/// a member with nothing open — landing every subsequent effective failure in
/// the no-op arm instead of opening an incident. The server went red with
/// nothing paging, permanently, since no later event could clear the stale
/// row.
async fn open_incident_holding(
	db: &mut AsyncPgConnection,
	issue_id: Uuid,
) -> Result<Option<Incident>> {
	use crate::schema::{incident_issues, incidents};

	incident_issues::table
		.inner_join(incidents::table.on(incidents::id.eq(incident_issues::incident_id)))
		.select(Incident::as_select())
		.filter(
			incident_issues::issue_id
				.eq(issue_id)
				.and(incident_issues::left_at.is_null())
				.and(incidents::closed_at.is_null()),
		)
		.first(db)
		.await
		.optional()
		.map_err(AppError::from)
}

/// What an issue's incident contribution attaches to: one of a group's
/// environments, the group itself, or canopy as a whole for canopy-wide
/// issues (self-alerts). Issues on ungrouped applications and machines have
/// no target and no incident path.
// spec: INC#targets
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IncidentTarget {
	Environment(Uuid, ServerRank),
	Group(Uuid),
	Global,
}

impl IncidentTarget {
	/// The `incidents.server_group_id` value for this target.
	fn group_id(self) -> Option<Uuid> {
		match self {
			Self::Environment(gid, _) | Self::Group(gid) => Some(gid),
			Self::Global => None,
		}
	}

	/// The `incidents.rank` value for this target.
	fn rank(self) -> Option<ServerRank> {
		match self {
			Self::Environment(_, rank) => Some(rank),
			Self::Group(_) | Self::Global => None,
		}
	}

	/// The target an existing incident row belongs to.
	pub fn of_incident(incident: &Incident) -> Self {
		match (incident.server_group_id, incident.rank) {
			(Some(gid), Some(rank)) => Self::Environment(gid, rank),
			(Some(gid), None) => Self::Group(gid),
			(None, _) => Self::Global,
		}
	}

	/// The target a member of `group_id` carrying `rank` contributes to: the
	/// environment its rank names, and the group itself where it carries none,
	/// there being no environment for it to be in.
	// spec: INC#targets
	pub fn of_member(group_id: Uuid, rank: Option<ServerRank>) -> Self {
		match rank {
			Some(rank) => Self::Environment(group_id, rank),
			None => Self::Group(group_id),
		}
	}
}

async fn target_has_open_incident(
	db: &mut AsyncPgConnection,
	target: IncidentTarget,
) -> Result<bool> {
	use crate::schema::incidents::dsl;

	let mut q = dsl::incidents.filter(dsl::closed_at.is_null()).into_boxed();
	q = match target {
		IncidentTarget::Environment(gid, rank) => q
			.filter(dsl::server_group_id.eq(gid))
			.filter(dsl::rank.eq(rank)),
		IncidentTarget::Group(gid) => q
			.filter(dsl::server_group_id.eq(gid))
			.filter(dsl::rank.is_null()),
		IncidentTarget::Global => q.filter(dsl::server_group_id.is_null()),
	};
	let count: i64 = q.count().get_result(db).await?;
	Ok(count > 0)
}

/// Returns the open incident's id and whether this call newly opened it.
/// The boolean is consumed by `re_evaluate_incident_membership` to decide
/// whether a Slack `incident_open` outbox row should be enqueued (re-joining
/// an existing incident shouldn't re-notify).
///
/// Two parallel event pushes against the same target must not each insert
/// a fresh incident row. For a group target we take a `FOR UPDATE` lock on
/// the group's row up-front so the find-or-create pair serializes
/// per-group; the global target has no row to lock, so it serializes on a
/// transaction-scoped advisory lock instead. The unique partial indexes on
/// open incidents are the belt-and-braces backstop at the DB layer.
/// Serialise everything that opens or closes an incident for one target.
///
/// The open path decides "join the target's existing incident" from an
/// unlocked read, so without a common lock a concurrent close can land between
/// that read and the open, and `find_or_open_incident` then creates a *new*
/// incident — for an issue whose severity may not warrant one, since a
/// sub-Error issue's `should_join` is justified solely by "the target already
/// has one open". Both close paths take this too, so the read, the open and
/// the close all serialise on the same object.
///
/// Always taken *before* any `incidents` row lock, so the two lock orders
/// can't deadlock against each other.
async fn lock_target(db: &mut AsyncPgConnection, target: IncidentTarget) -> Result<()> {
	use crate::schema::server_groups;
	match target {
		// One lock per group, so a group's own target and its environments'
		// serialise against each other as well as against themselves.
		IncidentTarget::Environment(gid, _) | IncidentTarget::Group(gid) => {
			let _group_lock: Uuid = server_groups::table
				.select(server_groups::id)
				.filter(server_groups::id.eq(gid))
				.for_update()
				.first(db)
				.await?;
		}
		IncidentTarget::Global => {
			// Arbitrary constant, stable across releases: the one global
			// incident slot.
			diesel::sql_query("SELECT pg_advisory_xact_lock(818_723_001)")
				.execute(db)
				.await?;
		}
	}
	Ok(())
}

async fn find_or_open_incident(
	db: &mut AsyncPgConnection,
	target: IncidentTarget,
	opened_at: Timestamp,
) -> Result<(Uuid, bool)> {
	use crate::schema::incidents;

	lock_target(db, target).await?;

	let mut q = incidents::table
		.select(Incident::as_select())
		.filter(incidents::closed_at.is_null())
		.into_boxed();
	q = match target {
		IncidentTarget::Environment(gid, rank) => q
			.filter(incidents::server_group_id.eq(gid))
			.filter(incidents::rank.eq(rank)),
		IncidentTarget::Group(gid) => q
			.filter(incidents::server_group_id.eq(gid))
			.filter(incidents::rank.is_null()),
		IncidentTarget::Global => q.filter(incidents::server_group_id.is_null()),
	};
	let open: Option<Incident> = q
		.order(incidents::opened_at.desc())
		.first(db)
		.await
		.optional()?;
	if let Some(inc) = open {
		return Ok((inc.id, false));
	}

	let new_incident: Incident = diesel::insert_into(incidents::table)
		.values((
			incidents::server_group_id.eq(target.group_id()),
			incidents::rank.eq(target.rank()),
			incidents::opened_at.eq(jiff_diesel::Timestamp::from(opened_at)),
		))
		.returning(Incident::as_select())
		.get_result(db)
		.await?;
	Ok((new_incident.id, true))
}

/// Flap grace for canopy-wide incidents, mirroring the per-group
/// `slack_open_delay` default (the global target has no config row).
const GLOBAL_OPEN_GRACE: SignedDuration = SignedDuration::from_secs(3 * 60);

/// Linger window for canopy-wide incidents, mirroring the per-group
/// `slack_close_delay` default (the global target has no config row).
const GLOBAL_CLOSE_GRACE: SignedDuration = SignedDuration::from_secs(5 * 60);

/// The target's linger window: how long an incident outlives its last
/// effective failure before it closes.
async fn linger_window(
	conn: &mut AsyncPgConnection,
	target: IncidentTarget,
) -> Result<SignedDuration> {
	Ok(match target {
		IncidentTarget::Environment(gid, _) | IncidentTarget::Group(gid) => {
			ServerGroup::get_by_id(conn, gid).await?.slack_close_delay.0
		}
		IncidentTarget::Global => GLOBAL_CLOSE_GRACE,
	})
}

async fn enqueue_slack_open(
	conn: &mut AsyncPgConnection,
	incident_id: Uuid,
	target: IncidentTarget,
	issue: &Issue,
) -> Result<()> {
	let (label, open_delay) = match target {
		IncidentTarget::Environment(gid, _) | IncidentTarget::Group(gid) => {
			let group = ServerGroup::get_by_id(conn, gid).await?;
			let server = match issue.application_id {
				Some(sid) => Some(Application::get_by_id(conn, sid).await?),
				None => None,
			};
			(
				format_group_label(&group, target.rank(), server.as_ref()),
				group.slack_open_delay.0,
			)
		}
		IncidentTarget::Global => ("Canopy".to_string(), GLOBAL_OPEN_GRACE),
	};
	// The deployed Slack workflow's trigger declares a `severity`
	// variable; feed it the result-derived urgency label.
	let urgency = if issue.escalates_now() {
		"Critical"
	} else if issue.opens_incident() {
		"Error"
	} else {
		"Warning"
	};
	let payload = crate::slack_outbox::vars::incident_open(
		&label,
		urgency,
		&issue.source,
		&issue.r#ref,
		&issue.message,
	);
	// Normally the row sits in the outbox for the target's open delay
	// before the drainer can ship it — that's the flap-suppression
	// window, so a transient open/close pair never reaches Slack.
	// An escalating failure bypasses this: the operator has marked the
	// check's policy as escalating — they don't want any delay.
	let deliver_after = if issue.escalates_now() {
		Timestamp::now()
	} else {
		Timestamp::now() + open_delay
	};
	crate::slack_outbox::SlackOutbox::enqueue(
		conn,
		crate::slack_outbox::KIND_INCIDENT_OPEN,
		Some(incident_id),
		Some(issue.id),
		None,
		payload,
		deliver_after,
	)
	.await?;
	Ok(())
}

/// Pull forward the `deliver_after` of any pending `incident_open` row
/// for `incident_id` to now. Used when a Critical-severity issue joins
/// an incident whose open was originally enqueued by a lower-severity
/// contributor — the operator's signal is "no more delay".
///
/// Affects only rows that are still in their delay window
/// (`delivered_at IS NULL`, `gave_up_at IS NULL`, `deliver_after > now`).
/// Already-shipped or cancelled rows are left alone.
///
/// Returns true if at least one row was accelerated. The caller uses
/// this to distinguish "open is still pending" (no extra Slack message
/// needed) from "open has already shipped" (fire an escalation open).
async fn accelerate_pending_open(conn: &mut AsyncPgConnection, incident_id: Uuid) -> Result<bool> {
	use crate::schema::slack_outbox::dsl;
	let now = Timestamp::now();
	let updated = diesel::update(
		dsl::slack_outbox
			.filter(dsl::incident_id.eq(Some(incident_id)))
			.filter(dsl::kind.eq(crate::slack_outbox::KIND_INCIDENT_OPEN))
			.filter(dsl::delivered_at.is_null())
			.filter(dsl::gave_up_at.is_null())
			.filter(dsl::deliver_after.gt(jiff_diesel::Timestamp::from(now))),
	)
	.set(dsl::deliver_after.eq(jiff_diesel::Timestamp::from(now)))
	.execute(conn)
	.await
	.map_err(AppError::from)?;
	Ok(updated > 0)
}

async fn enqueue_slack_resolve(
	conn: &mut AsyncPgConnection,
	incident: &Incident,
	by: &str,
) -> Result<()> {
	enqueue_slack_resolve_inner(conn, incident, Some(by)).await
}

async fn enqueue_slack_resolve_inner(
	conn: &mut AsyncPgConnection,
	incident: &Incident,
	by: Option<&str>,
) -> Result<()> {
	// If the matching open is still inside its `deliver_after` window
	// (or has otherwise not been delivered yet) we cancel it and skip
	// posting the resolve too: Slack never heard about the incident,
	// so there's nothing to "resolve" there. Once the drainer has shipped
	// the open this update affects zero rows and we fall through to the
	// normal enqueue.
	let cancelled = crate::slack_outbox::SlackOutbox::cancel_pending_open(
		conn,
		incident.id,
		"cancelled: incident resolved before the open had been delivered to Slack",
	)
	.await?;
	// Having cancelled something is not proof Slack is unaware. Escalation
	// enqueues a *second* `incident_open` for the same incident, so a
	// delivered original can coexist with a pending escalation open — and
	// cancelling the latter would then suppress the resolve for an incident
	// Slack is actively showing as open, permanently. What matters is whether
	// any open was ever delivered.
	let ever_delivered = crate::slack_outbox::SlackOutbox::delivered_open_ids(conn, &[incident.id])
		.await?
		.contains(&incident.id);
	if cancelled > 0 && !ever_delivered {
		return Ok(());
	}
	let label = match incident.server_group_id {
		Some(gid) => {
			let group = ServerGroup::get_by_id(conn, gid).await?;
			format_group_label(&group, incident.rank, None)
		}
		None => "Canopy".to_string(),
	};
	let payload = crate::slack_outbox::vars::incident_resolve(&label, by);
	crate::slack_outbox::SlackOutbox::enqueue(
		conn,
		crate::slack_outbox::KIND_INCIDENT_RESOLVE,
		Some(incident.id),
		None,
		None,
		payload,
		Timestamp::now(),
	)
	.await?;
	Ok(())
}

/// How an incident's target reads in a notification: the environment it is on,
/// and the application the issue was filed against where there is one.
// spec: INC#notification
pub(crate) fn format_group_label(
	group: &ServerGroup,
	rank: Option<ServerRank>,
	server: Option<&Application>,
) -> String {
	let group_name = match rank {
		Some(rank) => crate::server_groups::environment_name(&group.name, rank),
		None => group.name.clone(),
	};
	if let Some(server) = server {
		let host = server.host.as_ref().map(|h| h.0.to_string());
		let server_part = match (&server.name, host) {
			(Some(n), Some(h)) if !n.is_empty() => format!("{n} ({h})"),
			(Some(n), None) if !n.is_empty() => n.clone(),
			(_, Some(h)) => h,
			(_, None) => server.id.to_string(),
		};
		format!("{group_name} · {server_part}")
	} else {
		group_name
	}
}

impl Issue {
	pub async fn list_for_device(
		db: &mut AsyncPgConnection,
		device_id: Uuid,
		filter: IssueFilter,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::issues::dsl;

		let mut q = dsl::issues
			.select(Self::as_select())
			.filter(dsl::device_id.eq(device_id))
			// Healthy check state (rows that never degraded) isn't an
			// issue; it has its own read surfaces.
			.filter(dsl::active.eq(true).or(dsl::last_degraded_at.is_not_null()))
			.into_boxed();
		if filter == IssueFilter::ActiveOnly {
			q = q
				.filter(dsl::active.eq(true))
				.filter(dsl::resolved_at.is_null());
		}
		q.order(dsl::last_seen.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_for_server(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		filter: IssueFilter,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::issues::dsl;

		let mut q = dsl::issues
			.select(Self::as_select())
			.filter(dsl::application_id.eq(application_id))
			// Healthy check state (rows that never degraded) isn't an
			// issue; it has its own read surfaces.
			.filter(dsl::active.eq(true).or(dsl::last_degraded_at.is_not_null()))
			.into_boxed();
		if filter == IssueFilter::ActiveOnly {
			q = q
				.filter(dsl::active.eq(true))
				.filter(dsl::resolved_at.is_null());
		}
		q.order(dsl::last_seen.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Filtered cross-server issues list. Used by the global Incidents page.
	///
	/// - `active_only`: when true, only `active = true` *and* unresolved
	///   issues — operator-resolved items don't count even if the source
	///   keeps pushing them.
	/// - `results`: when `Some` and non-empty, restrict to issues whose
	///   latest effective result is one of those.
	/// - `server_group_id`: when `Some`, restrict to issues whose server is
	///   in that group. A direct `IN (SELECT id FROM applications WHERE group_id = $)`.
	/// - `application_id`: when `Some`, restrict to issues on that server.
	///
	/// Every filter is applied in SQL, so `limit` bounds the *filtered* set.
	/// Narrowing a page after the fact would let a busy fleet push a quiet
	/// server's issues off the end and report it clean.
	pub async fn list(
		db: &mut AsyncPgConnection,
		filters: IssueListFilters,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::applications;
		use crate::schema::issues::dsl;

		let mut q = dsl::issues
			.select(Self::as_select())
			// Canopy-wide issues (self-alerts: scoped to no target at all)
			// have their own surface (`crate::self_alerts`); they are not
			// fleet issues. A machine-scoped issue is one, and reading only
			// the application and group columns hid every machine check.
			.filter(
				dsl::application_id
					.is_not_null()
					.or(dsl::machine_id.is_not_null())
					.or(dsl::server_group_id.is_not_null()),
			)
			// Healthy check state (rows that never degraded) isn't an
			// issue; it has its own read surfaces.
			.filter(dsl::active.eq(true).or(dsl::last_degraded_at.is_not_null()))
			.into_boxed();
		if filters.active_only {
			q = q
				.filter(dsl::active.eq(true))
				.filter(dsl::resolved_at.is_null());
		}
		if let Some(results) = filters.results.as_ref().filter(|v| !v.is_empty()) {
			let strs: Vec<String> = results.iter().map(|r| r.to_string()).collect();
			q = q.filter(dsl::effective_result.eq_any(strs));
		}
		if let Some(gid) = filters.server_group_id {
			// A group's issues are its applications', its machines', and its
			// own. Collecting only applications dropped every machine check
			// from a group's view.
			let server_ids: Vec<Uuid> = applications::table
				.select(applications::id)
				.filter(applications::group_id.eq(gid))
				.load(db)
				.await?;
			let machine_ids: Vec<Uuid> = crate::schema::machines::table
				.select(crate::schema::machines::id)
				.filter(crate::schema::machines::group_id.eq(gid))
				.load(db)
				.await?;
			q = q.filter(
				dsl::application_id
					.eq_any(server_ids)
					.or(dsl::machine_id.eq_any(machine_ids))
					.or(dsl::server_group_id.eq(gid)),
			);
		}
		if let Some(sid) = filters.application_id {
			// The machine's issues come with the application's, which is what
			// the application's own detail page presents.
			// spec: MCP#incidents-and-issues
			let machine_id: Option<Uuid> = applications::table
				.select(applications::machine_id)
				.filter(applications::id.eq(sid))
				.first(db)
				.await
				.optional()?;
			q = q.filter(
				dsl::application_id.eq(sid).or(dsl::machine_id
					.is_not_distinct_from(machine_id)
					.and(dsl::machine_id.is_not_null())),
			);
		}
		if let Some(mid) = filters.machine_id {
			q = q.filter(dsl::machine_id.eq(mid));
		}
		if let Some(since) = filters.since {
			q = q.filter(dsl::last_seen.ge(jiff_diesel::Timestamp::from(since)));
		}
		q.order(dsl::last_seen.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn get_by_id(db: &mut AsyncPgConnection, issue_id: Uuid) -> Result<Self> {
		use crate::schema::issues::dsl;

		dsl::issues
			.select(Self::as_select())
			.filter(dsl::id.eq(issue_id))
			.first(db)
			.await
			.map_err(AppError::from)
	}

	/// Refs of the server's *active* issues under `source` whose ref
	/// starts with `prefix`. Used by status ingestion to decide which
	/// per-check issues to close: deriving closes from what's actually
	/// open (rather than diffing against the previous status row)
	/// stays correct across interludes that keep an issue open without
	/// re-filing it — e.g. failed → broken → passed, where the failure
	/// issue must close on the `passed` push even though the previous
	/// push didn't report a failure.
	/// The machine-grain counterpart of [`Self::active_refs_with_prefix`].
	///
	/// The two grains need separate previously-active sets. Sharing one would
	/// make a check that moves grain read as unmentioned on the grain it left,
	/// closing and reopening it as a new issue on every push.
	// spec: STA
	pub async fn active_refs_with_prefix_for_machine(
		db: &mut AsyncPgConnection,
		machine_id: Uuid,
		source: &str,
		prefix: &str,
	) -> Result<Vec<String>> {
		use crate::schema::issues::dsl;
		debug_assert!(
			!prefix.contains(['%', '_', '\\']),
			"prefix is used in a LIKE pattern and must not contain wildcards"
		);
		dsl::issues
			.select(dsl::ref_)
			.filter(
				dsl::machine_id
					.eq(machine_id)
					.and(dsl::source.eq(source))
					.and(dsl::ref_.like(format!("{prefix}%")))
					.and(dsl::active.eq(true)),
			)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn active_refs_with_prefix(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		source: &str,
		prefix: &str,
	) -> Result<Vec<String>> {
		use crate::schema::issues::dsl;
		debug_assert!(
			!prefix.contains(['%', '_', '\\']),
			"prefix is used in a LIKE pattern and must not contain wildcards"
		);
		dsl::issues
			.select(dsl::ref_)
			.filter(
				dsl::application_id
					.eq(application_id)
					.and(dsl::source.eq(source))
					.and(dsl::ref_.like(format!("{prefix}%")))
					.and(dsl::active.eq(true)),
			)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Bulk lookup of issues that share the same `(source, ref)` across many
	/// applications. Each `(application_id, source, ref)` is unique, so at most one row
	/// per server is returned. Used by the canopy reachability sweep.
	/// Group ids carrying an active group-scoped issue with this
	/// `(source, ref)`. Used by sweeps to recover only where an alert is
	/// live, keeping their idle path read-only.
	pub async fn active_group_ids_by_source_ref(
		db: &mut AsyncPgConnection,
		source: &str,
		ref_: &str,
	) -> Result<Vec<Uuid>> {
		use crate::schema::issues::dsl;
		dsl::issues
			.filter(dsl::source.eq(source))
			.filter(dsl::ref_.eq(ref_))
			.filter(dsl::active.eq(true))
			.filter(dsl::server_group_id.is_not_null())
			.select(dsl::server_group_id.assume_not_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Application ids carrying an active server-scoped issue with this
	/// `(source, ref)`. The counterpart to
	/// [`Self::active_group_ids_by_source_ref`], for the sweeps whose work list is
	/// "what is wrong now" and which need "what was wrong last time" to know what
	/// to close.
	pub async fn active_server_ids_by_source_ref(
		db: &mut AsyncPgConnection,
		source: &str,
		ref_: &str,
	) -> Result<Vec<Uuid>> {
		use crate::schema::issues::dsl;
		dsl::issues
			.filter(dsl::source.eq(source))
			.filter(dsl::ref_.eq(ref_))
			.filter(dsl::active.eq(true))
			.filter(dsl::application_id.is_not_null())
			.select(dsl::application_id.assume_not_null())
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_by_source_ref(
		db: &mut AsyncPgConnection,
		source: &str,
		ref_: &str,
		server_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::issues::dsl;
		if server_ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::issues
			.select(Self::as_select())
			.filter(
				dsl::source
					.eq(source)
					.and(dsl::ref_.eq(ref_))
					.and(dsl::application_id.eq_any(server_ids)),
			)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Like [`Self::list_by_source_ref`], but keyed by machine rather than by
	/// application. Drives sweeps whose subject is the box.
	pub async fn list_by_source_ref_for_machines(
		db: &mut AsyncPgConnection,
		source: &str,
		ref_: &str,
		machine_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::issues::dsl;
		if machine_ids.is_empty() {
			return Ok(Vec::new());
		}
		dsl::issues
			.select(Self::as_select())
			.filter(
				dsl::source
					.eq(source)
					.and(dsl::ref_.eq(ref_))
					.and(dsl::machine_id.eq_any(machine_ids)),
			)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Like [`Self::list_by_source_ref`], but matching any of several refs
	/// at once. Used by the staleness sweep, whose per-source check refs
	/// are only known at runtime.
	pub async fn list_by_source_refs(
		db: &mut AsyncPgConnection,
		source: &str,
		refs: &[String],
		server_ids: &[Uuid],
	) -> Result<Vec<Self>> {
		use crate::schema::issues::dsl;
		if server_ids.is_empty() || refs.is_empty() {
			return Ok(Vec::new());
		}
		dsl::issues
			.select(Self::as_select())
			.filter(
				dsl::source
					.eq(source)
					.and(dsl::ref_.eq_any(refs))
					.and(dsl::application_id.eq_any(server_ids)),
			)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Most recent check-state stamp per (server, reporting source): every
	/// report a source pushes re-stamps `last_seen` on the state rows of
	/// the checks it mentions, so the max per source is when that source
	/// last reported — maintained incrementally by ingestion, with no scan
	/// of the statuses history. The reserved sources are excluded (canopy
	/// and manual filings aren't reports), as are rows never stamped by
	/// the check-state model.
	pub async fn source_freshness(
		db: &mut AsyncPgConnection,
		server_ids: &[Uuid],
	) -> Result<Vec<(Uuid, String, Timestamp)>> {
		use crate::check_policies::CheckPolicy;
		use crate::schema::issues::dsl;
		use std::collections::HashMap;
		if server_ids.is_empty() {
			return Ok(Vec::new());
		}

		// A source is only "expected" through check-states backed by a live
		// catalog row: this drops sources whose every check is decommissioned,
		// and orphaned sources whose check-states have no catalog row at all
		// (e.g. a superseded reporter like bestool-alertd) — otherwise a dead
		// source would hold applications into a reachability warning forever, with
		// no catalog entry to silence or decommission. Mirrors the health
		// rollup (see [`health_from_check_state`]).
		let cataloged = CheckPolicy::live_cataloged_pairs(db).await?;
		let types = crate::applications::Application::types_by_id(db, server_ids).await?;

		let rows: Vec<(Uuid, String, String, jiff_diesel::Timestamp)> = dsl::issues
			.filter(
				dsl::application_id
					.eq_any(server_ids)
					.and(
						dsl::source
							.ne_all([crate::statuses::CANOPY_SOURCE, crate::issues::MANUAL_SOURCE]),
					)
					.and(dsl::check_name.is_not_null()),
			)
			.select((
				dsl::application_id.assume_not_null(),
				dsl::source,
				dsl::check_name.assume_not_null(),
				dsl::last_seen,
			))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let mut latest: HashMap<(Uuid, String), Timestamp> = HashMap::new();
		for (server, source, check, seen) in rows {
			if !CheckPolicy::live_for(&cataloged, &source, &check, types.get(&server)) {
				continue;
			}
			let seen: Timestamp = seen.into();
			latest
				.entry((server, source))
				.and_modify(|t| {
					if seen > *t {
						*t = seen;
					}
				})
				.or_insert(seen);
		}
		Ok(latest
			.into_iter()
			.map(|((server, source), seen)| (server, source, seen))
			.collect())
	}

	/// As [`Self::source_freshness`], at the machine grain.
	///
	/// A machine's expected sources are its own: the agent reporting about the
	/// box files at machine scope, and a source that only ever reports about
	/// the workloads on it says nothing about whether the box is reachable.
	/// Every check filed at machine scope sits in the machine namespace, so
	/// there is no type to qualify the catalog lookup by.
	// spec: CHK#reachability
	pub async fn source_freshness_for_machines(
		db: &mut AsyncPgConnection,
		machine_ids: &[Uuid],
	) -> Result<Vec<(Uuid, String, Timestamp)>> {
		use crate::check_policies::CheckPolicy;
		use crate::schema::issues::dsl;
		use commons_types::namespace::Namespace;
		use std::collections::HashMap;
		if machine_ids.is_empty() {
			return Ok(Vec::new());
		}

		let cataloged = CheckPolicy::live_cataloged_pairs(db).await?;

		let rows: Vec<(Uuid, String, String, jiff_diesel::Timestamp)> = dsl::issues
			.filter(
				dsl::machine_id
					.eq_any(machine_ids)
					.and(
						dsl::source
							.ne_all([crate::statuses::CANOPY_SOURCE, crate::issues::MANUAL_SOURCE]),
					)
					.and(dsl::check_name.is_not_null()),
			)
			.select((
				dsl::machine_id.assume_not_null(),
				dsl::source,
				dsl::check_name.assume_not_null(),
				dsl::last_seen,
			))
			.load(db)
			.await
			.map_err(AppError::from)?;

		let mut latest: HashMap<(Uuid, String), Timestamp> = HashMap::new();
		for (machine, source, check, seen) in rows {
			let namespace = Namespace::for_machine(&source, &check);
			if !CheckPolicy::live_in(&cataloged, &source, &namespace, &check) {
				continue;
			}
			let seen: Timestamp = seen.into();
			latest
				.entry((machine, source))
				.and_modify(|t| {
					if seen > *t {
						*t = seen;
					}
				})
				.or_insert(seen);
		}
		Ok(latest
			.into_iter()
			.map(|((machine, source), seen)| (machine, source, seen))
			.collect())
	}

	/// Mark an issue as operator-resolved. Triggers incident-membership
	/// re-evaluation (typically: leaves the incident).
	pub async fn resolve(
		db: &mut AsyncPgConnection,
		issue_id: Uuid,
		by: &str,
		reason: commons_types::issue::ResolvedReason,
	) -> Result<Self> {
		use crate::schema::issues;
		let now = Timestamp::now();

		db.transaction::<_, AppError, _>(async |conn| {
			let issue = diesel::update(issues::table.filter(issues::id.eq(issue_id)))
				.set((
					issues::resolved_at.eq(jiff_diesel::Timestamp::from(now)),
					issues::resolved_by.eq(Some(by)),
					issues::resolved_reason.eq(Some(reason.to_string())),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await?;
			if let Some((target, monitored)) = issue_target_and_monitored(conn, &issue).await? {
				re_evaluate_incident_membership(conn, &issue, target, monitored, now, Some(by))
					.await?;
			}
			Ok(issue)
		})
		.await
	}

	pub async fn unresolve(db: &mut AsyncPgConnection, issue_id: Uuid) -> Result<Self> {
		use crate::schema::issues;
		let now = Timestamp::now();

		db.transaction::<_, AppError, _>(async |conn| {
			let issue = diesel::update(issues::table.filter(issues::id.eq(issue_id)))
				.set((
					issues::resolved_at.eq(None::<jiff_diesel::Timestamp>),
					issues::resolved_by.eq(None::<String>),
					issues::resolved_reason.eq(None::<String>),
				))
				.returning(Self::as_select())
				.get_result(conn)
				.await?;
			// Unresolve rejoins; cascade close path doesn't fire here.
			if let Some((target, monitored)) = issue_target_and_monitored(conn, &issue).await? {
				re_evaluate_incident_membership(conn, &issue, target, monitored, now, None).await?;
			}
			Ok(issue)
		})
		.await
	}

	/// Snooze an issue until the given timestamp. While snoozed, the issue
	/// can't open or join incidents. Triggers re-evaluation.
	///
	/// Snooze doesn't carry an operator login today, so a cascade close
	/// triggered by snoozing the last live issue still attributes the
	/// Slack resolve to "the healthcheck recovering" rather than the
	/// operator. Worth revisiting if/when snooze records its `by`.
	pub async fn snooze(
		db: &mut AsyncPgConnection,
		issue_id: Uuid,
		until: Timestamp,
	) -> Result<Self> {
		use crate::schema::issues;
		let now = Timestamp::now();

		db.transaction::<_, AppError, _>(async |conn| {
			let issue = diesel::update(issues::table.filter(issues::id.eq(issue_id)))
				.set(issues::snoozed_until.eq(jiff_diesel::Timestamp::from(until)))
				.returning(Self::as_select())
				.get_result(conn)
				.await?;
			if let Some((target, monitored)) = issue_target_and_monitored(conn, &issue).await? {
				re_evaluate_incident_membership(conn, &issue, target, monitored, now, None).await?;
			}
			Ok(issue)
		})
		.await
	}

	pub async fn unsnooze(db: &mut AsyncPgConnection, issue_id: Uuid) -> Result<Self> {
		use crate::schema::issues;
		let now = Timestamp::now();

		db.transaction::<_, AppError, _>(async |conn| {
			let issue = diesel::update(issues::table.filter(issues::id.eq(issue_id)))
				.set(issues::snoozed_until.eq(None::<jiff_diesel::Timestamp>))
				.returning(Self::as_select())
				.get_result(conn)
				.await?;
			if let Some((target, monitored)) = issue_target_and_monitored(conn, &issue).await? {
				re_evaluate_incident_membership(conn, &issue, target, monitored, now, None).await?;
			}
			Ok(issue)
		})
		.await
	}
}

/// Minimal incident metadata returned alongside an issue so the UI can
/// link from the issue card to the incident(s) the issue is attached to.
#[derive(Debug, Clone)]
pub struct IssueIncidentRef {
	pub incident_id: Uuid,
	pub opened_at: Timestamp,
	pub closed_at: Option<Timestamp>,
}

/// Aggregate counts displayed against an incident in the UI.
#[derive(Debug, Clone, Copy, Default)]
pub struct IncidentStats {
	pub issue_count: i64,
	/// `incident_notes` for this incident + `issue_notes` across all linked issues.
	pub note_count: i64,
}

impl Incident {
	/// Bulk-fetch stats for a set of incidents. Missing incident_ids get
	/// `Default` (zero).
	///
	/// `incident_issues` is keyed on `(incident_id, issue_id, joined_at)`,
	/// so an issue that leaves and rejoins the same incident produces
	/// multiple rows for the same pair. Counts must dedupe on the pair
	/// before joining to `issue_notes`, otherwise every note gets
	/// multiplied by the rejoin count.
	///
	/// Strategy: pull the distinct `(incident_id, issue_id)` pairs first,
	/// then run the per-issue note counts and the direct incident_notes
	/// count concurrently. Takes the pool (`&Db`) so the parallel futures
	/// don't fight over one mutable conn handle.
	pub async fn stats_for(
		pool: &crate::Db,
		incident_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, IncidentStats>> {
		use crate::schema::{incident_issues, incident_notes, issue_notes};
		use diesel::dsl::count_star;
		use std::collections::{HashMap, HashSet};

		let mut out: HashMap<Uuid, IncidentStats> = incident_ids
			.iter()
			.map(|id| (*id, IncidentStats::default()))
			.collect();
		if incident_ids.is_empty() {
			return Ok(out);
		}

		let ids = incident_ids.to_vec();
		let pairs: Vec<(Uuid, Uuid)> = {
			let mut c = pool.get().await?;
			incident_issues::table
				.filter(incident_issues::incident_id.eq_any(&ids))
				.select((incident_issues::incident_id, incident_issues::issue_id))
				.distinct()
				.load(&mut c)
				.await?
		};

		let mut issues_by_incident: HashMap<Uuid, HashSet<Uuid>> = HashMap::new();
		for (incident_id, issue_id) in &pairs {
			issues_by_incident
				.entry(*incident_id)
				.or_default()
				.insert(*issue_id);
		}
		for (incident_id, issues) in &issues_by_incident {
			out.entry(*incident_id).or_default().issue_count = issues.len() as i64;
		}

		let unique_issue_ids: Vec<Uuid> = issues_by_incident
			.values()
			.flatten()
			.copied()
			.collect::<std::collections::HashSet<_>>()
			.into_iter()
			.collect();

		let f_inotes = async {
			let mut c = pool.get().await?;
			incident_notes::table
				.group_by(incident_notes::incident_id)
				.select((incident_notes::incident_id, count_star()))
				.filter(incident_notes::incident_id.eq_any(&ids))
				.load::<(Uuid, i64)>(&mut c)
				.await
				.map_err(AppError::from)
		};
		let f_jnotes = async {
			if unique_issue_ids.is_empty() {
				return Result::<Vec<(Uuid, i64)>>::Ok(Vec::new());
			}
			let mut c = pool.get().await?;
			issue_notes::table
				.group_by(issue_notes::issue_id)
				.select((issue_notes::issue_id, count_star()))
				.filter(issue_notes::issue_id.eq_any(&unique_issue_ids))
				.load::<(Uuid, i64)>(&mut c)
				.await
				.map_err(AppError::from)
		};
		let (inote_rows, jnote_rows) = futures::try_join!(f_inotes, f_jnotes)?;

		let notes_per_issue: HashMap<Uuid, i64> = jnote_rows.into_iter().collect();

		for (incident_id, issues) in &issues_by_incident {
			let entry = out.entry(*incident_id).or_default();
			for issue_id in issues {
				entry.note_count += notes_per_issue.get(issue_id).copied().unwrap_or(0);
			}
		}
		for (id, n) in inote_rows {
			out.entry(id).or_default().note_count += n;
		}

		Ok(out)
	}
}

impl Incident {
	/// An incident is owned by the environment an application is in, so a
	/// caller asking for one application's incidents wants that
	/// environment's, and the group's own where the application belongs to no
	/// environment. Ungrouped applications return an empty Vec.
	// spec: INC#targets
	pub async fn list_for_server(
		db: &mut AsyncPgConnection,
		application_id: Uuid,
		include_closed: bool,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::incidents::dsl;

		let application = Application::get_by_id(db, application_id).await?;
		let Some(gid) = application.group_id else {
			return Ok(Vec::new());
		};
		let mut q = dsl::incidents
			.select(Self::as_select())
			.filter(dsl::server_group_id.eq(gid))
			.filter(dsl::rank.is_not_distinct_from(application.rank))
			.into_boxed();
		if !include_closed {
			q = q.filter(dsl::closed_at.is_null());
		}
		q.order(dsl::opened_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_for_group(
		db: &mut AsyncPgConnection,
		server_group_id: Uuid,
		include_closed: bool,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::incidents::dsl;

		let mut q = dsl::incidents
			.select(Self::as_select())
			.filter(dsl::server_group_id.eq(server_group_id))
			.into_boxed();
		if !include_closed {
			q = q.filter(dsl::closed_at.is_null());
		}
		q.order(dsl::opened_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	pub async fn list_active(db: &mut AsyncPgConnection, limit: i64) -> Result<Vec<Self>> {
		use crate::schema::incidents::dsl;

		dsl::incidents
			.select(Self::as_select())
			.filter(dsl::closed_at.is_null())
			.order(dsl::opened_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Incidents that were open at any point at or after `since`: either still
	/// open (`closed_at IS NULL`) or closed no earlier than `since`. Optionally
	/// restricted to one group. Ordered newest-opened first. Drives historical
	/// queries like "incidents open in the past week".
	/// Incidents that were open at any point since `since` — still open, or
	/// closed within the window — narrowed by `status`, newest-opened first.
	///
	/// `status` is applied in SQL so `limit` bounds the *filtered* set. The
	/// window routinely holds more sub-grace flap rows than the limit, and
	/// they sort by `opened_at` alongside everything else, so narrowing a page
	/// afterwards can hide a still-open incident entirely.
	pub async fn list_open_since(
		db: &mut AsyncPgConnection,
		since: Timestamp,
		group_id: Option<Uuid>,
		status: IncidentStatusFilter,
		limit: i64,
	) -> Result<Vec<Self>> {
		use crate::schema::incidents::dsl;

		let mut q = dsl::incidents
			.select(Self::as_select())
			.filter(
				dsl::closed_at
					.is_null()
					.or(dsl::closed_at.ge(jiff_diesel::Timestamp::from(since))),
			)
			.into_boxed();
		if let Some(gid) = group_id {
			q = q.filter(dsl::server_group_id.eq(gid));
		}
		match status {
			IncidentStatusFilter::All => {}
			IncidentStatusFilter::Open => q = q.filter(dsl::closed_at.is_null()),
			IncidentStatusFilter::Resolved => q = q.filter(dsl::resolved_at.is_not_null()),
		}
		q.order(dsl::opened_at.desc())
			.limit(limit)
			.load(db)
			.await
			.map_err(AppError::from)
	}

	/// Bulk lookup: for each issue id, the distinct incidents it is or was
	/// linked to, with their open/close timestamps. Used to surface the
	/// "attaching incident(s)" pill on each issue card.
	///
	/// The same `(issue, incident)` pair can have multiple `incident_issues`
	/// rows (issue left and rejoined). We dedupe on `incident_id` and order
	/// by `opened_at desc` so the most recent attachment is first.
	pub async fn for_issues(
		db: &mut AsyncPgConnection,
		issue_ids: &[Uuid],
	) -> Result<std::collections::HashMap<Uuid, Vec<IssueIncidentRef>>> {
		use crate::schema::{incident_issues, incidents};
		use std::collections::HashMap;

		let mut out: HashMap<Uuid, Vec<IssueIncidentRef>> = HashMap::new();
		if issue_ids.is_empty() {
			return Ok(out);
		}

		let rows: Vec<(
			Uuid,
			Uuid,
			jiff_diesel::Timestamp,
			jiff_diesel::NullableTimestamp,
		)> = incident_issues::table
			.inner_join(incidents::table.on(incidents::id.eq(incident_issues::incident_id)))
			.filter(incident_issues::issue_id.eq_any(issue_ids))
			.select((
				incident_issues::issue_id,
				incidents::id,
				incidents::opened_at,
				incidents::closed_at,
			))
			.distinct()
			.order(incidents::opened_at.desc())
			.load(db)
			.await?;

		for (issue_id, incident_id, opened_at, closed_at) in rows {
			out.entry(issue_id).or_default().push(IssueIncidentRef {
				incident_id,
				opened_at: opened_at.into(),
				closed_at: Option::<Timestamp>::from(closed_at),
			});
		}
		Ok(out)
	}

	/// One row per distinct issue. An issue that left and rejoined this
	/// incident shows up once with its most recent link metadata
	/// (`joined_at`/`left_at`); without the dedup a flapping issue can
	/// produce hundreds of repeated rows.
	pub async fn get_with_issues(
		db: &mut AsyncPgConnection,
		incident_id: Uuid,
	) -> Result<(Self, Vec<(IncidentIssue, Issue)>)> {
		use crate::schema::{incident_issues, incidents, issues};

		let incident: Self = incidents::table
			.select(Self::as_select())
			.filter(incidents::id.eq(incident_id))
			.first(db)
			.await?;

		let mut rows: Vec<(IncidentIssue, Issue)> = incident_issues::table
			.inner_join(issues::table.on(issues::id.eq(incident_issues::issue_id)))
			.select((IncidentIssue::as_select(), Issue::as_select()))
			.filter(incident_issues::incident_id.eq(incident_id))
			.distinct_on(incident_issues::issue_id)
			.order((incident_issues::issue_id, incident_issues::joined_at.desc()))
			.load(db)
			.await?;
		rows.sort_by_key(|(link, _)| link.joined_at);
		Ok((incident, rows))
	}

	/// Mark an incident as operator-resolved. This is metadata only — it does
	/// *not* force `closed_at` (that's still driven by auto rules). An
	/// open incident can be resolved (the cause is dealt with) and a closed
	/// incident can be resolved retroactively.
	/// Resolve the incident. Cascades to every currently-contributing issue
	/// (active link), marking them with the same reason and timestamp; each
	/// one triggers `re_evaluate_incident_membership`, so the normal
	/// auto-close path sets `closed_at` once the last issue leaves. As a
	/// belt-and-suspenders, force-closes the incident if the cascade left
	/// it open (e.g. an incident with no live links — shouldn't happen, but
	/// we'd rather not leave the UI in a 'resolved but still open' state).
	pub async fn resolve(
		db: &mut AsyncPgConnection,
		incident_id: Uuid,
		by: &str,
		reason: commons_types::issue::ResolvedReason,
	) -> Result<Self> {
		use crate::schema::{incident_issues, incidents, issues};
		let now = Timestamp::now();

		db.transaction::<_, AppError, _>(async |conn| {
			let incident_loaded: Incident = incidents::table
				.select(Incident::as_select())
				.filter(incidents::id.eq(incident_id))
				.first(conn)
				.await?;
			let target = IncidentTarget::of_incident(&incident_loaded);
			// Take the target lock before touching any incident row, same
			// order as the open path, so an operator resolve can't land inside
			// another transaction's join decision and make it open a fresh
			// incident. An incident's target never changes, so reading it
			// before the lock is safe.
			lock_target(conn, target).await?;

			let open_issue_ids: Vec<Uuid> = incident_issues::table
				.select(incident_issues::issue_id)
				.filter(
					incident_issues::incident_id
						.eq(incident_id)
						.and(incident_issues::left_at.is_null()),
				)
				.load(conn)
				.await?;

			for issue_id in open_issue_ids {
				let existing: Issue = issues::table
					.select(Issue::as_select())
					.filter(issues::id.eq(issue_id))
					.first(conn)
					.await?;
				if existing.resolved_at.is_some() {
					// Already resolved by the operator earlier; leave the
					// reason/by intact so the audit trail tells the truth.
					continue;
				}
				let issue = diesel::update(issues::table.filter(issues::id.eq(issue_id)))
					.set((
						issues::resolved_at.eq(jiff_diesel::Timestamp::from(now)),
						issues::resolved_by.eq(Some(by)),
						issues::resolved_reason.eq(Some(reason.to_string())),
					))
					.returning(Issue::as_select())
					.get_result(conn)
					.await?;
				// The issue is now resolved so the leave path fires
				// regardless of `monitored`; pass the live value
				// anyway so the function never sees stale state. Group-scoped
				// issues (application_id = None) bypass the per-server gate.
				let monitored = match issue.application_id {
					Some(sid) => Application::get_by_id(conn, sid).await?.is_monitored,
					None => true,
				};
				re_evaluate_incident_membership(conn, &issue, target, monitored, now, Some(by))
					.await?;
			}

			let incident: Incident =
				diesel::update(incidents::table.filter(incidents::id.eq(incident_id)))
					.set((
						incidents::resolved_at.eq(jiff_diesel::Timestamp::from(now)),
						incidents::resolved_by.eq(Some(by)),
						incidents::resolved_reason.eq(Some(reason.to_string())),
					))
					.returning(Incident::as_select())
					.get_result(conn)
					.await?;

			if incident.closed_at.is_some() {
				return Ok(incident);
			}
			let incident: Incident =
				diesel::update(incidents::table.filter(incidents::id.eq(incident_id)))
					.set(incidents::closed_at.eq(jiff_diesel::Timestamp::from(now)))
					.returning(Incident::as_select())
					.get_result(conn)
					.await?;
			// The per-issue resolves above drive most members out through the
			// leave arm, but one already carrying `resolved_at` is skipped to
			// keep its audit trail, and never leaves. Release whatever is left.
			release_remaining_members(conn, incident.id, now).await?;
			enqueue_slack_resolve(conn, &incident, by).await?;
			Ok(incident)
		})
		.await
	}

	pub async fn unresolve(db: &mut AsyncPgConnection, incident_id: Uuid) -> Result<Self> {
		use crate::schema::incidents;
		diesel::update(incidents::table.filter(incidents::id.eq(incident_id)))
			.set((
				incidents::resolved_at.eq(None::<jiff_diesel::Timestamp>),
				incidents::resolved_by.eq(None::<String>),
				incidents::resolved_reason.eq(None::<String>),
			))
			.returning(Self::as_select())
			.get_result(db)
			.await
			.map_err(AppError::from)
	}
}
