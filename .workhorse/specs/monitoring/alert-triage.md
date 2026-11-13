---
id: TRI
---

# Alert triage

Applies to how Claude Tag, Anthropic's Claude agent in Slack, triages the incident notifications Canopy posts to Slack, what Canopy decides for it, and where it reports what was decided.

## Channels

- Canopy's [incident notifications](incidents.md#notification) and maintenance announcements post to the **alerts channel**.
- Each of those posts mentions Claude and writes out the link to its incident, so Claude triages every one as it arrives, in a thread under the post.
- What needs a person now goes to the **triage channel**, which holds escalations and one write-up thread per day and nothing else.
- Both channels are public and Claude is a member of both.
- Which channels these are, and how posts in them are laid out, is Claude Tag's channel configuration; Canopy holds no Slack channel names for triage.

> [!NOTE]
> Claude Tag reads messages from workflows and other bots as context and acts on one only when it mentions Claude, so the mention in Canopy's posts is what starts triage. Claude Tag posts from one channel into another only when both are public. The incident link in a post points at the operator interface on the tailnet, which Claude Tag cannot reach, so Claude reads the incident id from it rather than following it.

## Where the rules live

- The triage rules and the context they need are served by Canopy, through the [fleet query interface](../private-server/mcp.md#alert-triage), so they change with Canopy rather than with each channel's configuration.
- Canopy decides every escalation that follows mechanically from the incident's state, and Claude judges only what needs judgement.
- Canopy refuses a verdict that logs an alert it has decided must be escalated.

## The assessment

Each triage settles three things:

- **Whether it is new.** An incident is related to another when the same check is failing in both, or when their failures began close together with a shared cause plausible, such as a common release, cluster, or upstream service.
- **How severe it is.** Severity is Canopy's own grading of the incident's worst live result: critical, error, or warning.
- **Whether it needs action now**, which decides whether it is escalated or logged.

An incident's earlier triage is the baseline for the next: what Canopy compares against is the incident's worst result and failing checks as they stood when it was last triaged.

### What Canopy decides must be escalated

- The incident is on a production environment or on Canopy as a whole, and has not been triaged before.
- A failure from an escalating check is live that was not at the last triage.
- The incident's worst live result is more severe than at the last triage.
- A failure is live on a check that was not failing at the last triage.
- The same check is failing in an open incident on another environment.
- A failing check is unfamiliar: it has no documentation, no incident memory entry is linked to it, and it has never been part of a published incident before.

Each reason is given with what it rests on, such as the other incident sharing the check.

### What Claude judges

- Whether incidents on other environments that opened close in time to this one share a cause, which Canopy offers as candidates.
- Whether it can explain the failure from what it read. Anything it cannot explain with confidence is escalated, and the escalation says that it is out of uncertainty.

> [!NOTE]
> Triage leans towards escalating. A false escalation costs a person a minute; a swallowed outage at a clinical site costs far more. Canopy decides the clear cases so that the model's judgement can only add escalations, never remove one Canopy requires, except under the harmless-pattern exception below.

### The harmless-pattern exception

- A first triage of a production incident may be logged instead of escalated when an incident memory entry linked to its check or group names the pattern as harmless and the check's stability record shows that pattern recovering on its own.
- The verdict names the memory entry it relies on, and Canopy refuses it otherwise.
- The exception covers only the production-opening reason; any other reason Canopy gives still requires escalation.

### What is logged

Every alert that is not escalated is logged in the day's write-up thread, including:

- an opening on a lesser environment (clone, demo, test or dev) that Canopy gives no reason to escalate and that Claude can explain
- a reminder for an incident unchanged since its last triage
- a close
- a maintenance window being declared or ending, which links no incident and is logged without a verdict

## Recording a verdict

- Each triage's verdict is recorded on its incident: escalated or logged, the reasoning, where it was posted, and the incident's worst result and failing checks at the time.
- A verdict shows on the incident as a note written by Claude, so operators see the reasoning on the incident's page.
- When an operator confirms a cause or says that a pattern is harmless, in an escalation thread or the write-up thread, Claude records it in the incident memory, linked to the check, group, or incident it concerns.
- A memory entry describes the pattern and how to recognise it, never a copy of a check's raw output.

## Escalation posts

- An escalation is a top-level post in the triage channel.
- It gives, in plain language:
  - what is wrong, and where, as a group and environment
  - Canopy's severity
  - why it was escalated
  - what to look at first
  - related incidents
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

## In the alerts channel

- Claude replies to each alert in its thread with one line saying whether it was escalated or logged, linking where.
