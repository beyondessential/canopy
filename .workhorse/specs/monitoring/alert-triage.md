---
id: TRI
---

# Alert triage

Applies to how an agent triages the incident notifications Canopy posts to Slack: the context and guidance Canopy serves for each, how verdicts are recorded, and the daily write-up.

## How an alert reaches triage

Canopy's [incident notifications](incidents.md#notification) and [maintenance notifications](maintenance.md#notification) carry a mention of the triaging agent in their layout, so the agent triages each one as it arrives, in a thread under it.
The channel those notifications post to is the **alerts channel**, and the channel the agent reports escalations and the daily write-up in is the **triage channel**.
The agent reads the incident's identifier from the notification's link to it, and triages through the [fleet query interface](../private-server/mcp.md#alert-triage).

> [!NOTE]
> The triaging agent is Claude Tag, Anthropic's Claude agent in Slack, which acts on a bot's message only when the message mentions it. The link in a notification points at the operator interface on the tailnet, which the agent cannot reach, so the agent reads the identifier from it rather than following it.

## What triage reads

Triage of an incident reads that incident and what bears on it:

- the incident, its target, and its live issues with their results and whether each escalates
- each failing check's documentation and stability record
- the current incident memory entries linked to a failing check, the incident's group, or the incident
- the incident's notes and its earlier verdicts
- what has changed since the incident's latest verdict: its worst live result then and now, and the checks newly failing, newly escalating, and no longer failing

Results are ranked, most severe first, as an escalating failure, a failure, then a warning, a broken check counting as a warning (see [CHK](checks.md#results)).
A check is failing when its effective result is a live failure, and two checks are the same check when they share a source and a name, the identity an [incident memory](incident-memory.md#what-an-entry-holds) entry links a check by.

Where what is served does not explain a failure, the agent reads further about the incident's own group, machines and applications through the fleet query interface before deciding.

An incident whose live issues and results are unchanged since its latest verdict has already been triaged at that state, and triage returns that verdict and where it was reported, so a repeated alert adds nothing.

## How the agent decides

The agent decides whether each alert is escalated or logged, weighing the incident as a whole.
Canopy serves the guidance it decides by with every triage, and the guidance is kept in Canopy's codebase, so it changes with Canopy and is reviewed as Canopy is.

The guidance sets out what bears on the decision rather than fixed rules:

- the environment's rank, production and Canopy as a whole weighing most
- the incident's worst live result, and whether a failing check escalates, which operators reserve for failures needing a person at once
- what has changed since the incident was last triaged, such as a worse result or a newly failing check
- whether the check's documentation, its stability record, the memory, and the notes explain the failure
- a memory entry naming the pattern as harmless, borne out by the check's stability record

An alert for an incident that has closed or been resolved is logged, and reported in the incident's escalation thread when it has one.
A maintenance notification links no incident and is logged without a verdict.

> [!NOTE]
> Triage leans towards escalating. A false escalation costs a person a minute; a swallowed outage at a clinical site costs far more. The agent escalates what it cannot explain with confidence, and says when an escalation is out of uncertainty, so the reader knows which are judgement and which are clear.

## Recording a verdict

A verdict is recorded before it is reported.
It holds whether the incident is escalated or logged, the agent's reasoning, the revision of the incident the triage read, and any memory entries relied on.
It is refused when the incident has changed since the revision read, so the agent triages again from the incident as it stands.
Once the verdict is reported, where it was reported is attached to it.

A verdict records the incident's state as of the revision read, which is what the next triage's changes are measured from.
It shows on the incident as a note by the agent, and the verdict is kept apart from that note, so editing or deleting the note leaves the verdict as recorded.
Recording a verdict and attaching where it was reported are audited actions, and the note is audited as adding a note (see [AUD](../platform/audit-log.md)).

## Reporting

The guidance Canopy serves with each triage directs how the agent reports it.

An escalation is a top-level post in the triage channel giving, in plain language, what is wrong and where by group and environment, the incident's worst live result, why it is escalated, and what to look at first, with a link to the incident.
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
