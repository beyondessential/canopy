---
id: TRI
---

# Alert triage

Applies to the triage Canopy serves an agent for each incident notification: what Canopy decides must be escalated, what it leaves to the agent's judgement, how verdicts are recorded, and the guidance it gives for reporting them.

## How an alert reaches triage

Canopy's [incident notifications](incidents.md#notification) and [maintenance notifications](maintenance.md#notification) carry a mention of the triaging agent in their layout, so the agent triages each one as it arrives, in a thread under it.
The channel those notifications post to is the **alerts channel**, and the channel the agent reports escalations and the daily write-up in is the **triage channel**.
The agent reads the incident's identifier from the notification's link to it, and triages through the [fleet query interface](../private-server/mcp.md#alert-triage).

> [!NOTE]
> The triaging agent is Claude Tag, Anthropic's Claude agent in Slack, which acts on a bot's message only when the message mentions it. The link in a notification points at the operator interface on the tailnet, which the agent cannot reach, so the agent reads the identifier from it rather than following it. The rules are served by Canopy rather than written into the agent's channel configuration, so they change with Canopy and can lean on what only Canopy can compute.

## What is triaged

An open incident is triaged against Canopy's escalation rules below.
An alert for an incident that has closed or been resolved is logged, and reported in the incident's escalation thread when it has one.
A maintenance notification links no incident and is logged without a verdict.

An incident's latest verdict is its **baseline**: its worst live result and its failing checks, each marked escalating or not, as they stood when that verdict was recorded.
An incident with no verdict has no baseline, and its triage is its **first triage**.
An incident whose state matches its baseline has already been triaged at that state, and triage returns the existing verdict and where it was reported, so a repeated alert adds nothing.

Results are ranked, most severe first, as an escalating failure, a failure, then a warning, a broken check counting as a warning (see [CHK](checks.md#results)).
A check is failing when its effective result is a live failure, and two checks are the same check when they share a source and a presented name (see [CHK](checks.md#names)).
Two failures are **close in time** when they began within thirty minutes of each other.

## What Canopy decides must be escalated

An open incident must be escalated when any of the following holds:

- It is a first triage, and the incident is on a production environment or on Canopy as a whole.
- An escalating check is failing that was not failing as an escalating check at the baseline.
- Its worst live result is more severe than at the baseline.
- A check is failing that was not failing at the baseline.
- A check failing in it is also failing in an open incident on another target, the two failures close in time.
- A check failing in it is unfamiliar: it has no documentation, no current incident memory entry is linked to it, and it has been part of no other published incident across the fleet.

Canopy gives each reason with what it rests on, such as the other incident sharing the check.
An environment is production when its rank is production, so an incident carrying a group's own checks is production when the group's headline rank is (see [INC](incidents.md#targets)).

> [!NOTE]
> Triage leans towards escalating. A false escalation costs a person a minute; a swallowed outage at a clinical site costs far more. Canopy decides the clear cases so that the agent's judgement can add escalations but cannot remove one Canopy requires, outside the harmless-pattern exception.

## What the agent judges

Canopy offers as candidates the incidents on other targets whose failures are close in time to this one's without sharing a check, and the agent judges whether they share a cause, such as a common release, cluster, or upstream service.
A candidate the agent judges to share a cause is escalated with this incident.
The agent escalates any failure it cannot explain with confidence from what it read, and says that the escalation is out of uncertainty.

### The harmless-pattern exception

A first triage whose only reason is a production opening is logged rather than escalated when every failing check has a current incident memory entry linked to it that names the pattern as harmless, and each check's stability record shows that pattern recovering on its own.
The verdict names the entries it relies on.
Canopy holds the verdict to entries that are current and linked to each failing check, and the agent judges whether they name the pattern as harmless and whether the stability records bear that out.
A first triage of an incident on Canopy as a whole is always escalated.

## Recording a verdict

A verdict is recorded before it is reported.
It holds whether the incident is escalated or logged, the agent's reasoning, the revision of the incident the triage read, and any memory entries relied on.
It is refused when the incident has changed since the revision read, so the agent triages again from the incident as it stands.
A logged verdict is refused, naming the reasons, when Canopy has decided the incident must be escalated and the harmless-pattern exception does not answer them.
Once the verdict is reported, where it was reported is attached to it.

A verdict records the incident's baseline as of the revision read.
It shows on the incident as a note by the agent, and the verdict is kept apart from that note, so editing or deleting the note leaves the verdict as recorded.
Recording a verdict and attaching where it was reported are audited actions, and the note is audited as adding a note (see [AUD](../platform/audit-log.md)).

## Related escalations

Triage returns, for each incident related to this one by a shared check or by a candidate the agent judges to share a cause, its latest verdict and where that was reported.
An incident related to one already escalated is escalated in that escalation's thread, so related trouble is one escalation naming every environment involved.
Recording a verdict returns any related incident escalated since the triage read, and the agent links the two escalations.

## Reporting

The guidance Canopy serves with each triage directs how the agent reports it.

An escalation is a top-level post in the triage channel giving, in plain language, what is wrong and where by group and environment, the incident's worst live result, why it is escalated, what to look at first, the related incidents, and a link to the incident.
An escalated incident that worsens is escalated again in a new post linking the earlier one, and that holds when the same alert is also a reminder.
Other news on an escalated incident, such as a reminder that changes nothing or its close, is a reply in its escalation's thread.

Every alert, escalated or logged, adds one reply to the day's write-up thread naming the incident's group and environment, what happened, and how it was triaged and why, with an escalated alert's reply linking its escalation.
The agent replies under the notification itself with one line saying whether it was escalated or logged and linking where.

When an operator confirms a cause or says that a pattern is harmless, in an escalation's thread or the write-up thread, the agent records it in the [incident memory](incident-memory.md), linked to the check, group, or incident it concerns.
A memory entry it records describes the pattern and how to recognise it, in the agent's own words.

## The daily write-up

A write-up day begins at 8am New Zealand time and ends at the next, so overnight alerts fall in the day the morning summary covers.
Each write-up day opens with a top-level post in the triage channel summarising the day before: how many alerts were escalated and how many logged, which incidents are still open, and the published incidents of that day that have no verdict.
An alert in a write-up day whose thread has not been started starts it.
Canopy serves the verdicts recorded in a write-up day, and the published incidents in it with no verdict, so the summary is drawn from Canopy's record rather than from the channel.

## Untriaged incidents

A published incident still open an hour after it notified, with no verdict, raises a [self-alert](../private-server/self-alerts.md) while alert triage is in use.
Alert triage is in use while any verdict has been recorded in the past week.
