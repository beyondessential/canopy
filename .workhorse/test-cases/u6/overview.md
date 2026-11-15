# Alert triage test cases

Scenarios that verify Canopy's side of alert triage, and a manual run of the Slack side end to end.

## Canopy

- [x] The guidance tool reads only and returns the guidance kept in the codebase (verifies spec: MCP)
- [x] The interface's instructions point an agent at the guidance before triaging (verifies spec: MCP)

## Manual, in Slack

- [ ] An incident opening in the alerts channel gets a one-line reply from Claude, and a triage note in the guidance's shape appears on the incident (verifies spec: TRI)
- [ ] A production failure Claude cannot explain is escalated as a top-level post in the triage channel, with the incident link (verifies spec: TRI)
- [ ] A test environment opening with an explained cause is logged as a reply in the day's write-up thread (verifies spec: TRI)
- [ ] A reminder for an unchanged incident adds nothing new beyond a link to the earlier report (verifies spec: TRI)
- [ ] A reminder after a check newly fails on an escalated incident gets a new escalation post linking the earlier one (verifies spec: TRI)
- [ ] The 8am routine posts the day's thread with yesterday's counts, open incidents, and any published incident with no triage note (verifies spec: TRI)
- [ ] An operator saying "this is harmless" in an escalation thread produces a memory entry linked to the check (verifies spec: TRI)
