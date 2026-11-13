# Alert triage test cases

Scenarios that verify Canopy's side of alert triage, and a manual run of the Slack side end to end.

## What triage reads

- [x] Triage reads the incident's target, failing checks with whether each escalates, their documentation, and the current memory linked to its checks (same source only), its group and the incident; retired entries and other groups' entries are left out (verifies spec: TRI)
- [x] Before any verdict there are no changes to read (verifies spec: TRI)
- [x] After a verdict, the worst result then and now, and the checks newly failing, newly escalating and no longer failing, are read as facts (verifies spec: TRI)
- [x] The guidance is served with every triage (verifies spec: MCP)
- [ ] Each live issue is presented with the grain it is filed at, a machine and its application sharing an id told apart (verifies spec: MCP)

## Recording verdicts

- [x] A repeat triage at an unchanged state returns the verdict already given (verifies spec: TRI)
- [x] Closing an incident is a new state, so the close is not mistaken for a repeat (verifies spec: TRI)
- [x] A verdict against a revision the incident has moved past is refused and records nothing (verifies spec: TRI)
- [x] A verdict records the memory entries it relies on, and an unknown entry is refused (verifies spec: TRI)
- [x] Recording adds an agent note to the incident, and attaching a report link is validated as an http(s) address (verifies spec: TRI)
- [x] Recording and attaching need the notes grant; reading does not (verifies spec: MCP)
- [x] Recording audits the verdict and the note; attaching audits the report (verifies spec: TRI)
- [ ] Deleting the verdict's note leaves the verdict as recorded (verifies spec: TRI)

## Write-up day and the self-alert

- [x] A write-up day runs from 8am to 8am New Zealand time, and the day daylight saving ends is an hour longer (verifies spec: TRI)
- [x] A moment before 8am in Auckland belongs to the day before (verifies spec: TRI)
- [x] A write-up day lists its verdicts and its published incidents with no verdict (verifies spec: TRI)
- [x] A published incident open an hour past its notification with no verdict raises the self-alert while triage is in use, and recovers once it has a verdict (verifies spec: SELF)
- [x] Nothing is raised while triage has never been used (verifies spec: TRI)

## Manual, in Slack

- [ ] An incident opening in the alerts channel gets a one-line reply from Claude, and a verdict note appears on the incident
- [ ] A production failure Claude cannot explain is escalated as a top-level post in the triage channel, with the incident link
- [ ] A test environment opening with an explained cause is logged as a reply in the day's write-up thread
- [ ] A reminder for an unchanged incident adds nothing new beyond a link to the earlier report
- [ ] The 8am routine posts the day's thread with yesterday's counts and open incidents
- [ ] An operator saying "this is harmless" in an escalation thread produces a memory entry linked to the check
