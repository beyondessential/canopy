# Re-notify Slack after 24 hours open incident

## Summary-style notifications

- [x] An opening sent after its grace lists the issues that joined during it, not only the one that opened the incident (verifies spec: INC)
- [x] An issue that left before the opening was sent is not listed (verifies spec: INC)
- [x] The summary names the target alone (environment or group), with the application each issue is on in its line (verifies spec: INC)
- [x] Issues are listed worst result first, newest first among equals, with ungraded issues last (verifies spec: INC)
- [x] A line reads the issue's headline, or its check where it has none (verifies spec: INC)
- [x] A group- or Canopy-scoped issue's line carries no location (verifies spec: INC)
- [x] At most five issues are listed, followed by a count of the rest; counts cover every live issue (verifies spec: INC)
- [x] Severity is the worst live result: Critical when a live failure escalates, else Error, else Warning (verifies spec: INC)
- [x] An escalation summarises the incident at Critical (verifies spec: INC)
- [x] The drainer posts the summary rendered at claim time and records it on the row (verifies spec: INC)

## Reminders

- [x] A notified incident open a day is reminded, once per day however often the sweep runs (verifies spec: INC)
- [x] A reminder leads with how long the incident has been open (verifies spec: INC)
- [x] Days missed while the sweep wasn't running catch up with one reminder, not a backlog (verifies spec: INC)
- [x] An incident whose opening was never delivered is not reminded (verifies spec: INC)
- [x] A reminder that falls due while the incident lingers is held, and ships once a failure returns (verifies spec: INC)
- [x] Closing the incident cancels a held reminder, and the resolve is still sent (verifies spec: INC)
- [x] A reminder posts to the open workflow's webhook
- [ ] In a real Slack channel, the reworked open workflow reads well for an opening, an escalation, and a reminder, with `source_ref` relabelled for the counts
