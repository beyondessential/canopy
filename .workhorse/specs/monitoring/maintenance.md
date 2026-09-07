---
id: MNT
---

# Maintenance windows

A maintenance window is an operator's declaration that a machine or a group is being worked on, so what Canopy observes while the work runs raises nothing.
A window is bounded in time, ends itself, and records who declared it and what for, so a quiet part of the fleet is always attributable to a decision someone made.

## Why it exists

An upgrade looks exactly like a machine falling over: it stops reporting, its checks go stale, and the group opens an incident someone has to read before recognising it as work already under way.
Alerting through planned work costs an operator the trust they place in the next alert.

Neither existing control covers it.
A silence is one check held down until someone removes it, so covering an upgrade means writing several and remembering to clear every one.
Turning a machine's monitoring off is unbounded, and a switch with nothing to turn it back on stays off.

## Declaring

An operator declares a window over one machine or one group, giving the time it is expected to end and, optionally, a note saying what is being done.
Canopy records who declared it and when, alongside the expected end and the note.
Declaring, amending, and lifting are administrative actions and are audited (see [ADM](../private-server/admin-access.md)).

A window is over the machine rather than over one workload on it, and covers the machine's own checks and those of every application running on it.
Taking a box down to patch it stops everything on it, so a window naming one application would leave the others alerting through work that was always going to stop them: one declaration, however many workloads.

A group's window covers the group's own checks and those of every machine in it, including machines that join while it holds.
A machine covered by its own window and by its group's stays suspended until the last of them has ended.

A target has at most one open window: declaring over one that already has a window amends it, recording who amended and when.

Canopy never opens a window by itself.
A group with an open upgrade plan is offered the declaration from that plan, prefilled with its window and note, so declaring is one action at the moment the work starts (see [UPG](../private-server/upgrade-plans.md)).
An hour someone typed in advance is not evidence that work began, so a planned window suspends nothing on its own.
An open incident offers the declaration over its target too, so an operator who recognises an alert as their own work declares from where they are reading it.

## What a window suspends

While a window holds over a target, every check on that target has an effective result of skipped, whatever it was observed as: the transform a silence applies to one check, applied to all of them for as long as the window holds (see [CHK](checks.md), "Policy").
Observed results are recorded throughout, and sources are expected to report and told to run their checks exactly as they were before, so a window changes how Canopy grades what it sees and nothing else.

A skipped check is not an issue, so a target under a window contributes nothing to incidents and raises no notification.
An issue in an open incident leaves it when the window is declared, and an incident whose last effective failure leaves this way closes immediately, as it does for any operator action (see [INC](incidents.md), "Membership").
Where that close is notified, the notice says maintenance was declared, so a reader does not take it as the problem having gone away.

Canopy-wide checks are Canopy monitoring its own operation, and are never suspended by a window over a machine or a group (see [SELF](../private-server/self-alerts.md)).

A window also holds off configuration runs by others: while it holds, the inventory of every environment it covers is refused to anyone but the operator who declared it (see [INV](../private-server/inventory.md), "Work under way").

## Ending

A window ends when an operator lifts it or when its expected end passes, and Canopy records which, with the operator and the time.
Ending at the expected end without asking anyone is what keeps a forgotten window from leaving a target unwatched.
Work running long extends the window by amending its end before it passes; a window that has ended is history, and suspending again is a fresh declaration.

Ended windows are retained as the target's maintenance history, so what was being done the last time it went quiet is readable against it.

## Settling

Suspension persists for a settle period after the window ends, unchanged in every respect from the window itself.

A machine is back before the sources on it have reported again, and a machine whose every source is stale is unreachable (see [CHK](checks.md), "Reachability"), so ending suspension the instant the work finishes would report a machine that has just come back as failed for as long as the work took.
The settle period is the same for every window.
When it elapses, every check on the target is graded normally again, and anything still degraded contributes from then on.

## Notification

Canopy notifies operators when a window is declared and again when its suspension ends, over the channel that carries the target's incidents, which for a machine is its group's (see [INC](incidents.md), "Notification").
The declaration names the target, the operator, the expected end, and the note.
The ending says whether an operator lifted the window or its expected end passed, and when watching resumes.

## Presentation

A target under a window presents as under maintenance wherever its health or reachability is presented as it currently stands, marked in the manner an unmonitored machine is and distinguishable from one (see [CHK](checks.md), "Monitoring gate").
Its health is muted and carries the window and when it ends, so a failing machine under maintenance is not read as one nobody has noticed.
The status legend names the mark.

Canopy presents every open window across the fleet in one view: what each covers, who declared it, when it ends, and its note.
The view answers "what are we not watching right now" without reading each group.

A target's own surface presents its open window with the actions to amend or lift it, and its ended windows as history.
An application presents the window over the machine it runs on, and a machine covered by its group's window presents that window too, naming what holds it and leading there, since a target under maintenance without a window of its own would otherwise read as one nobody had declared.
A check skipped because a window holds says so where its result is presented.

## Out of scope

- Performing, scheduling, or triggering the maintenance itself.
- Declaring a window to begin at a future time: a window says work is happening now, and an intended one is what a plan records (see [UPG](../private-server/upgrade-plans.md)).
- Suspending Canopy's own self-monitoring.
- Exempting a check from suspension so it still alerts while a window holds.
