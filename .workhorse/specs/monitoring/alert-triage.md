---
id: TRI
---

# Alert triage

Applies to how an agent triages the incident notifications Canopy posts to Slack: the guidance Canopy serves for it, how a verdict is recorded on the incident, and the daily write-up.

## How an alert reaches triage

Canopy's [incident notifications](incidents.md#notification) and [maintenance notifications](maintenance.md#notification) carry a mention of the triaging agent in their layout, so the agent triages each one as it arrives, in a thread under it.
The channel those notifications post to is the **alerts channel**, and the channel the agent reports escalations and the daily write-up in is the **triage channel**.
The agent reads the incident's identifier from the notification's link to it, and triages through the [fleet query interface](../private-server/mcp.md#alert-triage).

> [!NOTE]
> The triaging agent is Claude Tag, Anthropic's Claude agent in Slack, which acts on a bot's message only when the message mentions it. The link in a notification points at the operator interface on the tailnet, which the agent cannot reach, so the agent reads the identifier from it rather than following it.

## What triage reads

The agent triages through the incident's own record: the incident, its live issues with their results and whether each escalates, and its notes; each failing check's documentation and stability record; and the [incident memory](incident-memory.md) linked to a failing check, the incident's group, or the incident.
Where that does not explain a failure, the agent reads further about the incident's own group, machines and applications before deciding.
It compares an incident only with others in the same group and environment.

## How the agent decides

The agent decides whether each alert is escalated or logged, weighing the incident as a whole.
Canopy serves the guidance it decides by through the [fleet query interface](../private-server/mcp.md#alert-triage), and the guidance is kept in Canopy's codebase, so it changes with Canopy and is reviewed as Canopy is.

The guidance sets out what bears on the decision rather than fixed rules:

- the environment's rank, production and Canopy as a whole weighing most
- the incident's worst live result, and whether a failing check escalates, which operators reserve for failures needing a person at once
- what has changed since the incident was last triaged, such as a worse result or a newly failing check
- whether the check's documentation, its stability record, the memory, and the notes explain the failure
- a memory entry naming the pattern as harmless, borne out by the check's stability record

An alert for an incident that has closed or been resolved is logged, and reported in the incident's escalation thread when it has one.
A maintenance notification links no incident and is logged in the write-up alone.

> [!NOTE]
> Triage leans towards escalating. A false escalation costs a person a minute; a swallowed outage at a clinical site costs far more. The agent escalates what it cannot explain with confidence, and says when an escalation is out of uncertainty, so the reader knows which are judgement and which are clear.

## Recording a verdict

A verdict is recorded as a note by the agent on the [incident](incidents.md#resolution), after it is reported.
The note opens with whether the incident was escalated or logged and a link to where it was reported, then the incident's worst live result and failing checks as triaged, then the reasoning and any memory entries relied on.

An incident whose latest triage note names the same failing checks at the same worst result has already been triaged at that state, so a repeated alert reports nothing new and the agent replies under it with a link to the earlier report.

> [!NOTE]
> Canopy keeps no triage record beyond the note. The note is where an operator looking at the incident already reads, and it is what the next triage compares against.

## Reporting

The guidance directs how the agent reports.

An escalation is a top-level post in the triage channel giving, in plain language, what is wrong and where by group and environment, the incident's worst live result, why it is escalated, and what to look at first, with a link to the incident.
An escalated incident that worsens is escalated again in a new post linking the earlier one, and that holds when the same alert is also a reminder.
Other news on an escalated incident, such as a reminder that changes nothing or its close, is a reply in its escalation's thread.

Every alert, escalated or logged, adds one reply to the day's write-up thread naming the incident's group and environment, what happened, and how it was triaged and why, with an escalated alert's reply linking its escalation.
The agent replies under the notification itself with one line saying whether it was escalated or logged and linking where.

When an operator confirms a cause or says that a pattern is harmless, in an escalation's thread or the write-up thread, the agent records it in the incident memory, linked to the check, group, or incident it concerns.
A memory entry it records describes the pattern and how to recognise it, in the agent's own words.

## The daily write-up

A write-up day begins at 8am New Zealand time and ends at the next, so overnight alerts fall in the day the morning summary covers.
Each write-up day opens with a top-level post in the triage channel summarising the day before: how many alerts were escalated and how many logged, which incidents are still open, and the published incidents of that day with no triage note.
An alert in a write-up day whose thread has not been started starts it.
