---
id: CHK
---

# Check state

Canopy's monitoring is organised around checks: named conditions, each with a current result, reported by sources or determined by Canopy itself.
This spec covers the check-state model — targets, sources, results, policy, and the operator controls over them.
How device reports arrive is the status contract (see [STA](../public-server/statuses.md)); how degraded checks aggregate into incidents is the incident spec (see [INC](incidents.md)).

## Targets

Every check is scoped to exactly one target: an application, a machine, a group, or Canopy as a whole.

Application checks and machine checks both come from sources reporting on them, and from Canopy's own determinations such as reachability.
What separates them is what the check asserts something about: whether the software is serving, or whether the box it runs on has room on its disk (see [FLT](../servers/overview.md)).
Group checks are conditions Canopy determines about a group's control plane, such as backup maintenance health (see [BKJ](../jobs/backup.md)).
Canopy-wide checks are Canopy monitoring its own operation (see [SELF](../private-server/self-alerts.md)).

### A machine's checks present on its applications

Every machine check appears on every application on that machine, marked as belonging to the machine.
An operator triaging an application sees every check bearing on it, its own and its host's, in one list.

There is one filing per machine check however many applications present it, so a degraded machine check contributes one issue at machine scope rather than one per application (see [INC](incidents.md)).
A silence on a machine check is machine-scoped and quiets it everywhere it appears, being one check seen from several places.

Reachability is not presented this way, each grain having its own (see "Reachability").

## Sources

A source is a named reporter of checks, identified by a short string.
Multiple sources may report on the same target, each concerned with part of the system, and each source's reports are independent: a report from one source says nothing about another source's checks.

Two source names are reserved for Canopy itself: `canopy` for conditions Canopy determines on its own (reachability, backup health, key expiry, self-monitoring), and `manual` for conditions raised by operators.
Reports arriving over the device API cannot use the reserved names.

### Source policy

Each source other than the reserved names carries two operator-set modes, global to the source and edited on a dedicated Sources page reached from the check catalog.
Because each mode governs the whole reporter fleet-wide and is changed only rarely, switching either mode is confirmed before it takes effect, with the consequence of the chosen mode spelled out; abandoning the confirmation leaves the policy untouched.

Its **reachability mode** governs how the source's silence bears on the reachability of what it reports on (see "Reachability"):

- `on` — a stale source warns, and all of a target's sources stale is unreachable;
- `quiet` — a stale source raises no warning, but still counts toward unreachable;
- `off` — the source is excluded from reachability entirely.

Its **ingest mode** governs whether the device API accepts the source's reports (see [STA](../public-server/statuses.md)):

- `allow` — reports are ingested normally;
- `ignore` — reports are accepted, but the source's data is discarded before ingestion;
- `deny` — the device API rejects the push.

New sources default to `on` and `allow`. A source that isn't ingested (`ignore` or `deny`) is excluded from reachability regardless of its reachability mode — there is no fresh data to judge it by.

## Names

A check's name is a category, not an instance.
It names the condition being checked, and it is the unit an operator configures once and then reasons about across the whole fleet.

A check's identity is the source that reports it, the namespace it belongs to, and its name.
The namespace is a field of its own beside the name, and it takes one of three shapes.
A check reported for an application belongs to the reporting application's type, so two types reporting the same name are two entries rather than one.
A check reported for a machine belongs to the machine namespace, there being no application type to distinguish it by.
A check from a source Canopy curates itself is flat: those names are Canopy's own and mean one thing fleet-wide.

An entry whose namespace is an application type presents as `<type>.<check>`, and every other entry as its name alone.
That is how an entry reads and not how it is held: the namespace is never concatenated into the name, and an address for a check carries the namespace as a part of its own.

A namespace is derived from where a check was filed rather than asserted alongside it, so a reporter needs no knowledge of the scheme and the two cannot fall out of step.

