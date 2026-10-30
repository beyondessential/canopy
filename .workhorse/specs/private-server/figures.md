---
id: FIG
---

# Reported figures

A machine's and an application's detail views, and their point-in-time status snapshots, each present a row of figures describing what they are running, and a fleet view presents how those figures are spread across the fleet.
The figures are derived from the detail their sources report (see [STA](../public-server/statuses.md)), not from anything an operator enters.

## Sourcing

Canopy keeps each source's detail as that source's current report on the target: an ingested push replaces what the source previously reported for it, and reports from other sources on the same target are untouched.
Canopy's own generated statuses carry no reported detail and leave a target's reports unchanged.
A source's current report is kept for as long as the target exists, so a figure remains available however long the target has been quiet, and is discarded with the target.

The application version is the exception to a report replacing what came before: a report that carries no version keeps the version that source last reported.
An agent omits the version when it cannot read it — the application is down, or mid-upgrade — which says nothing about what the application is installed to run.

Several sources report on one target, and they do not all report the same figures.
Each figure is taken from the most recent source to report that figure, rather than from whichever source pushed most recently.
So a figure holds its last reported value when the newest push comes from a source that does not carry it, and two figures presented together may come from different sources reporting at different times.

A figure that has never been reported is omitted rather than presented empty.

## Machine figures

The platform is the operating system the machine runs, as it reports it, qualified by the operating system version where one is reported.
A machine that reports no operating system falls back to the family the database engine reported by an application on it gives away, which distinguishes Windows from anything else but nothing finer.

The operating system timezone is the machine's own configured timezone, presented so an operator can read the machine's local time.
It is named for what it is, an application's own configured timezone being a separate figure.

The hostname is the machine's own name as it reports it, which is a different thing from the DNS names an application serves at.

The bestool version is the version of bestool, the first-party agent that reports on the machine, as it reports it.
A machine reported on only by sources other than bestool presents no bestool version.

The machine's hardware and capacity — its processor count, memory, filesystems, uptime and addresses — present as reported.

## Application figures

The application version is presented with how far behind the latest published version it is, and with the minimum embedded browser version that release requires.
Both accompany the version only for an application whose type has a tracked release train, and how much of the version figure an application presents at all follows from its type (see [APP](../servers/application-types.md)).

The database engine version and the runtime version are presented as reported.
Each is an application's own even though both run on the machine, because each application has its own.
When no source reports the runtime version, it falls back to the runtime named by the connection metadata of the identity reporting on the application's machine.

The application's configured timezone presents as reported, and can differ from the timezone its machine's operating system is set to without either being wrong.

A central presents how many mobile devices synced to it in the last 30 days, as reported, zero included, since zero says nobody uses mobile there.
An application whose sources report no such count presents none, which is not the same as zero.

## Fleet spread

The fleet view presents how each figure is spread across the fleet: for every distinct value, how many report it, ordered by how many, and which they are.
A machine figure spreads over machines and an application figure over applications, so a box running two applications is one machine in a platform spread.
Targets reporting no value for a figure are counted together as a group of their own, so the size of the unreported population is visible rather than hidden by omission.
The view covers every live machine and application; archived ones and canopy itself are not part of the fleet.

The spread of the application version, and any crossing with it as an axis, cover only applications whose type has a tracked release train (see [APP](../servers/application-types.md)).
One excluded that way is absent from the spread rather than counted among those reporting no value, and a crossing drops it from both axes rather than placing it in the unreported row.

Each version figure is spread at the grain the fleet moves in rather than at the grain it reports: the application's release branch, and the database engine's own major version.
The exact versions those group are figures in their own right, available like any other field, so an operator who needs the patch level can still reach it.
The view leads with the coarse groupings, since a spread over exact versions splits the fleet finer than an operator can act on.

A spread can be ordered by its values instead of by how many report each, without changing which values a large spread collapses.
The values compare as whatever they look like: as numbers where they are numbers, component by component where they are versions, and as text otherwise.
The unreported group stays last in either order, being a population rather than a value.

Beyond the figures, an operator can name any field a source reports and see its spread the same way.
The fields the fleet currently reports are offered as suggestions, so an operator can find a field without knowing its name in advance.
A field whose values are near-unique across the fleet presents its largest groups with the remainder collapsed, rather than a line per target.

A field a source reports on one of its healthchecks rather than against the target itself is named through the check that reports it, as `check.field`, and spreads across the fleet the same way.
A check's own graded result is available as one of those fields, so the fleet spread of a check's outcome reads like any other field.
What presents here is what the target's own check list presents: a silenced check reads as skipped, and a decommissioned check doesn't present at all.

## Crossings

An operator can cross two fields, presenting a table of one against the other: for each combination of values, how many report both, with the targets behind each combination available.
So "which platforms is this Tamanu version running on" is one crossing rather than a join an operator does in their head.

A crossing counts machines, whatever figures are on its axes.
One unit for every crossing reads more clearly than a unit that changes with what an operator picked, and the view names the unit it is counting.

A machine whose applications disagree on an application figure has a value in more than one cell and appears in each, so a crossing's cells can sum to more than the fleet.

Targets reporting no value for either field occupy their own row and column, so a combination is never silently dropped.
The rows and columns order the same two ways a spread does, and the crossing opens on the coarse version figures against each other.

## Active versions

The status view summarises which release branches the production fleet is actively running: how many distinct branches, which they are, and the range of exact versions across them.
The summary covers the production applications whose type has a tracked release train (see [APP](../servers/application-types.md)).

A production application counts once, at the version the most recent source to report one gave — a source reporting no version does not drop it from the summary by having pushed last.
Unlike the figures elsewhere, this summary is bounded by recency: an application that has not reported within the last week is not running anything as far as the summary is concerned, so a decommissioned one that was never archived stops inflating the count.

## Point in time

The status snapshot presents the same figures as of the moment being viewed, from each source's most recent report at or before that moment.
Reconstructing a moment reads the status history, which is bounded: a source silent for the thirty days before that moment contributes nothing to the snapshot.
