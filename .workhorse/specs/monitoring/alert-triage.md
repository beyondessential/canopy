---
id: TRI
---

# Alert triage

Applies to how Claude Tag, Anthropic's Claude agent in Slack, triages the incident notifications Canopy posts to Slack, and where it reports what it decided.

## Channels

- Canopy's [incident notifications](incidents.md#notification) and maintenance announcements post to the **alerts channel**.
- Each of those posts addresses Claude, so Claude triages every one as it arrives, in a thread under the post.
- What needs a person now goes to the **triage channel**, which holds escalations and one write-up thread per day and nothing else.
- Both channels are public and Claude is a member of both.

> [!NOTE]
> Claude Tag reads messages from workflows and other bots as context and acts on one only when it mentions Claude, so the mention in Canopy's posts is what starts triage. Claude Tag posts from one channel into another only when both are public.

## Reading an alert

- Triage starts from the incident the post links to, read through the [fleet query interface](../private-server/mcp.md), never from the post's text alone.
- For each live issue in the incident, Claude reads the check's documentation, its stability record, and the [incident memory](incident-memory.md) entries linked to the check, the group, or the incident.
- Claude reads the incident's notes, including what earlier triage of the same incident recorded.
- Claude reads the other incidents open across the fleet, and those closed within the last day, to find related trouble.

## The assessment

Each triage settles three things:

- **Whether it is new.** An incident is related to another when the same check is failing in both, or when their failures began close together with a shared cause plausible, such as a common release, cluster, or upstream service.
- **How severe it is.** Severity is Canopy's own grading of the incident's worst live result: critical, error, or warning.
- **Whether it needs action now**, which decides whether it is escalated.

### What is escalated

An alert is escalated when any of these holds:

- It opens an incident on a production environment or on Canopy as a whole.
- It is an escalation, carrying a failure from an escalating check.
- The incident is worse than when it was last triaged: its worst live result is more severe, or a failure joined on a check that was not failing before.
- It is related to another open incident on a different environment, so the trouble spans more than one environment.
- Claude cannot assess it with confidence: the incident cannot be read, a failing check is undocumented and unfamiliar, or nothing Claude reads explains the failure.

An opening on a production environment is logged rather than escalated only when an incident memory entry names the pattern as harmless and the check's stability record shows that pattern recovering on its own.

> [!NOTE]
> Triage leans towards escalating. A false escalation costs a person a minute; a swallowed outage at a clinical site costs far more. Claude says when it escalates out of uncertainty, so the reader knows which escalations are judgement and which are clear.

### What is logged

Every alert that is not escalated is logged in the day's write-up thread. That includes:

- an opening on a lesser environment (clone, demo, test or dev) whose failures are explained and unrelated to production trouble
- a reminder for an incident unchanged since its last triage
- a close
- a maintenance window being declared or ending

## Escalation posts

- An escalation is a top-level post in the triage channel.
- It gives, in plain language:
  - what is wrong, and where, as a group and environment
  - Canopy's severity
  - why it was escalated
  - what to look at first
  - a link to the incident
- An escalation of related incidents is one post naming every environment involved, not one post per incident.
- When an escalated incident worsens, the new escalation links the earlier post.
- Later news on an escalated incident, such as a reminder or its close, is a reply in the escalation post's thread.
- Claude answers questions and takes corrections from operators in an escalation's thread.

## The daily write-up

- Each day at 8am New Zealand time, Claude starts the day's write-up thread in the triage channel.
- The thread's opening summarises the day before it: how many alerts were escalated and how many logged, and which incidents are still open.
- A write-up day runs from one opening to the next, so overnight alerts are in the thread the morning summary covers.
- An alert arriving when the day's thread has not been started starts it.
- Each alert adds one reply to the day's thread, naming the incident's group and environment, what happened, and how it was triaged and why.
- An escalated alert's reply links its escalation post, so the thread is the whole day's record.

## Recording in Canopy

- Each triage adds a note to its incident recording the verdict, the reasoning, and a link to the escalation post or write-up reply.
- These notes are how a later triage of the same incident knows what was decided before.
- When an operator confirms a cause or says that a pattern is harmless, in an escalation thread or the write-up thread, Claude records it in the incident memory, linked to the check, group, or incident it concerns.
- A memory entry describes the pattern and how to recognise it, never a copy of a check's raw output.

## In the alerts channel

- Claude replies to each alert in its thread with one line saying whether it was escalated or logged, linking where.
