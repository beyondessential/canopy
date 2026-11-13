# canopy-mcp plugin

Points agents at the internet-facing mount of Canopy's MCP fleet query
interface (`https://meta.tamanu.app/mcp`). The `.mcp.json` carries no
credentials: the endpoint requires a bearer token, which must be supplied by
the agent platform (for Claude Tag, the admin Connections credential proxy).

Setup for Claude Tag (admin, at claude.ai/admin-settings/claude-tag):

1. Mint a token in the Canopy operator UI: Settings → MCP access. The
   `canopy_mcp_…` secret is shown once. Tokens expire after one year; a
   rotation alert fires 15 days out.
2. In the Access bundle's Credentials tab, use **Connect another app**:
   credential type **Bearer**, paste the secret, allowed website
   `meta.tamanu.app`.
3. Add this repository as an organization plugin source and attach the
   `canopy-mcp` plugin to the bundle.
4. Verify by asking Claude to run the `fleet_summary` tool; the token's
   "Last used" column in Canopy's Settings → MCP access updates within a
   minute.

Operators on the tailnet don't need this plugin or a token — use the tailnet
mount instead: `claude mcp add --transport http canopy
https://canopy.tail53aef.ts.net/api/mcp`.

## Alert triage in Slack

Claude Tag triages each incident notification Canopy posts to the alerts
channel: it escalates what needs a person now to a separate triage channel,
and logs the rest in a daily write-up thread there. The rules live in Canopy
(spec `TRI`, `.workhorse/specs/monitoring/alert-triage.md`) and are served by
the `triage_alert` tool, so the Slack side only says where things go.

Setup:

1. **Token.** Mint a token in Settings → MCP access with the **notes** grant
   (recording a verdict writes a note on the incident), and use it as the
   bundle's `meta.tamanu.app` credential in place of a read-only one.
2. **Channels.** Create the triage channel as a **public** channel; the alerts
   channel must be public too, since Claude Tag only posts across public
   channels. Invite Claude to both (`/invite @Claude`), and attach the bundle
   carrying this plugin to both. In the alerts channel, keep **Respond
   automatically** on: with it off, Claude ignores a workflow's mention.
3. **Workflows.** In Slack's Workflow Builder, add an `@Claude` mention (picked
   from the editor's autocomplete) to the message of each Canopy workflow:
   incident open, incident resolve, maintenance declared and maintenance
   ended. Claude reads the incident id from the **Open in Canopy** button.
4. **Alerts channel instructions** (`@Claude !configure` → Channel
   instructions):

   > You triage the Canopy alerts posted here. For each incident
   > notification, read the incident id from the UUID at the end of its Open
   > in Canopy link (never fetch the link; it is on a private network), call
   > the canopy `triage_alert` tool, and follow its `guidance` exactly.
   > Escalations and the daily write-up go in #TRIAGE-CHANNEL. A maintenance
   > notification is logged in the write-up without calling the tool.

5. **Triage channel instructions:**

   > This channel holds Canopy alert escalations and one write-up thread per
   > day, which you post while triaging alerts from #ALERTS-CHANNEL. Answer
   > people in threads, never at the top level. The day's write-up thread is
   > the newest top-level post titled "Alert triage · <weekday> <d Mon> (NZ)".
   > When an operator confirms a cause or says a pattern is harmless, record
   > it with `record_memory_entry` as the triage guidance describes.

6. **Routine.** In the triage channel, post once:

   > @Claude every day at 8am Pacific/Auckland, call the canopy
   > `find_alert_triage_verdicts` tool for the write-up day that just ended,
   > and post a new top-level message titled "Alert triage · <weekday>
   > <d Mon> (NZ)" for today, summarising yesterday: how many alerts were
   > escalated and how many logged, which incidents are still open (with
   > links), and any published incidents that were never triaged.

Canopy raises a self-alert when a published incident stays open an hour past
its notification with no verdict, while triage is in use (any verdict in the
past week), so a triage that stops working is noticed.
