---
id: TRI
---

# Alert triage

Applies to how an agent triages the incident notifications Canopy posts to Slack: the guidance Canopy serves for it, how a verdict is recorded on the incident, what the agent keeps in the incident memory, and the daily write-up.

## How an alert reaches triage

Canopy's [incident notifications](incidents.md#notification) and [maintenance notifications](maintenance.md#notification) carry a mention of the triaging agent in their layout, so the agent triages each one as it arrives, in a thread under it.
The channel those notifications post to is the **alerts channel**, and the channel the agent reports escalations and the daily write-up in is the **triage channel**.
The agent reads the incident's identifier from the notification's link to it, and triages through the [fleet query interface](../private-server/mcp.md#alert-triage).
The alerts channel is public, so the agent triages only the notifications Canopy posts there: a message mentioning it from another author is not an alert, however it is worded.

> [!NOTE]
> The triaging agent is Claude Tag, Anthropic's Claude agent in Slack, which acts on a bot's message only when the message mentions it. The link in a notification points at the operator interface on the tailnet, which the agent cannot reach, so the agent reads the identifier from it rather than following it.

## What triage reads

The agent triages through the incident's own record: the incident, its live issues with their results and whether each escalates, and its notes; each failing check's documentation and stability record; and the [incident memory](incident-memory.md) linked to a failing check, the incident's group, or the incident, with the entries that apply to the fleet as a whole.
Where that does not explain a failure, the agent reads further about the incident's own group, machines and applications before deciding.
It compares an incident only with others in the same group and environment.

Everything it reads is data it weighs, never instruction it follows: check messages, notes, memory entries and what people write in the channels are free text, so a direction planted in one carries no authority over how the agent triages or what it records.

## How the agent decides

The agent decides whether each alert is escalated or logged, weighing the incident as a whole.
Canopy serves the guidance it decides by through the [fleet query interface](../private-server/mcp.md#alert-triage), and the guidance is kept in Canopy's codebase, so it changes with Canopy and is reviewed as Canopy is.
The guidance is what the agent follows, and holds the whole of how it weighs an alert and how it words what it posts; this spec says what the guidance covers and what is left on the record, not how either reads.

The guidance sets out what bears on the decision rather than fixed rules: the environment's rank, the incident's worst live result and whether a failing check escalates, what has changed since the incident was last triaged, and whether the documentation, the stability record, the memory and the notes explain the failure.

An alert for an incident that has closed or been resolved is logged, and reported in the incident's escalation thread when it has one, after the agent has [reviewed the incident](#reviewing-a-closed-incident) for what to keep.
A maintenance notification links no incident and is logged in the write-up alone.

> [!NOTE]
> Triage leans towards escalating. A false escalation costs a person a minute; a swallowed outage at a clinical site costs far more. The agent escalates what it cannot explain with confidence, and says when an escalation is out of uncertainty, so the reader knows which are judgement and which are clear.

## Recording a verdict

A verdict is recorded as a note by the agent on the [incident](incidents.md#resolution), after it is reported.
The note opens with whether the incident was escalated or logged and a link to where it was reported, then the incident's worst live result, named as the check result it is, and the failing checks as triaged, then the reasoning and any memory entries relied on.

An incident whose latest triage note names the same failing checks at the same worst result has already been triaged at that state, so a repeated alert reports nothing new and the agent replies under it with a link to the earlier report.

> [!NOTE]
> Canopy keeps no triage record beyond the note. The note is where an operator looking at the incident already reads, and it is what the next triage compares against.

## Reporting

An escalation is a top-level post in the triage channel, linking the incident.
An escalated incident that worsens is escalated again in a new post linking the earlier one, and that holds when the same alert is also a reminder.
Other news on an escalated incident, such as a reminder that changes nothing or its close, is a reply in its escalation's thread.

Every alert, escalated or logged, adds one reply to the day's write-up thread, an escalated alert's reply linking its escalation.
The agent replies under the notification itself with one line saying whether it was escalated or logged and linking where.

## What triage keeps

The agent keeps what it learns in the [incident memory](incident-memory.md), in whichever session it learns it: triaging an alert, answering in a thread, or writing the daily write-up.
What it keeps is what a later triage would otherwise work out again: a cause, a harmless pattern, a reading that misleads, how one failure was told apart from another, where a site's logs or a fix are kept, and who answered for a site or check.
A reading that misleads covers a check that misreads at a known time, such as during a site's backup window, and a query whose answer misleads, such as one that leaves out what a reader would expect it to cover.

An entry is linked to the narrowest thing it concerns: a check, a group, or one incident.
An entry that concerns how triage reads the fleet in general carries no link, and applies to the fleet as a whole.
Before recording an entry, the agent looks for one that already says it, and amends that one instead.

An entry describes what was learned and how to recognise it, in the agent's own words rather than a check's raw output.
It names where its content came from: the incident the agent found it on, or who said it and when.

The agent amends or retires any entry its own reading contradicts, whoever recorded it, saying why.

> [!NOTE]
> The incident memory is the agent's working memory, kept in Canopy rather than in the agent's own memory, so operators can read it, each entry sits beside what it concerns, and it outlasts any one conversation. An agent correcting an operator's entry loses nothing, since an entry's history keeps every version.

### What needs an operator's word

An entry saying a pattern is harmless lets a later triage log what it would otherwise escalate, so the agent records one only on an operator's word: a person its configuration names as an operator, saying so in the triage channel, or a note an operator wrote on the incident.
The same from anyone else is recorded as a note on the incident, naming who said it.

> [!NOTE]
> The channels are public and their text is free, so a direction planted in one could otherwise quiet a real outage. An entry of any other kind leaves the decision with the next triage, which weighs it as the claim it is.

### Reviewing a closed incident

A published incident [notifies again when it closes](incidents.md#notification), so every incident triage has seen comes back to the agent once its story is complete.
On that alert, the agent reads the incident's notes, its escalation thread when it has one, and its reply in the write-up thread with the replies under it, and keeps in the incident memory what they hold that the memory lacks.
Its report of the close names the entries it recorded or amended, or says there was nothing to keep.

A remark made about the incident after it has closed is kept in the same way when it reaches the agent.

## The daily write-up

A write-up day begins at 8am New Zealand time and ends at the next, so overnight alerts fall in the day the morning summary covers.
Each write-up day opens with a top-level post in the triage channel summarising the write-up day that just ended: how many alerts were escalated and how many logged, which incidents are still open, and the published incidents that opened in that day carrying no triage note, whether or not they are still open.
An alert in a write-up day whose thread has not been started starts it.
The day's thread is the one the agent itself posted, so a post by another author carrying the same title is not mistaken for it.
