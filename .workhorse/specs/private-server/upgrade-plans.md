---
id: UPG
---

# Planned upgrades

Canopy records where each group is going: the version it intends to move to, and optionally the day and the window on it.
A plan makes the fleet's intended upgrades visible in one place, and it tells the rest of Canopy which version to hold a group's data against, so pre-upgrade testing exercises the version that will actually be applied rather than guessing.

## Scope

This spec covers what a plan records, who sets it, when Canopy considers it met, and what reads it.

It does not cover performing an upgrade.
Canopy serves versions to applications and records what they report running; the act of upgrading is the group's own, and a plan is a statement of intent rather than an instruction to anything.

## Why it exists

Canopy already knows what every application runs and every version that exists.
What it cannot derive is where a group is going: which minor it moves to next is a decision made by people, weighing what the release contains against what the site can absorb.

Left unrecorded, that decision costs twice.
Pre-upgrade migration testing has nothing to aim at, so a group gets no answer about its own data until it says where it is going (see [RST](../public-server/restore-replicas.md)).
And nobody can see at a glance which groups are mid-plan, which are overdue to move, and which have no plan at all.

## A plan

An operator records, per group:

- the **target version**, which must be a published version newer than the group is running;
- an optional **planned date**, the day the upgrade is expected to happen;
- an optional **planned time**, the hour on that day the upgrade starts, with the **timezone** it is a wall clock in;
- an optional **end time**, the hour the window closes, in the same zone;
- an optional **note**, for whatever an operator needs the next reader to know;
- who recorded it and when.

An hour needs a day to sit on and a zone to be read in: Canopy holds no timezone for a group, and the fleet spans enough of the world that a bare wall clock means nothing to anyone but whoever typed it.
Most plans have no hour, since which night a group moves is usually settled well before what time it starts.

A close needs an open to bound, and one earlier in the day than the open is the following morning: an upgrade from 22:00 to 02:00 is one night's window rather than a plan that runs backwards.
A close at the hour it opens is refused, since it reads as either no window at all or a whole day.
An hour on its own says nothing about how long a group is down, so recording one offers a close an hour later, which is the length the calendar feed would otherwise have assumed.
That is a starting point and not a claim: it is editable, and a plan whose window nobody has settled records no close at all.

A group has at most one open plan.
A group moves to one place next, so a second plan replaces the first rather than queueing behind it, and the replaced plan is retained as history.

An open plan's date, time, and note can be amended, and an amendment records who made it and when.
A corrected date or a reworded note is the same plan better described, so it stays one plan rather than entering the history as a second.
Changing the target is not an amendment: where a group is going is what the history exists to record, so a new target replaces the plan as any other second plan would.
A plan that has been met or replaced is history and is no longer amendable.

Plans are managed through the operator interface and are audited.
Withdrawing a plan says the group is no longer going there; it does not say the upgrade happened.
A withdrawn plan is retained and records who withdrew it and when: a group that was going somewhere and stopped is part of the same history as one that arrived, and it frees the group to be planned somewhere else.

A planned date is a plan, not a deadline.
Canopy schedules nothing on it, and a date that passes changes only how the plan is presented.

## When a plan is met

Canopy decides a plan is met, rather than asking anyone to mark it done.
A plan is met once the group's reported version has reached its target, at which point the plan closes and records when.

Reaching a version *past* the target also meets the plan: a group that jumped further than planned has done the upgrade and then some, and holding the plan open would misreport it as outstanding.

A met plan is retained.
The record of what a group planned, when it planned it for, and when it actually landed is the fleet's upgrade history, and it is what makes "how long do our upgrades really take to happen" answerable.

## What reads a plan

Pre-upgrade migration testing takes its target from the open plan, and a group with no plan is not tested at all.
Recording a plan is what asks for the testing, and no restore is spent on a version nobody intends to apply.

An upgrade run on a production environment reads the open plan as its permission and is refused without one (see [INV](inventory.md), "Planned upgrades").
It reads that a plan exists and not its day, so recording the plan is what permits the run, whichever night the deployment moves.

A plan changes what is tested, so changing one invalidates nothing already recorded: earlier verdicts stand against the versions they named, and the new target simply becomes the one that has not been tested yet.

Declaring maintenance reads the open plan for its prefill: an operator starting the work is offered a window carrying the plan's window and note (see [MNT](../monitoring/maintenance.md)).
The plan is read at the moment someone declares, so a planned hour arriving suspends nothing on its own.

## The dashboard

Canopy presents planned upgrades across the fleet in one view, so the question "what is moving, and when" is answered without reading each group.

For each group with an open plan it shows the target version, the version the group is on now, the planned date and window where there are ones, and the pre-upgrade verdict for that target, so an operator sees both the intent and whether the group's data survives it.
The window carries its zone, abbreviated: whose midnight it is is the whole question a reader has.
Where an attempt is under way it shows that too, since a restore takes hours and a verdict of not-yet-tested otherwise looks the same whether the pipeline is working or has stopped.
Where nothing is declared to migrate the group's data it says so in place of the verdict, since a plan on a group with no such declaration is never dispatched and a reader would otherwise wait on a result that cannot arrive.
Groups with no plan are shown too, behind a disclosure that counts them: an unplanned group several minors behind is what this view exists to surface, and the count surfaces it without the list crowding out what is moving.

A plan whose date has passed without being met is presented as late, judged on the day alone rather than the hour.
Late is a presentational state and not an incident: an upgrade slipping is normal operational reality, and Canopy has no basis for treating a date someone typed as a failure of anything.

Alongside the table the same view presents the fleet's plans as a calendar, read as a month, a week, or a single day.
The month answers which week is busy and which two groups land on the same night; the week and the day place each plan on an hour grid, where the length of a block is how long the group expects to be down and a window running past midnight is drawn again on the following morning rather than clipped at it.
A day in the month opens that day on its own.
An entry on the calendar amends the plan it names, so a date moved by a week is moved where it is read.

The same view presents the plans that have closed, most recently closed first, so what a group planned before is readable beside what it plans now.
Each shows where it was going, the date and hour it was planned for, and how it closed: met, replaced by a later plan, or withdrawn with the operator who withdrew it and when.
A withdrawn plan is otherwise unreadable anywhere, since a group that stopped going somewhere leaves no other mark on the fleet.

## The calendar feed

Canopy publishes planned upgrades as an iCalendar feed a calendar application subscribes to, so the fleet's intent appears where people already look to see what a day holds.

The feed is served by the internet-facing interface rather than the operator one, because the calendar services people use fetch a subscription unattended from their own infrastructure and cannot reach the operator network.
Access is carried by a secret in the URL: a calendar application has no way to be asked for a credential, so holding the URL is what grants the read.

There is one feed at one URL, configured with the Canopy instance rather than minted per subscriber.
The audience is everyone who works on the fleet, and the link is posted once where they can all read it, so a URL each would be bookkeeping against a credential they already share.
The dashboard offers the URL for copying, and rotating it is a configuration change every subscriber then resubscribes to.
The secret is the one already gating the public server-versions page: the same fleet information, to the same audience, over the same open path.
Where none is configured the feed serves nothing, since an ungated one is an open read of the fleet's plans.

The calendar carries the plans that name a day, while a plan is where the group is going and after it has been met.
A replaced or withdrawn plan leaves the calendar, since the group is no longer going there; a met one stays as the record of what landed.

An entry names the group and the version it is going to, and carries what the plan records beyond the day: the version the group is on now and the note, or for a met plan the day it landed.
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
