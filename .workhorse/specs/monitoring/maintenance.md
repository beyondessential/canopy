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

A group's window covers the group's own checks and those of every machine in it, including machines that join while it holds.

A window over one of a group's environments covers the machines serving that environment and nothing else of the group: an upgrade rehearsed on a site's clone leaves its production watched, and the group's own checks such as its backups with it.
The machines serving an environment are those in it, a machine being in the environment its rank names (see [GRP](../servers/groups.md), "Environments").
A window over a group's headline environment leaves the group's own checks watched too: their incidents are the headline environment's, but they are on none of its machines.

A target stays suspended until the last window covering it has ended: for an application, its own and its machine's; for a machine, its own, its environment's and its group's.

A group's own window, and the window over each of its environments, are separate targets, so a target has at most one open window: declaring over one that already has a window amends it, recording who amended and when.
The window is then the amender's work as well as the declarer's, which decides who may run against it (see [INV](../private-server/inventory.md), "Work under way").

Canopy never opens a window by itself.
An environment with an open upgrade plan is offered the declaration over itself from that plan, prefilled with the plan's window and note, so declaring is one action at the moment the work starts (see [UPG](../private-server/upgrade-plans.md)).
An hour someone typed in advance is not evidence that work began, so a planned window suspends nothing on its own.
An open incident offers the declaration over its target too, so an operator who recognises an alert as their own work declares from where they are reading it.
A group, a machine, and an application each offer the declaration over themselves from their maintenance section, and again at the head of their page beside its other actions, so it is at hand without finding that section.
From a group, each environment it has is a choice away (see "Choosing what to cover"), so an environment is declarable whether or not a plan is open on it.
The control at the head of the page reads "Maintenance" whatever the target's state, and opens as an amendment of the target's own window where it has one, offering to lift it from there too.

## Choosing what to cover

The declaration starts at the grain of wherever it was offered from, and shows which grain that is before anything is declared, so an operator learns it from the control rather than from the result.
The operator can retarget it to any grain on the same line of descent: whatever contains the starting grain, and whatever it contains.
From an application the choices are its machine, the machine's environment, and its group; from a machine, its group, the environment it serves, and its applications; from an environment, its group, the machines serving it, and their applications; from a group, each of its environments and every machine and application in it.
A grain the starting point has none of is passed over: a machine in no group offers its applications alone, and a pending machine, being in no environment, offers its group with no environment between them.
The choices are listed in the shape they nest, group over environment over machine over application, so the choice reads as a choice of how wide.
A group's pending machines are listed under it apart from its environments, as the group presents them.

Retargeting onto a grain that has an open window of its own makes the declaration an amendment of that window, and the dialog says so and shows that window's expected end and note.
The amendment changes only what the operator has explicitly changed in the dialog, so retargeting onto someone else's window never shortens it or replaces its note unless the operator has decided to.

Offered from an open incident, the declaration starts at the incident's environment.
A choice that leaves any of the incident's failing checks contributing is marked as not covering them all.
So an operator reading a backup failure on a group's headline environment learns from the dialog that only the group quiets it, since a window over that environment leaves the group's own checks watched.
Coverage is reckoned against failures alone, because an incident whose failures have all left closes whatever warnings remain in it (see [INC](incidents.md), "Membership").

The declaration an upgrade plan offers, and the one a configuration run asks for before taking its lease, stay over the target they were offered for and cannot be retargeted: each declares the work on one environment, which the plan stays open for and the lease is served against (see [UPG](../private-server/upgrade-plans.md) and [INV](../private-server/inventory.md), "Work under way").

## Moving a window

Amending a window can retarget it as well, to any grain on its target's line of descent, so a window declared too wide or too narrow is corrected without lifting it and declaring again.
It stays the same window: its declarer and when it was declared carry over, the move is recorded as an amendment, and the window is the mover's work from then on as well as the declarer's.
It is the new target's window from then on, and joins the new target's history when it ends.
The target it left keeps it in its own history over the span it covered there, so a quiet spell is attributable from either.
A grain with an open window of its own is listed when moving but cannot be chosen, since a target holds at most one window.

