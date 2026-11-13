# Alert triage with Claude Tag

The triage rules live in Canopy, served through two new MCP tools. Claude Tag calls them from the thread under each alert. Its channel instructions keep only the Slack side: which channels, the post layout and the write-up thread.

## Shape

### Canopy (this repo)

- **`triage_alert(incident_id)`** in `crates/canopy-mcp`. It gathers, in one call:
  - the incident, its live issues and its notes
  - its triage verdicts so far
  - each failing check's documentation, stability record and linked memory entries
  - the related-incident candidates

  It computes the must-escalate reasons in TRI's "What Canopy decides" list, and returns them with the judgement rules and post contents as text.
- **`record_alert_triage(...)`** stores the verdict and the baseline snapshot (worst result and failing checks), and adds the reasoning as an agent note.
  - It refuses a logged verdict against must-escalate reasons, except the production-opening reason backed by a linked memory entry.
  - It's held by the notes grant, and is a new audit action.
- **Storage:** a new table of triage verdicts, one row per verdict, keyed to the incident. Each row holds the verdict, worst result, failing check set (source and check), post link, memory entry used, the note it wrote, the caller and the time. Create it with `just migration`, never by hand.
  - A structured row rather than parsing a note's first line: the comparison with the last triage must not depend on free text.
- **Mechanical rules**, all decided in the database crate so they can be tested without MCP:
  - First triage of a production or Canopy-wide incident: no earlier verdict rows, and the target is production (`Machine::environment_rank`, never the raw `rank`) or Canopy.
  - Escalating failure newly live: a live member with an effective failure and escalates set, whose check wasn't in the last snapshot as escalating.
  - Worse: the current worst live result outranks the snapshot's.
  - New failing check: a check failing now that wasn't in the snapshot set.
  - Shared check: an open incident on another target with a live failure on the same source and check.
  - Unfamiliar: no documentation, no memory entry linked to the check, and no earlier published incident containing it.
- **Shared-cause candidates:** incidents on other targets opened within 30 minutes of this one. Claude judges these; they're never a must-escalate.
- **Server instructions** (`lib.rs`) get a paragraph pointing agents at `triage_alert` for Canopy incident notifications.

### Claude Tag (configuration, documented in a README)

- Add an `@Claude` mention, picked from the message editor's autocomplete, to the incident open, incident resolve and both maintenance workflows. Claude reads the incident id from the **Open in Canopy** button, so no written-out link is needed.
- Alerts channel instructions: take the incident id from the Open in Canopy button's URL (never fetch it), call `triage_alert`, follow it, post, call `record_alert_triage`, then reply with one line.
- Triage channel instructions: the escalation thread behaviour, memory recording and the daily write-up.
- The 8am Pacific/Auckland routine in the triage channel.
- A token with the notes grant in the bundle's credential for `meta.tamanu.app`.

## Spike results (2026-11-13)

All three unknowns passed in scratch channels, so the design stands with no fallback:
- Claude Tag replies to a Workflow Builder message that mentions it, using the `@` autocomplete in the message editor. **Respond automatically** must be on in the alerts channel.
- From the thread under that message, Claude posted at the top level of the other public channel and replied inside an existing thread there.
- Claude read the right incident id from the **Open in Canopy** button's URL alone. That URL (`https://canopy.tail53aef.ts.net/incidents/<id>`) is on the tailnet, so Claude takes the id from it and never fetches it.

## Remaining risks

- **Health data.** Claude Tag isn't covered by Anthropic's BAA. Check messages are free text, so confirm none carries patient data. Memory entries never hold raw output (TRI).
- **Spend.** Each alert costs one session. Set a spend limit on the bundle and watch the first week.

## Checklist

- [x] Spike in scratch channels: workflow mention, cross-channel post and thread reply, button URL visibility
- [ ] Migration and model for triage verdicts in `crates/database`
- [ ] Must-escalate reasons and shared-cause candidates in `crates/database`, with tests covering each reason, the baseline comparison across two triages, and a machine and application seeded under one id
- [ ] `triage_alert` and `record_alert_triage` in `crates/canopy-mcp`, with the refusal rules and the notes-grant check
- [ ] New audit action in the `actions!` catalogue for recording a verdict
- [ ] Server instructions paragraph, and MCP tests in `crates/private-server/tests/it/mcp.rs`
- [ ] Show the verdict notes on the incident page (they're agent notes, so they may need nothing new; confirm)
- [ ] README in `plugins/canopy-mcp/` covering:
  - the workflow edits
  - both channels' instructions
  - the routine message
  - the notes-grant token
- [ ] Trial: run `triage_alert` over a week of past published incidents and compare its verdicts with what operators did
