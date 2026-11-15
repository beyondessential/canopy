# Alert triage with Claude Tag

Claude makes every triage decision and records it as an ordinary note on the incident. Canopy's only addition is the guidance Claude decides by, kept as markdown in `crates/canopy-mcp/src/alert_triage_guidance.md` and served by one read-only tool (TRI, MCP "Alert triage"). Claude Tag reads the incident through the existing tools from the thread under each alert. Its channel configuration holds only the Slack side: the workflow mentions, which channels are which, and the 8am routine.

## Shape

### Canopy (this repo)

- **One tool** in `crates/canopy-mcp`: `get_alert_triage_guidance`, read-only, returning the guidance markdown. The server instructions point agents at it before triaging an alert.
- **Everything else is existing tools.** Claude reads with `get_incident` (issues and notes), `get_check_documentation`, `get_check_stability` and `find_memory`, records with `add_incident_note`, and learns with `record_memory_entry`. Writing needs a token with the notes grant.
- **The verdict is a note** in a fixed shape: verdict and report link, then worst result and failing checks, then reasoning and memory relied on. The next triage compares against the latest such note to spot a repeat, so no revision digest or stored state is needed.
- **No verdict store, audit actions, write-up day logic or self-alert.** An earlier version had all of these, and before that escalation rules enforced in code. Both were dropped: triage weighs too many factors for fixed rules, comparing across environments is rarely meaningful, and the notes system already holds, shows and audits what the agent writes. The morning write-up names published incidents with no triage note, which covers what the self-alert did.

### Claude Tag (configuration, documented in `plugins/canopy-mcp/README.md`)

- Add an `@Claude` mention, picked from the message editor's autocomplete, to the incident open, incident resolve and both maintenance workflows. Claude reads the incident id from the **Open in Canopy** button.
- Alerts channel instructions: name the triage channel. For each alert, take the incident id from the button's URL (never fetch it), read the guidance, and follow it.
- Triage channel instructions: answer in threads, record memory from operator confirmations.
- The 8am Pacific/Auckland routine in the triage channel, which opens the day's thread as the guidance describes.
- A token with the notes grant in the bundle's credential for `meta.tamanu.app`.

## Spike results (2026-11-13)

All three unknowns passed in scratch channels, so the design stands with no fallback:
- Claude Tag replies to a Workflow Builder message that mentions it, using the `@` autocomplete in the message editor. **Respond automatically** must be on in the alerts channel.
- From the thread under that message, Claude posted at the top level of the other public channel and replied inside an existing thread there.
- Claude read the right incident id from the **Open in Canopy** button's URL alone. That URL (`https://canopy.tail53aef.ts.net/incidents/<id>`) is on the tailnet, so Claude takes the id from it and never fetches it.

## Remaining risks

- **Health data.** Claude Tag isn't covered by Anthropic's BAA. Check messages are free text, so confirm none carries patient data. Memory entries never hold raw output (TRI).
- **Spend.** Each alert costs one session. Set a spend limit on the bundle and watch the first week.
- **Repeat detection rests on the note's shape.** If Claude drifts from the format, repeats may be reported twice. Watch for it in the first week and tighten the guidance if so.

## Checklist

- [x] Spike in scratch channels: workflow mention, cross-channel post and thread reply, button URL visibility
- [x] `get_alert_triage_guidance` tool and server instructions paragraph
- [x] Guidance written around existing tools and incident notes
- [x] README in `plugins/canopy-mcp/` covering the workflow edits, both channels' instructions, the routine message and the notes-grant token
- [ ] Trial: have Claude triage a week of past published incidents with the guidance and compare its verdicts with what operators did
