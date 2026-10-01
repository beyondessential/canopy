# Re-notify Slack after 24 hours open incident

## Slack workflow contract

Reminders reuse the open workflow (`SLACK_WEBHOOK_OPEN_URL`), so no new webhook is configured.
The open workflow's variables are repurposed for the summary style:

- `server`: the incident target's label (`format_group_label` with no application), or "Canopy"
- `severity`: worst live result: Critical / Error / Warning
- `source_ref`: the per-result counts, e.g. "2 failed, 1 warning"
- `message`: for a reminder, an "Open for N days" lead line, then one line per live issue (result, `description` falling back to the check, location), at most five, then "and N more"
- `link`: unchanged, injected by the drainer

The variable names stay the same, so the deployed workflow keeps working.
The Slack workflow's label for `source_ref` needs renaming on the Slack side when this ships.

## Render at delivery time

Currently `enqueue_slack_open` renders the payload in the opening transaction, so issues that join during the grace are lost.
Summary-style rows (open, escalation, reminder) have to be rendered when the drainer claims them, from the incident's current membership.
The drainer already has a DB connection inside the claim transaction, so rendering there is straightforward.
The module docs in `slack_outbox/mod.rs` that justify enqueue-time rendering need rewriting to match.

## Reminder rows

- Use a new outbox kind (e.g. `incident_reminder`) routed to the open URL, not `incident_open`. `cancel_pending_open`, `delivered_open_ids` and `pending_opens_until` all key off `incident_open`, and a reminder must not count as the open or cancel the resolve.
- Scheduling: a monitor sweep enqueues a reminder for each open, notified incident that has crossed its next whole day since `opened_at`. A per-incident counter (or a last-reminded stamp) makes it idempotent across ticks and restarts.
- `claim_pending`'s lingering exclusion extends to reminder rows. Closing the incident cancels any pending reminder, but the resolve still ships because the open was delivered.

## Build steps

- [x] Migration: `incidents.reminders_sent` (whole days already reminded for), default 0
- [x] `slack_outbox::summary`: render the summary payload for an incident from its live members (target label, worst severity, counts, capped line list, reminder lead)
- [x] `format_group_label` names the target only; `enqueue_slack_open` uses the summary renderer for its snapshot
- [x] `KIND_INCIDENT_REMINDER`; `claim_pending` holds it back while lingering; closing an incident cancels pending reminders
- [x] `enqueue_due_reminders` sweep, run from the monitor loop
- [x] Drainer re-renders open and reminder rows at delivery, stores what it sent, routes reminders to the open webhook
- [x] Update the existing payload tests; add database tests for summary, reminders, cancellation; drainer unit tests
- [x] Test-cases file