An address naming only a source and a check name resolves to the one entry it can mean.
Where several entries share that source and name, Canopy asks which was meant rather than picking one.

Anything that varies between instances of the same condition — which backup configuration, which restore intent, which certificate — belongs in the check's detail, and never in its name.
Detail is where policy rules read from, so an operator can grade or silence one instance differently from the rest without a catalog entry for each.
A name that encodes a parameter turns one configurable check into as many entries as there are instances, and a catalog that has to be configured that many times has stopped being a way to get your eyes on things efficiently.

### Checks with instances

Where a target has several instances of one condition, Canopy holds one state for the check, as it does for every (target, source, check).

Each instance is graded through policy on its own, against its own detail, so a rule or silence written for one instance applies to only that instance.
Where it takes more than one field to say which instance this is, the detail carries those fields joined into one as well as separately, because a rule condition matches a single variable and a silence for one instance has to pin all of them.
The check's effective result is then the most urgent across the instances that were not skipped, and its detail carries every instance that is not passing, each with its own result, so an operator can see which ones are in trouble without opening anything else.
Its message names those instances.
The check recovers when no instance is left degraded.

## Results

A check's result is one of, in decreasing order of urgency:

- **failed** — the condition is failing.
- **warning** — the condition is degraded but not failing.
- **broken** — the check itself could not run; the condition is unconfirmed either way.
- **passed** — the condition holds.
- **skipped** — the check deliberately did not run.

Every check has two results: the **observed** result, what the source reported, and the **effective** result, what policy makes of it.
The observed result is always recorded as reported; everything Canopy acts on — issues, incidents, health rollups — follows the effective result.

## Policy

Policy is a transformation of results: for each check it maps the observed result to the effective one.
There is one vocabulary on both sides — policy speaks in results, and what a source is told about its checks is the policy itself, not a projection of it.

Fleet-wide policy lives in a catalog keyed by a check's identity, so an operator grades one application type's check without touching another type's check of the same name.
An entry carries:

- a **ceiling** — the maximum effective result, on the urgency ordering: a ceiling of `failed` changes nothing, `warning` grades failures as warnings, `passed` means recorded but never alerting, and `skipped` additionally tells the source not to bother running the check.
- optional **rules** — conditional transforms evaluated against the check's own detail, the detail the report carries for the target, and the target's effective tags; a rule can move a result in any direction, including upward: a warning graded as a failure, or a pass with a particular detail graded as a warning.
- an **escalates** flag — an effective failure of this check notifies immediately, bypassing incident grace (see [INC](incidents.md)).

A check is registered in the catalog with a ceiling of `warning` the first time it is reported, and is pending operator review; operators confirm or adjust its policy from there, and checks still awaiting review are surfaced for it.
While a check is pending review its effective result is hard-capped at warning — whatever its ceiling or rules would otherwise yield — so a never-vetted check records state but cannot open an incident (see [INC](incidents.md)); reviewing the policy, even a no-op save, lifts the cap.
Canopy's own checks register already reviewed, with the policy their condition warrants instead of the default.

### Scoped policy

Beyond the fleet catalog, a transform can be scoped to a target: per application, per machine, per group, or Canopy-wide.
Transforms apply in order — fleet catalog, then group, then the target itself — each acting on the previous effective result, so the most specific scope has the last word.

The operator interface presents two scoped policies.
The **silence** is a scoped ceiling of `skipped` on one check, recording who silenced and when.
A silenced check keeps recording its observed results; its effective result is skipped, so it raises nothing and counts nowhere.
The model admits arbitrary scoped transforms; surfaces beyond the silence are deliberately not offered yet.

A maintenance window is not one of these transforms: it holds a target's issues out of incidents for a bounded time and leaves their grading alone, so an operator sees what their work is doing (see [MNT](maintenance.md)).

#### Silences follow the event

