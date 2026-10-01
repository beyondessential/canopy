# Re-notify Slack after 24 hours open incident

## Slack workflow contract

Reminders reuse the open workflow (`SLACK_WEBHOOK_OPEN_URL`), so no new webhook is configured.
The open workflow's variables are repurposed for the summary style:

- `server`: the incident target's label (`format_group_label` with no application), or "Canopy"
- `severity`: worst live result: Critical / Error / Warning
- `source_ref`: the per-result counts, e.g. "2 failed, 1 warning"
- `message`: for a reminder, an "Open for N days" lead line, then one line per live issue (result, `description` falling back to the check, location), capped under `MAX_MESSAGE_LEN` and ending with "and N more"
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
