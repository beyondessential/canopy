---
id: INC
---

# Incidents

An incident is a span of trouble on a target: one of a group's environments, or Canopy as a whole.
It aggregates the issues active on that target over its lifetime, from when it opens until it closes or an operator resolves it.
At most one incident is open per target at a time.

Issues and effective results are defined by the check-state model (see [CHK](checks.md)).
An issue is scoped to what its check asserts something about — an application, a machine, a cluster, a group, or Canopy as a whole — and that scope decides which target's incident it belongs to.

## Targets

Each of a group's environments is a target of its own, its other environment included (see [GRP](../servers/groups.md), "Environments").
So a site's test box going down is an incident on that site's test environment while its production central going down is an incident on the site's production, and neither joins the other.

An application-scoped issue belongs to its application's environment, and a machine-scoped issue to its machine's.
So an environment's trouble is one incident whether it began on a box or in the software on it, and a machine's failure is not split across the applications it hosts: a box going down takes its machine's checks, its ranked workloads' and the unranked database beside them into one incident.

A group-scoped issue belongs to the group's other environment, because what a group check asserts is held once for the group however many environments it has: its backups are one repository (see [GRP](../servers/groups.md)).
Canopy-wide issues belong to the Canopy target.
An issue on an application or a machine belonging to no group belongs to no target and cannot contribute to incidents.
A cluster belongs to no group, so a cluster's issues belong to no target either: they are read on the cluster and count towards its health (see [K8S](kubernetes.md), "A cluster's page").

Canopy attaches no configuration to an environment, so an environment's notification channel, grace period, and linger window are its group's.

## Membership

An incident opens when a check's effective result becomes failed on a target with no open incident.
While an incident is open, every issue on its target joins it — effective warnings included — so the incident carries the full context of what was wrong during its span.

An issue leaves the incident when it stops being one: its effective result recovers (to passed or skipped, whether by report or by policy), it is resolved or snoozed, or the machine or application it is on stops being monitored.
Warnings never hold an incident open.

When the last effective failure leaves because its result recovered, the incident does not close immediately: it **lingers** for its target's linger window, remaining the target's open incident.
A check whose effective result becomes failed during the window — a fresh failure or the same one returning — ends the lingering and the incident continues.
An incident whose linger window elapses without an effective failure closes, recording the close as of when its last effective failure left.
Lingering damps reporter flapping, not operator action: a last failure leaving through resolution, snooze, silence, a maintenance window declared over its target (see [MNT](maintenance.md)), or its target's monitoring being turned off closes the incident immediately.

The membership history — which issues joined and left, and when — is kept and presented as the incident's timeline.
An issue can leave and rejoin the same incident.

The timeline leads with what is worst rather than what is newest: issues are ordered by effective result, most severe first, and issues sharing a result are ordered most recent first.
An issue with no recorded result is ordered below every graded one.
Notes are ordered most recent first and sit below every issue.

Operator actions that change what counts (monitoring toggles, group membership changes, rank changes, policy and silence changes) re-evaluate the affected issues' incident membership.
A rank change moves an application's issues, its machine's, and those of every unranked application on its machine to the environment they now belong to, closing an incident the move leaves with no effective failure.

Membership evaluation is asynchronous. A report records its issue state immediately; the resulting open, join, leave, or close follows within a short bounded delay rather than synchronously with the report. Membership is therefore eventually consistent with the current issue state.

## Notification

Operators are notified over the notification channel: the incidents of every environment in a group to the group's configured channel, Canopy-wide incidents to the operator channel.
Wherever an incident's target is named, an environment reads as its group's name with its rank, or "other", after it, and a production environment reads as the group's name alone, so a site's production trouble is announced under the site and the rest of its environments under their own names.
On the group's own surface the group's name is already the heading, so an environment there reads by its rank, or as "other", alone.

An incident notifies when it has stayed open past its target's grace period; the notification additionally waits out any lingering, so it is sent only while an effective failure is live.
An incident that closes before its notification was sent never notifies.
Whether an incident notified is recorded as its **published** flag, so flaps can be excluded from reporting.
An escalating check's effective failure (see [CHK](checks.md), "Policy") notifies immediately, bypassing any remaining grace; if the incident has already notified, the join escalates it with a further notification, at most once per incident.
A notified incident notifies again when it closes.

A notified incident still open a day after it opened sends a reminder, and another for each further day it stays open.
A reminder waits out any lingering as the opening notification does, so it is sent only while an effective failure is live, and a reminder whose incident closes while lingering is never sent.

The opening notification, an escalation, and a reminder each summarise the incident as it stands when the notification is sent, so issues that joined during the grace period appear in it.
The summary gives the incident's target, its worst live result as the notification's severity (critical when a live failure escalates), and how many of its live issues sit at each result.
It then lists the live issues in timeline order, one line each, giving the issue's result, its headline (or its check where it has none), and the application or machine it is on; an issue scoped to a group or to Canopy as a whole carries no location.
The list shows at most five issues; any beyond those are counted at its end rather than listed.
A reminder leads with how long the incident has been open.

## Resolution

An operator can resolve an open incident, recording who and why.
Resolution cascades to the incident's open issues — each is resolved with the same attribution — and the incident closes as its members leave.
Unresolving clears the resolution record; it does not reopen the incident.

Notes attach free-form operator commentary to an incident.