A check that can be filed at a scope can be silenced at that scope.
The scopes a check can be silenced at are the ones it applies at: its own target, and that target's group.
So a machine's checks are silenced against the machine, an application's against the application, and either against the group they belong to.

This holds at every point a silence is read, and those points must agree: what the consolidated view presents as skipped, what the reporting source is told not to run, and what an incident counts are one answer.
A silence that quiets a check in one of those and not the others is a defect rather than a degree of silencing.

Silencing a check everywhere is not a silence but the check's own ceiling in the fleet catalog, so no scope above the group is offered as one.
A silence is per target and records who set it; a fleet-wide decision is a policy the catalog holds.

A silence names a check, so it names that check's namespace: quieting one application type's check leaves another type's check of the same name alerting.
A silence on a target that is itself of one type needs no namespace stated, the target supplying it.
A group spans several types, so a group silence states which type's check it quiets.

## Documentation

Each catalogued check can carry operator-authored documentation: a single markdown document.
By convention it covers three things — a general description of what the check observes, what each result means (what makes it fail as opposed to warn), and hints for solving a failure — and the editor seeds new documentation with a template of those sections; Canopy attaches no meaning to the document's structure.
Operators author and edit the documentation in the operator UI; it is presented alongside the check wherever its state is presented, and is available over the MCP interface (see [MCP](../private-server/mcp.md)) so agents work from curated knowledge about a check rather than deriving it.
Canopy's own checks ship with their documentation.

## State

For each (target, source, check) Canopy keeps exactly one state: the observed and effective results, the detail the source attached to the check's most recent report, when the check was first and most recently reported, and — while it is degraded — when the current degradation began.
All reported checks are kept, including passing ones, so that "everything reporting this check" is answerable without scanning history.

A state whose effective result is warning or failed is an **issue**, eligible to contribute to incidents.
"Degraded since" is the start of the current unbroken run of degradation; a recovery ends the run, and a later degradation starts a fresh one.

### Stability

Alongside each state, Canopy keeps a bounded stability record, updated as reports arrive.
It is derived from **observed** results — untouched by policy, so operator grading and any noise damping built on the record never feed back into it.
An observation is degraded when the observed result is warning, failed, or broken, and healthy when it is passed; skipped observations carry no signal and are not recorded.

The record holds:

- how many observations the state has received, and how many were degraded;
- the most recent transitions between healthy and degraded, each with when it happened — a bounded ring, so a long-stable check remembers its distant history while a flapping one remembers only its recent churn;
- an hour-of-week profile of how often the check is observed degraded, weighted towards recent weeks, so a load-dependent check (degraded during working hours, healthy overnight) is distinguishable from one degraded around the clock.

From the ring, Canopy derives and presents how often the state has flapped recently and how long its degraded runs and healthy gaps typically last.
The record is presented alongside the check on its check detail page and is available over the MCP interface (see [MCP](../private-server/mcp.md)).
Nothing beyond the record is kept: it is a fixed-size summary per state, not a history.

An effective broken result neither confirms nor clears the check's previous definite result: while broken, the state retains the contribution and degraded-since of its last definite effective result, and the brokenness itself additionally counts as a warning.
A policy rule can grade brokenness differently — up to a failure where not being able to check is itself the failure, or down to a pass where a flaky check runner should not raise noise.
A definite effective result (passed, warning, or failed) ends the broken condition and replaces the retained contribution.

## Reporting semantics

A source's report for a target carries that source's complete current set of checks for it.
Canopy trusts the reporter: a check the source previously reported but omits from its current report has recovered, and its state records that.
Omission by one source never affects another source's checks.

An application omitted from a machine's report is not the same as a check omitted from an application's.
A check that goes away has recovered; an application that goes away has stopped being reported on, and becomes unreachable rather than resolved or removed (see [FLT](../servers/overview.md), "Applications come from reports").

## Reachability

A target is reachable while something is currently reporting about it, and unreachable while nothing is.
Machines and applications each have reachability, computed the same way at each: a machine is reported on by its agent, and an application by the machine that carries it.

