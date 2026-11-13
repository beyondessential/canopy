# Alert triage with Claude Tag

Delivered as a Claude plugin in this repo, attached to the Claude Tag bundle for the two channels, plus Slack and Claude Tag configuration. There are no Canopy code changes.

## Shape

- A new plugin `plugins/canopy-alert-triage/` with two skills:
  - **triage-canopy-alert**: runs in the thread under each alert. It reads the incident over MCP, makes the assessment, posts the escalation or the write-up reply, replies in the alert's thread, and writes the incident note.
  - **daily-alert-write-up**: run by a routine at 8am Pacific/Auckland. It summarises the previous write-up day and opens the new thread.
- The plugin depends on the `canopy-mcp` plugin for fleet access. Both attach to the same bundle.
- The **incident note is the state store.** Each triage note carries a fixed machine-readable first line, so a later triage can read the last verdict, the worst result at the time, and the escalation post's permalink. An example of the line: `triage: escalated | worst=error | post=<permalink>`. That way, deciding whether an incident is worse needs nothing from Slack.
- Finding the day's thread: the opening post has a fixed title, `Alert triage · <weekday> <date> (NZ)`. The triage skill finds the newest such post by Claude in the triage channel, and starts one itself when the newest is more than a day old.

## Risks to settle before building

- **Cross-channel posting from a reply-triggered session.** The Claude Tag docs allow a *routine* to post into another public channel. They don't say whether a thread session started by a bot's `@Claude` can post top-level into another channel, or reply in a thread there. Spike this first with a test workflow in a scratch pair of channels.
  - If it can't, the fallback is to run triage from an hourly routine in the triage channel that sweeps the alerts channel, which loses immediacy.
  - The other fallback is a second Canopy webhook that posts each alert to the triage channel too.
- **Workflow mentions.** Confirm that a Workflow Builder message can mention the Claude app so that it reaches Claude as a real `@Claude`. Claude Tag replies to a bot's mention unless the channel's **Respond automatically** setting is off.
- **Finding the incident from the post.** The open workflow's message reads as follows:
  - a `:deciduous_tree::rotating_light: <server>` line
  - `Severity:`
  - `Source:`, which carries the counts
  - the issue list
  - an **Open in Canopy** button linking to `https://canopy.tail53aef.ts.net/incidents/<id>`

  That link is on the tailnet, which Claude Tag can't reach. The skill takes the incident id from the URL and reads the incident through the public MCP mount, never by fetching the link. The open question is whether Claude Tag sees a button's URL at all, or only the message text.
  - Test it in the spike by asking Claude for the incident id.
  - If it can't see the URL, put the link (or the bare id) in the message text too, beside the `@Claude` mention.
  - The last resort is matching `server` against Find incidents, which is ambiguous when a group has more than one open incident across its environments.
- **Health data.** Claude Tag isn't covered by Anthropic's BAA. Canopy alerts are infrastructure, but check messages are free text. Confirm no check puts patient data in its message, and keep raw output out of memory entries (TRI already says so).
- **Spend.** Triage costs one session per alert, which on a noisy day could be dozens. Set a spend limit on the bundle and watch the first week.

## Checklist

- [ ] Spike: cross-channel post and workflow `@Claude` mention, in scratch channels
- [ ] Mint a Canopy MCP token named for the triage with the **notes grant**, and swap it in as the bundle's credential for `meta.tamanu.app`
- [ ] Write `plugins/canopy-alert-triage/` (plugin.json, two skills) and add it to `.claude-plugin/marketplace.json`
- [ ] Encode TRI's escalation rules in the triage skill, including the uncertainty rule and the harmless-pattern exception
- [ ] Write the plugin README with setup steps:
  - create the triage channel (public) and invite Claude
  - add `@Claude` to the incident open, incident resolve, maintenance declared and maintenance ended workflows
  - attach both plugins to a bundle scoped to the two channels
  - set **Respond automatically** on in the alerts channel
  - post the routine message in the triage channel
- [ ] Update the `canopy-mcp` README: the token is no longer read-only when the notes grant is used
- [ ] Trial run: replay a week of past incidents (Find incidents) through the triage skill by hand and compare its verdicts with what operators did
