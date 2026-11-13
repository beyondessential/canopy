---
id: UPG
---

# Planned upgrades

Canopy records where each environment is going: the version one of a group's environments intends to move to, and optionally the day and the window on it.
A plan makes the fleet's intended upgrades visible in one place, and it tells the rest of Canopy which version to hold an environment's data against, so pre-upgrade testing exercises the version that will actually be applied rather than guessing.

## Scope

This spec covers what a plan records, who sets it, when Canopy considers it met, and what reads it.

It does not cover performing an upgrade.
Canopy serves versions to applications and records what they report running; the act of upgrading is the site's own, and a plan is a statement of intent rather than an instruction to anything.

## Why it exists

Canopy already knows what every application runs and every version that exists.
What it cannot derive is where an environment is going: which minor a site moves to next is a decision made by people, weighing what the release contains against what the site can absorb.
Nor is it one decision per site: a clone is often moved first, to rehearse the upgrade on a copy of the data before production follows, so the clone and the production are going different places at once.

Left unrecorded, that decision costs twice.
Pre-upgrade migration testing has nothing to aim at, so an environment gets no answer about its own data until it says where it is going (see [RST](../public-server/restore-replicas.md)).
And nobody can see at a glance which environments are mid-plan and which are overdue to move.

## A plan

An operator records, per environment (see [GRP](../servers/groups.md), "Environments"):

- the **target version**, which must be a published version newer than the environment is running;
- an optional **planned date**, the day the upgrade is expected to happen;
- an optional **planned time**, the hour on that day the upgrade starts, with the **timezone** it is a wall clock in;
- an optional **end time**, the hour the window closes, in the same zone;
- an optional **note**, for whatever an operator needs the next reader to know;
- who recorded it and when.

A planned time requires a planned date and a timezone.
Canopy holds no timezone for a group, and the fleet spans enough of the world that a wall-clock time without a zone means nothing to anyone but whoever typed it.
The time and window are optional, so a plan can be recorded before its window has been estimated and gain one when it is.

A close needs an open to bound, and one earlier in the day than the open is the following morning: an upgrade from 22:00 to 02:00 is one night's window rather than a plan that runs backwards.
A close at the hour it opens is refused, since it reads as either no window at all or a whole day.
An hour on its own says nothing about how long an environment is down, so recording one offers a close an hour later, which is the length the calendar feed would otherwise have assumed.
That is a starting point and not a claim: it is editable, and a plan whose window nobody has settled records no close at all.

An environment has at most one open plan, and a group holds as many open plans as it has environments going somewhere.
An environment moves to one place next, so a second plan for it replaces the first rather than queueing behind it, and the replaced plan is retained as history.
A plan for a group's clone leaves its production's plan where it was.

What an environment runs is what its own central reports, so a site's production reads from its production central and its clone from the clone's (see [APP](../servers/application-types.md), "Versions").
Wherever an environment is named, it is named by its group with the rank after it, except a group's production, which is named by the group alone.
In the upgrades tables an environment's rank is also shown as a chip, production's included, so a production row stands out from its clone and demo.
A plan names an environment the group has applications at; one cannot be recorded for a rank the group holds nothing at, so a group with nothing ranked records no plans (see [GRP](../servers/groups.md), "Environments").

An open plan's date, time, and note can be amended, and an amendment records who made it and when.
A corrected date or a reworded note is the same plan better described, so it stays one plan rather than entering the history as a second.
Changing the target is not an amendment: where an environment is going is what the history exists to record, so a new target replaces the plan as any other second plan would.
A plan that has been met or replaced is history and is no longer amendable.

Plans are managed through the operator interface and are audited (see [AUD](../platform/audit-log.md)).
Withdrawing a plan says the environment is no longer going there; it does not say the upgrade happened.
A withdrawn plan is retained and records who withdrew it and when: an environment that was going somewhere and stopped is part of the same history as one that arrived, and it frees the environment to be planned somewhere else.