Canopy tracks, for each target, the sources expected to report — those that have reported, are not in reachability mode `off`, and whose checks are not all decommissioned — and when each last reported.
It keeps one `reachability` check per target, under the `canopy` source, reflecting how many expected sources are currently reporting within that target's down threshold:

- **passed** when every expected source is fresh;
- **warning** when a source in mode `on` is stale but not every expected source is; the stale sources are named in the check's detail, so an operator sees which reporter went quiet. A `quiet` source going stale never raises this warning;
- **failed** when every expected source is stale — nothing is reaching Canopy — and the target is presented as unreachable. `quiet` and `on` sources count alike here.

A stale source degrades its target rather than silently dropping its checks, so a reporter going quiet is never mistaken for health.
There is no per-source staleness check; the one reachability check carries the full picture.

Nothing derives one grain's reachability from another's.
A machine that goes quiet stops reporting about the applications on it by the same act, so each of them becomes unreachable on its own account under the same rule, and each recovers the same way.
An application whose machine is reporting normally also becomes unreachable if that machine stops mentioning it, which is the same rule reaching a case no derived one could express.

An unreachable target's checks keep their last observed results.
Presenting the target as unreachable is what says those results are no longer current.

Every target presents a reachability check as it currently stands, whether or not a reporter has ever gone quiet: one with nothing stale presents it as passed, and one whose reachability is silenced presents it as skipped.
So the check — and the controls on it — are reachable before anything has gone wrong.
A target's checks as they stood at a past time carry reachability only where it was recorded at that time.

Reachability has no intermediate degrees.
A target is reachable, unreachable, or has never reported, and how long it has been quiet is measured against its own configured threshold rather than any fixed one.
The check's three results grade how much of what should be reporting still is, which is a different question from whether the target is reachable: only the failed result presents it as unreachable, and a warning is a target still reachable with one of its reporters quiet.
Every surface presenting reachability presents those three states and grades them on the target's own threshold, so an indicator and the check behind it are measuring the same thing against the same clock.
A target that has reported at some point presents as unreachable however long ago that was; never reported is for one that has never been heard from at all.

## Liveness and decommissioning

Reachability is a per-target signal about reporters that have gone quiet; check liveness is a fleet-wide signal about a catalogued check that has gone away everywhere.
For each catalogued check Canopy tracks when it was most recently reported on any target.

A catalogued check not reported anywhere for seven days is surfaced to operators as a candidate for decommissioning.
A catalogued check not reported anywhere for thirty days raises a Canopy-wide warning (see [SELF](../private-server/self-alerts.md)).

Both are about a reporter falling silent, so neither counts a check nothing could report.
A check whose namespace names an application type absent from the live fleet is left out of the candidate list and the warning: no report of it is possible, whatever an operator does, so it is an entry whose population has gone rather than a check that has.
The machine and curated namespaces are never left out on this reasoning, their populations being every box and Canopy itself.

Decommissioning is an operator action, never automatic: the candidate list and the Canopy-wide warning surface what has gone away, and an operator decides.
A decommissioned check is retired fleet-wide: its state on every target is resolved, recording decommissioning as the reason, and it then contributes to nothing — not health, not incidents, not reachability.
A source all of whose checks are decommissioned is no longer an expected source, so it drops out of the reachability signal.

If a decommissioned check is reported again it is treated as newly registered — pending operator review, at the warning ceiling — so a resurrected check never silently resumes a retired policy.

## Health rollup

A target's health is derived from the checks currently contributing across all its sources: any effective failure makes it unhealthy; otherwise any effective warning or brokenness makes it degraded; otherwise it is healthy.
Passed and skipped checks, and states that are resolved, snoozed, or decommissioned, do not count against a target.

An application's contributing checks include its machine's, so a box whose disk is filling makes every application on it degraded.

