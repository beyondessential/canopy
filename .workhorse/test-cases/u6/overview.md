# Alert triage test cases

Scenarios that verify Canopy's side of alert triage, and a manual run of the Slack side end to end.

## What Canopy decides

- [x] A first triage of a production incident requires escalation (production opening), and the harmless-pattern exception is available (verifies spec: TRI)
- [x] A first triage of a Canopy-wide incident requires escalation, and the exception is not available (verifies spec: TRI)
- [x] A first triage of a lesser environment with a documented failing check requires nothing (verifies spec: TRI)
- [x] An escalating failing check requires escalation (verifies spec: TRI)
- [x] An unfamiliar check (no documentation, no memory, no other published incident) requires escalation (verifies spec: TRI)
- [x] A closed incident and a resolved incident have no reasons (verifies spec: TRI)
- [x] After a verdict, a newly failing check, a newly escalating check and a worse worst result are each a reason against the baseline (verifies spec: TRI)
- [x] The same check failing on another target within thirty minutes requires escalation; one failing for days elsewhere is listed as shared but forces nothing (verifies spec: TRI)
- [x] A different check failing elsewhere close in time is offered as a shared-cause candidate (verifies spec: TRI)
- [ ] A check with documentation but no memory and no past incidents is familiar (documentation alone is enough)
- [ ] A group-scoped issue's incident on a group whose headline rank is production counts as production

## Recording verdicts

- [x] A repeat triage at an unchanged state returns the verdict already given and no reasons (verifies spec: TRI)
- [x] A verdict against a revision the incident has moved past is refused and records nothing (verifies spec: TRI)
- [x] A logged verdict against a required escalation is refused, naming the reasons (verifies spec: TRI)
- [x] The harmless-pattern exception needs a current entry linked to each failing check; a group entry or a retired entry does not count (verifies spec: TRI)
- [x] Recording returns related incidents escalated since the triage read (verifies spec: TRI)
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
- [ ] A production opening is escalated as a top-level post in the triage channel, with the incident link
- [ ] A test environment opening with an explained cause is logged as a reply in the day's write-up thread
- [ ] A reminder for an unchanged incident adds nothing new beyond a link to the earlier report
- [ ] Two sites failing the same check together become one escalation naming both
- [ ] The 8am routine posts the day's thread with yesterday's counts and open incidents
- [ ] An operator saying "this is harmless" in an escalation thread produces a memory entry linked to the check
