---
id: MNT
---

# Maintenance windows

A maintenance window is an operator's declaration that an application, a machine, a group, or one of a group's environments is being worked on, so what Canopy observes while the work runs raises nothing.
A window is bounded in time, ends itself, and records who declared it and what for, so a quiet part of the fleet is always attributable to a decision someone made.

## Why it exists

An upgrade looks exactly like a machine falling over: it stops reporting, its checks go stale, and the group opens an incident someone has to read before recognising it as work already under way.
Alerting through planned work costs an operator the trust they place in the next alert.

Neither existing control covers it.
A silence is one check held down until someone removes it, so covering an upgrade means writing several and remembering to clear every one.
Turning a machine's monitoring off is unbounded, and a switch with nothing to turn it back on stays off.

## Declaring

An operator declares a window over one application, one machine, one group, or one of a group's environments, giving the time it is expected to end and, optionally, a note saying what is being done.
Canopy records who declared it and when, alongside the expected end and the note.
Declaring, amending, and lifting are administrative actions and are audited (see [ADM](../private-server/admin-access.md)).

A window over a machine covers the machine's own checks and those of every application running on it.
Taking a box down to patch it stops everything on it, so that is one declaration however many workloads, and an operator patching a host never has to name what it runs.

A window over one application covers that application's checks and nothing else on the box, for work that stops one product where the box serves several: an upgrade of the Tamanu server on a host also running mSupply leaves mSupply watched.
The machine's own checks stay watched with it, since the box is not being taken down.

A control offering the declaration says which grain it declares at, so an operator learns it from the control rather than from the result.

A group's window covers the group's own checks and those of every machine in it, including machines that join while it holds.

A window over one of a group's environments covers the machines serving that environment and nothing else of the group: an upgrade rehearsed on a site's clone leaves its production watched, and the group's own checks such as its backups with it.
An environment is a group's applications at one rank, and the machines serving it are those whose own rank is that one, a machine taking the rank of the highest-ranked application on it (see [GRP](../servers/groups.md), "Environments").

A target stays suspended until the last window covering it has ended: for an application, its own and its machine's; for a machine, its own, its environment's and its group's.

A group's own window, and the window over each of its environments, are separate targets, so a target has at most one open window: declaring over one that already has a window amends it, recording who amended and when.

Canopy never opens a window by itself.
An environment with an open upgrade plan is offered the declaration over itself from that plan, prefilled with the plan's window and note, so declaring is one action at the moment the work starts (see [UPG](../private-server/upgrade-plans.md)).
An hour someone typed in advance is not evidence that work began, so a planned window suspends nothing on its own.
An open incident offers the declaration over its target too, so an operator who recognises an alert as their own work declares from where they are reading it.

## What a window suspends

While a window holds over a target, its checks are observed, graded, and presented exactly as they would be without it.
An operator working through a window watches the check they are fixing come good, and a failure arriving mid-work is visible where it happened rather than held back until the window ends.

What a window suspends is what those results feed: no issue on the target contributes to an incident while it holds, so nothing opens, nothing joins, and nothing notifies.
An issue in an open incident leaves it when the window is declared, and an incident whose last effective failure leaves this way closes immediately, as it does for any operator action (see [INC](incidents.md), "Membership").
Where that close is notified, the notice says maintenance was declared, so a reader does not take it as the problem having gone away.

Canopy-wide checks are Canopy monitoring its own operation, and are never suspended by any window (see [SELF](../private-server/self-alerts.md)).

## Ending

A window ends when an operator lifts it or when its expected end passes, and Canopy records which, with the operator and the time.
Ending at the expected end without asking anyone is what keeps a forgotten window from leaving a target unwatched.
Work running long extends the window by amending its end before it passes; a window that has ended is history, and suspending again is a fresh declaration.

Ended windows are retained as the target's maintenance history, so what was being done the last time it went quiet is readable against it.

## Settling

Suspension persists for a settle period after the window ends, suppressing exactly what the window itself did.

A machine is back before the sources on it have reported again, and a machine whose every source is stale is unreachable (see [CHK](checks.md), "Reachability"), so ending suspension the instant the work finishes would page for a machine that has just come back, for as long as the work took.
The settle period is the same for every window.
When it elapses, anything still degraded on the target contributes to incidents from then on.

## Notification

Canopy notifies operators when a window is declared and again when its suspension ends, over the channel that carries the target's incidents, which for a machine is its group's (see [INC](incidents.md), "Notification").
The declaration names the target, the operator, the expected end, and the note.
The ending says whether an operator lifted the window or its expected end passed, and when watching resumes.

## Presentation

A target under a window presents its own health and reachability, and is marked as under maintenance wherever they are presented as they currently stand, distinguishably from a machine nobody is watching (see [CHK](checks.md), "Monitoring gate").
The mark is drawn at the grain the window was declared over, so an environment's window does not read as every machine in it having one of its own: an application's dot, a machine's enclosure, an environment's row, a group's card.
A dot is hollowed rather than patterned or cut: a pattern needs room to resolve, and a cut is what marks a target nobody is watching, which at a dot's size would differ only by its angle.
Every target the window reaches has its health muted, marked or not, and carries the window and when it ends, so a failing target under maintenance is not read as one nobody has noticed.
A target serving out the settle period carries the mark still, distinguished from one whose window holds, so lifting a window shows on the target rather than only on the window.
The two are told apart by motion as well as by weight: a holding window's mark moves and a settling one is still, so movement on the page means someone is in there now.
A mark with room to it is crossed, and one drawn small pulses, a sweep being gone before it resolves on a few pixels.
A reader who has asked for less motion gets the weight alone.
The status legend names both marks.

Canopy presents every open window across the fleet in one view: what each covers, who declared it, when it ends, and its note.
The view answers "what are we not watching right now" without reading each group.

A target's own surface presents its open window with the actions to amend or lift it, and its ended windows as history.
An application presents the window over the machine it runs on, and a machine covered by its group's window or its environment's presents that window too, naming what holds it and leading there, since a target under maintenance without a window of its own would otherwise read as one nobody had declared.
A group presents the windows over its environments beside its own, each naming the environment it covers, since a group whose clone is under maintenance is only partly quiet.
The mark sits on the target rather than on each of its checks, a window covering all of them alike, so a check under one is read against the target's mark exactly as a check on an unmonitored target is.

## Out of scope

- Performing, scheduling, or triggering the maintenance itself.
- Declaring a window to begin at a future time: a window says work is happening now, and an intended one is what a plan records (see [UPG](../private-server/upgrade-plans.md)).
- Suspending Canopy's own self-monitoring.
- Exempting a check from suspension so it still alerts while a window holds.