A planned date is a plan, not a deadline.

## When a plan is met

Canopy decides a plan is met, rather than asking anyone to mark it done.

A version is on a machine from the moment it is installed there, which is ahead of the environment serving it: an upgrade reports the version it is putting on as the work begins, with hours of it still to run.
A plan is met once the environment's reported version has reached its target and its work is over, at which point the plan closes and records when.

Two things say the work is still going, and either holds the plan open.
The plan's own window is the first: an operator who recorded when the work runs has said so in advance, and the plan stays open until that window closes whether or not anything else is declared.
A maintenance window over the environment or its group is the second, and it holds the plan for as long as it holds.
A plan that recorded no window has nothing to wait on, so the version arriving is all the evidence there is.

An operator therefore has the plan, and the controls on it, in front of them for the length of the work rather than losing it partway.
No elapsed time stands in for either signal: an upgrade that finishes in a minute and one that runs for hours are not told apart by a clock.

Reaching a version *past* the target counts as reaching it: an environment that jumped further than planned has done the upgrade and then some, and holding the plan open would misreport it as outstanding.
A clone arriving says nothing about its production, whose plan stays open until the production itself reports the target.

A met plan is retained.
The record of what an environment planned, when it planned it for, and when it actually landed is the fleet's upgrade history, and it is what makes "how long do our upgrades really take to happen" answerable.

## What reads a plan

Pre-upgrade migration testing takes each application's target from its own environment's open plan, and an environment with no plan is not tested at all.
Recording a plan is what asks for the testing, and no restore is spent on a version nobody intends to apply.

An upgrade run on a production environment reads the open plan as its permission (see [ANSRUN](ansible/runs.md), "Planned upgrades").

A plan changes what is tested, so changing one invalidates nothing already recorded: earlier verdicts stand against the versions they named, and the new target simply becomes the one that has not been tested yet.

Declaring maintenance reads the open plan for its prefill: an operator starting the work is offered a window over the plan's environment, carrying the plan's window and note (see [MNT](../monitoring/maintenance.md)).
A window declared this way stays over the plan's environment until it ends, so the plan it holds open stays held (see [MNT](../monitoring/maintenance.md), "Moving a window").
The plan is read at the moment someone declares, so a planned hour arriving suspends nothing on its own.

## The dashboard

Canopy presents planned upgrades across the fleet in one view, so the question "what is moving, and when" is answered without reading each group.

Open plans are shown a group at a time.
Each group has a header naming it, with the target version where every planned environment of the group is going to the same one, and beneath the header each environment with an open plan is a row giving its rank, the version it runs now, the planned date and window where there are ones, and the note.
Where a group's environments are going to different versions, each row names its own target instead.
The window is shown with its timezone abbreviated, so a reader can tell which site's midnight it is.
An environment's own actions, amending its plan, declaring maintenance over it, and withdrawing the plan, sit behind one control on its row.

The header gives the group's pre-upgrade verdict once, so an operator sees both the intent and whether the group's data survives it.
It counts only the applications a testing declaration restores (see [RST](../public-server/restore-replicas.md), "Verdicts").
Testing is usually set up on one server of a group, often only its production central, and an application nothing tests would read as not yet tested for good, which looks like a problem when nothing is wrong.
With one such application the header names it and its result: passed with how many migrations ran, how long they took and when, failed with the migration that failed, or not yet tested.
With several it says how many of them passed, naming a failure first.
A failure tints the header, and the error it produced is a hover away.
The verdict links to the group's own migration tests.
Where nothing tests any of the group's planned environments the header says so quietly in place of a verdict, since such a plan is never dispatched and a reader would otherwise wait on a result that cannot arrive; it is not presented as a warning, since a group with no testing set up is a choice rather than a fault.
The header also says whether the group is tested on the schedule or only when asked.