A declaration offered over a target that has an open window of its own is an amendment of that window from the start, wherever it is offered from, so choosing another grain in it moves that window.
One offered over a target with no window of its own declares at whichever grain is chosen.

A window declared from an upgrade plan's offer is that plan's window and stays over the plan's environment: it can be amended and lifted but not moved, so the plan it holds open is not released partway through the work (see [UPG](../private-server/upgrade-plans.md), "When a plan is met").
Any other window over the environment, such as one declared before the plan was recorded, moves like any other.
A window a run lease is being served against cannot be moved while that lease is held, since the run is acting on the environment the window covers; once the lease is released or expires, the window moves like any other (see [INV](../private-server/inventory.md), "Work under way").

What the move newly covers is suspended from the moment of the move.
What it leaves uncovered serves the settle period as though the window had ended over it, so narrowing a group's window to one environment does not page for the rest of the group the moment it moves.
A move is an amendment, so it is recorded on the window and audited rather than notified, and what it leaves uncovered resumes watching at the end of that settle period without a notice of its own.

## What a window suspends

While a window holds over a target, its checks are observed, graded, and presented exactly as they would be without it.
An operator working through a window watches the check they are fixing come good, and a failure arriving mid-work is visible where it happened rather than held back until the window ends.

What a window suspends is what those results feed: no issue on the target contributes to an incident while it holds, so nothing opens, nothing joins, and nothing notifies.
An issue in an open incident leaves it when a window is declared or moved over its target, and an incident whose last effective failure leaves this way closes immediately, as it does for any operator action (see [INC](incidents.md), "Membership").
Where that close is notified, the notice says maintenance was declared, so a reader does not take it as the problem having gone away.

Canopy-wide checks are Canopy monitoring its own operation, and are never suspended by any window (see [SELF](../private-server/self-alerts.md)).

A window also holds off configuration runs by others: while it holds, no one but the operator who declared it, or the one whose amendment of it stands, can take a run lease on an environment it covers (see [INV](../private-server/inventory.md), "Work under way").

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
The mark is drawn at the grain the window is over, so an environment's window does not read as every machine in it having one of its own: an application's dot, a machine's enclosure, an environment's row, a group's card.
A dot is hollowed rather than patterned or cut: a pattern needs room to resolve, and a cut is what marks a target nobody is watching, which at a dot's size would differ only by its angle.
Every target the window reaches has its health muted, marked or not, and carries the window and when it ends, so a failing target under maintenance is not read as one nobody has noticed.
A target serving out the settle period carries the mark still, distinguished from one whose window holds, so lifting a window shows on the target rather than only on the window.
What a move leaves uncovered settles at the grain the window was over before it moved, so a group's window narrowed to one environment marks the group's card as settling and that environment's row as held.
The two are told apart by motion as well as by weight: a holding window's mark moves and a settling one is still, so movement on the page means someone is in there now.
A mark with room to it is crossed, and one drawn small pulses, a sweep being gone before it resolves on a few pixels.
A reader who has asked for less motion gets the weight alone.
The status legend names both marks.

Canopy presents every open window across the fleet in one view: what each covers, who declared it, when it ends, and its note.
The view answers "what are we not watching right now" without reading each group.

A target's own surface presents its open window with the actions to amend or lift it, and its ended windows as history.
An application presents the window over the machine it runs on, and a machine covered by its group's window or its environment's presents that window too, naming what holds it and leading there, since a target under maintenance without a window of its own would otherwise read as one nobody had declared.
A group presents the windows over its environments beside its own, each naming the environment it covers, since a group whose clone is under maintenance is only partly quiet, and offers the actions to amend or lift each of them as it does for its own.
The mark sits on the target rather than on each of its checks, a window covering all of them alike, so a check under one is read against the target's mark exactly as a check on an unmonitored target is.

## Out of scope

- Performing, scheduling, or triggering the maintenance itself.
- Declaring a window to begin at a future time: a window says work is happening now, and an intended one is what a plan records (see [UPG](../private-server/upgrade-plans.md)).
- Suspending Canopy's own self-monitoring.
- Exempting a check from suspension so it still alerts while a window holds.