## Presentation

Wherever a target's checks are presented — as they stand now, or as they stood at a past time — all of its sources' checks are shown together, each by its effective result and rolled into the target's health by the same rules used everywhere else.
An application presents its machine's checks among its own, each marked as the machine's.
The detail a source attached to a check is presented with it, attributed to its source.
A past state is reconstructed from the status history.
No surface presents one source's checks in isolation, and none exposes a source's report other than as classified check state.

### One subject per mark

A status mark says one thing about one subject, and which element carries it says what the subject is.

An application's mark carries that application's state alone: healthy, warning where a check is failing but it is overall serving, failing, or never reported.
A machine's mark encloses the marks of the applications on it and carries the box's own state.
So a box carrying two applications is one enclosure holding two marks, and a box carrying one is still an enclosure — an enclosure means nothing on its own, only its contents do.

A mark carries no second encoding for a second subject.
Reachability was once carried alongside health on the application's mark, from when an application and the box it runs on were one record; it is the machine's, and it is on the machine's enclosure.

Severity reads from colour and subject from shape, so a colour means the same thing wherever it appears.
A degraded machine is distinguished from a degraded application, since one affects everything on the box and the other affects one workload.

An incident is on one of a group's environments or on the group itself (see [INC](incidents.md)), so wherever a group's environments are drawn the incident is marked on the one it is on.
A group's own incident has no environment to mark and is carried by the mark for the group.
That mark stands for whichever of its environments is in trouble, so a group with an incident anywhere in it reads as such before any environment is read.
Where several of its environments have an incident at once, the group's mark takes the state of the most serious of them.

A maintenance window is declared over a machine or a group and never over an application, so wherever a box is drawn its enclosure carries the window (see [MNT](maintenance.md)).
Where only applications are drawn, each covered application carries it instead — that is the window's consequence for that application rather than a window of its own.
A window's mark is distinguished from the mark for a target nobody is watching, so deliberate, temporary work does not read as neglect.

## Operator controls

**Silences** are the scoped policy described above.
A machine's and an application's own settings each carry a reachability silence, presented alongside a monitoring switch so the two are read together: with monitoring off no check on that target alerts at all, while with unreachability alerting off every other check alerts as normal and only the target going away is quiet.
That control is the same target-scoped silence of the reachability check reached from the check itself, and each surface reflects what the other did.
A silence on a machine's reachability is offered wherever that machine's state is read, including from the applications on it, so an operator quiets a host expected to be down without first working out which record owns the switch.

**Snoozes** suppress one state until a chosen time, after which it contributes again if still degraded.

**Resolution** is an operator marking one state as dealt with, recording who and why.
A resolved state that degrades again reopens: the resolution is cleared and the state contributes anew.

**Notes** attach free-form operator commentary to a state.

## Manual conditions

Operators can raise a condition directly against any target, under the `manual` source, with a chosen check name, result, and message, and optionally marked as escalating.
A manual condition behaves as a reported check whose reporter is the operator: it stays active until an operator resolves it or raises it again as recovered.

## Monitoring gate

Machines and applications each carry a monitoring switch.
Checks targeted at one that is not monitored are recorded and presented for visibility but do not contribute to incidents.
Canopy's own determinations are made for unmonitored targets just as for monitored ones, so an unmonitored target that has gone away still presents as unreachable and unhealthy — it simply raises nothing.
Group and Canopy-wide checks are not subject to any target's monitoring gate.

A machine's monitoring switch governs the checks targeted at the machine.
It does not silence the applications on it, each of which has a switch of its own.

Because an unmonitored target can present as failing while nothing is being alerted on, every surface presenting its health or reachability as they currently stand marks it as unmonitored.
Its health is presented muted and accompanied by a silenced indicator explaining that alerting is off for it, and its indicator is struck through with a diagonal cut, so the distinction survives at the smallest size it is drawn.
The status legend names the mark.