Where a restore attempt is under way the header shows that too.
A restore takes hours, and without this a plan that is still being tested and one the pipeline has stopped testing both read as not yet tested.
An attempt is the group's rather than one environment's, since a restore is issued credentials for the group's backups without naming the machine it will restore, so it shows for a group with an environment that is set up for testing and could be tested now: one tested on the schedule, or one tested on request while an ask waits.
An environment tested only on request with no ask waiting is not dispatched, so a restore under way in its group is some other work and is not shown.
A group set up for testing offers to test it now, asking for each of its planned environments that is set up for testing, and while an ask waits on a verdict it says so in place of the offer (see [RST](../public-server/restore-replicas.md), "Dispatching a migration test").

A plan whose date has passed without being met is presented as late, judged on the day alone rather than the hour.
Late is a presentational state and not an incident: an upgrade slipping is normal operational reality, and Canopy has no basis for treating a date someone typed as a failure of anything.

Alongside the table the same view presents the fleet's plans as a calendar, read as a month, a week, or a single day.
The month answers which week is busy and which two environments land on the same night; the week and the day place each plan on an hour grid, where the length of a block is how long the environment expects to be down and a window running past midnight is drawn again on the following morning rather than clipped at it.
A day in the month opens that day on its own.
An entry on the calendar amends the plan it names, so a date moved by a week is moved where it is read.

The same view presents the plans that have closed, most recently closed first, so what an environment planned before is readable beside what it plans now.
Each shows the environment, where it was going, the date and hour it was planned for, and how it closed: met, replaced by a later plan, or withdrawn with the operator who withdrew it and when.
A withdrawn plan is otherwise unreadable anywhere, since an environment that stopped going somewhere leaves no other mark on the fleet.

## The calendar feed

Canopy publishes planned upgrades as an iCalendar feed a calendar application subscribes to, so the fleet's intent appears where people already look to see what a day holds.

The feed is served by the internet-facing interface rather than the operator one, because the calendar services people use fetch a subscription unattended from their own infrastructure and cannot reach the operator network.
Access is carried by a secret in the URL: a calendar application has no way to be asked for a credential, so holding the URL is what grants the read.

There is one feed at one URL, configured with the Canopy instance rather than minted per subscriber.
The audience is everyone who works on the fleet, and the link is posted once where they can all read it, so a URL each would be bookkeeping against a credential they already share.
The dashboard offers the URL for copying, and rotating it is a configuration change every subscriber then resubscribes to.
The secret is the one already gating the public server-versions page: the same fleet information, to the same audience, over the same open path.
Where none is configured the feed serves nothing, since an ungated one is an open read of the fleet's plans.

The calendar carries the plans that name a day, while a plan is where the environment is going and after it has been met.
A replaced or withdrawn plan leaves the calendar, since the environment is no longer going there; a met one stays as the record of what landed.

An entry names the environment and the version it is going to, and carries what the plan records beyond the day: the version the environment is on now and the note, or for a met plan the day it landed.
It does not carry who recorded the plan: the feed is read by anyone holding its URL, and an operator's address is no part of what a calendar needs.
A plan that names an hour is an entry at that hour, resolved from the zone the plan is a wall clock in so every subscriber reads the same instant whatever their own zone; a plan with only a day is an all-day entry.
The entry runs until the hour the plan closes the window, or for an hour where the plan names no close: the entry marks the start of the window rather than claiming to know how long the upgrade takes.
An entry marks nobody busy: a planned upgrade is something to know about rather than something that occupies whoever subscribed.

An entry keeps its identity for the life of the plan, so amending a date moves the entry a subscriber already has rather than leaving them holding two.

## Out of scope

- Performing, scheduling, or triggering an upgrade.
- Approving a plan, or gating who may record one beyond the existing operator permissions.
- Planning anything other than a version move, such as an infrastructure migration.
- Per-subscriber access to the calendar feed, or withdrawing one subscriber's.
