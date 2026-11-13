# Alert triage with Claude Tag

The triage rules live in Canopy, served through new MCP tools (TRI, MCP "Alert triage"). Claude Tag calls them from the thread under each alert. Its channel configuration holds only the Slack side: the workflow mentions, which channels are which, and the 8am routine. How an escalation or a write-up line reads is guidance Canopy serves.

## Shape

### Canopy (this repo)

- **Tools** in `crates/canopy-mcp`:
  - `triage_alert(incident_id)`: read-only. Returns everything in MCP's list: the incident, its revision, an already-triaged short-circuit, check context, related incidents with their verdicts, the must-escalate reasons, and the served guidance text.
  - `record_alert_triage(incident_id, revision, verdict, reasoning, memory_entry_ids)`: refuses on a stale revision or a disallowed "logged". Stores the verdict and baseline, adds the agent note, and returns related incidents escalated since the revision.
  - `attach_alert_triage_report(verdict_id, link)`.
  - `find_alert_triage_verdicts(day)`: the write-up day's verdicts and the published incidents with none.
  - Record and attach are held by the notes grant.
- **Revision:** a digest of the incident's live membership state, built from each live member's issue id, effective result and escalates flag, plus whether the incident is open. It's derived on read, so no column is needed. "Already triaged at this state" means the digest equals the latest verdict's.
- **Storage:** a new table of verdicts, one row per verdict, keyed to the incident. Each row holds:
  - the verdict, the reasoning and the revision
  - the worst result
  - the failing check set (source, presented name, escalating)
  - the memory entries relied on and the report link
  - the note it wrote (`ON DELETE SET NULL`, so deleting the note leaves the verdict), the caller and the time

  Created with `diesel migration generate` (what `just migration` runs; `just` isn't installed on this machine). `schema.rs` was edited to match, since generating it needs a migrated database.
- **Mechanical rules**, all in the database crate so they can be tested without MCP:
  - Each rule applies only to an open incident. A closed or resolved one returns no reasons.
  - First triage of a production or Canopy-wide incident: no earlier verdict, and the target is production (via the environment rank, `Machine::environment_rank`, never the raw `rank`) or Canopy.
  - Escalating failure newly live: a failing check with escalates set that wasn't escalating in the baseline set.
  - Worse: escalating failure > failure > warning, with broken counted as a warning, compared with the baseline's worst.
  - New failing check: in the live failing set, not in the baseline's.
  - Shared check: an open incident on another target failing the same source and presented name, where the two failures' degraded-since times are within 30 minutes.
  - Unfamiliar: no documentation, no current (unretired) memory entry linked to the check, and no *other* published incident fleet-wide that contained it.
  - Harmless-pattern check: the only reason is the production opening (not Canopy-wide), and every failing check has a current memory entry among those given that is linked to it.
- **Shared-cause candidates:** incidents on other targets with a failure that began within 30 minutes of this one's and that share no check with it.
- **Write-up day:** from 08:00 Pacific/Auckland to the next 08:00. Use jiff with the zone, never a fixed offset, so daylight saving is handled. The database crate bundles the tz database (`tzdb-bundle-always`) so this works in images without `/usr/share/zoneinfo`.
- **Self-alert:** a job condition listing published incidents still open an hour past their notification with no verdict. It's live only while any verdict exists from the past week. It warns, and clears once each has a verdict or has closed.
- **Audit:** two new actions, recording a verdict and attaching its report. The note is audited as the existing note action.
- **Server instructions** (`lib.rs`): a paragraph pointing agents at `triage_alert` for every incident notification.

### Claude Tag (configuration, documented in a README)

- Add an `@Claude` mention, picked from the message editor's autocomplete, to the incident open, incident resolve and both maintenance workflows. Claude reads the incident id from the **Open in Canopy** button.
- Alerts channel instructions: name the triage channel. For each alert, take the incident id from the button's URL (never fetch it), call `triage_alert`, and follow it.
- Triage channel instructions: answer in threads, record memory from operator confirmations, follow `triage_alert`'s guidance for the write-up.
- The 8am Pacific/Auckland routine in the triage channel, which opens the day's thread from `find_alert_triage_verdicts`.
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
- [x] Migration and model for triage verdicts in `crates/database`
- [x] Incident revision digest, baseline comparison, must-escalate reasons, harmless-pattern check and shared-cause candidates in `crates/database`. Tests to cover:
  - each reason
  - closed incidents
  - two triages in a row
  - a stale revision
  - a machine and application seeded under one id
- [x] Write-up day boundaries in Pacific/Auckland, tested across a daylight saving change
- [x] The four MCP tools in `crates/canopy-mcp`, with the refusal rules and the notes-grant check
- [x] Two audit actions in the `actions!` catalogue
- [x] Self-alert condition for untriaged published incidents
- [x] Server instructions paragraph and the served guidance text, plus MCP tests (`crates/public-server/tests/it/mcp_alert_triage.rs`, and the writing-tools list in `crates/private-server/tests/it/mcp.rs`)
- [x] Verdict notes on the incident page: they are ordinary agent notes, which the incident page already lists, so nothing new was needed
- [x] README in `plugins/canopy-mcp/` covering:
  - the workflow edits
  - both channels' instructions
  - the routine message
  - the notes-grant token
- [ ] Trial: run `triage_alert` over a week of past published incidents and compare its verdicts with what operators did
