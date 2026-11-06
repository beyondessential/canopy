---
id: MCP
---

# Fleet query interface

A query interface to the Canopy fleet, exposed for AI agents and other automated clients that operators run.

## Why it exists

Operators increasingly investigate the fleet through AI agents rather than by hand.
The operator web UI answers these questions for a human, and the operator API answers them for the web UI, but neither is a surface an agent can discover and call on its own: the agent would have to be told each endpoint and its request shape out of band.
This interface gives an agent a single, self-describing entry point: it advertises the questions it can answer and the inputs each takes, so a client can enumerate the available queries and call them without prior knowledge of the fleet's internals.

## Access and identity

The interface speaks the Model Context Protocol over HTTP, and is reachable over two paths with distinct authentication.

On the operator-facing surface, alongside the operator API and web UI, it is served at `/api/mcp`.
Every caller there is identified by a tailnet user identity, the same identity the rest of the operator surface uses.
That path is available to any human tailnet user, not only to administrators, and is not reachable by tagged automation devices.
Its writes are available only to administrators.

On the internet-facing surface it is served at `/mcp`, for agents that operators run outside the tailnet.
A caller there authenticates with a bearer access token in the standard authorization header.
Device identity plays no part on this path: a device credential grants no access to the interface, and an access token grants no access to the device API.
Requests without a valid token are refused without revealing whether the token is unknown, revoked, or expired, and repeated failures from one origin are rate limited.

Each query a caller makes is attributable to its identity: the tailnet user on the operator path, the token's name on the token path.
A write is recorded as its caller's: an operator's on the operator path, the token's on the token path.

## Access tokens

Access tokens are minted, listed, and revoked by administrators on the operator surface.
Each token has an operator-chosen name, records who minted it and when, and carries a fixed lifetime of one year from minting that cannot be extended or chosen at mint time.
The token secret is shown exactly once, at minting; the system persists only a digest of it, and never logs or re-displays the secret.
A token can be revoked at any time, taking effect immediately.
Each token records when it was last used, so idle tokens are visible.

A token carries the grants chosen when it is minted, which cannot be changed afterwards; a token minted without one never gains it.
The **triage grant** allows the token to record, withdraw and edit vulnerability triage decisions.
A token is minted with no grants unless the administrator chooses one, so every token reads only unless minted to write.
The token list shows each token's grants.

From fifteen days before a token's expiry until that token expires or is revoked, a [self-alert](self-alerts.md) is raised, so rotation happens on schedule rather than as an outage.

## Reads and writes

Most of the surface is reads, as that's the safe set. Writes are deliberately added where it makes sense, with some safeguards:
- A write is refused to a caller not allowed it, without effect: on the operator path, a caller who is not an administrator; on the token path, a token without the triage grant.
- Each write is audited as the action it takes, recorded the same way as when it is taken in the operator interface (see [AUD](../platform/audit-log.md)).

## Queries

The interface advertises a fixed catalogue of named queries, each with a described set of inputs and a structured result.
A client can enumerate the catalogue and read each query's description and inputs before calling it.
Results carry resolved human-readable labels (such as group, machine and application names) alongside the identifiers they correspond to, so a result is legible on its own without a follow-up lookup.

Machines and applications are distinct throughout: a query about what a box is doing answers about a machine, and a query about what software is running answers about an application (see [FLT](../servers/overview.md)).
A result naming one carries the identifier of the other where the two are related, so a client can move between them without a search.

### Discovery

**Find applications** takes an optional free-text term (matched against name, URL, or identifier) and optional filters for type, rank, and group, plus a flag to include archived applications.
It returns a bounded list of matching applications in compact form — identifier, name, URL, type, rank, owning group, the machine it runs on, when each was last seen, its last known version, and its current health.
An application whose type has no version carries no version here (see [APP](../servers/application-types.md)).
When the result is truncated to its bound, the result says so, so the client does not mistake a partial list for the whole.

**Find machines** takes an optional free-text term (matched against name, reported hostname, or identifier) and optional filters for group, platform, and whether the machine is cloud-hosted, plus a flag to include archived machines.
It returns a bounded list of matching machines in compact form — identifier, name, reported hostname, platform, owning group, how many applications run on it, when each was last seen, and its current health — and says so when truncated, as the application search does.

**Find groups** takes an optional free-text term and returns the matching groups, each with its live member count, its effective version, the highest rank among its members, its backup configuration state, and when it last backed up.

### Detail

**Get application** takes an application identifier and returns the full record for one application: its own fields (type, rank, name, public name, URL, tags), its reported figures (version, database engine version, runtime version, configured timezone), its overall health and per-check health, its reachability, when it was last seen, its owning group, the count of its siblings in that group, and the machine it runs on.
Its per-check health is its own checks as the operator UI presents them (see [CHK](../monitoring/checks.md), "Presentation"); its machine's checks are read with **Get machine**.

**Get machine** takes a machine identifier and returns the full record for one machine: its own fields (name, group, whether it is cloud-hosted, geolocation, tags), its reported figures (platform, operating system timezone, hostname, bestool version, processor count, memory, filesystems, uptime, addresses), its overall health and per-check health, its reachability, when it was last seen, the applications running on it in compact form, which backup types it is capable of, and for each its effective schedule with the layer it comes from, its next scheduled backup, and its most recent successful backup.

Backup capability and history are the machine's rather than any application's, so a box hosting two workloads reports one set (see [BAK](../public-server/backup.md)).

**Get group** takes a group identifier and returns the full record for one group: its own fields, its members in compact form with each member's version and health, its backup configuration, per-type schedules, and its machines' schedule overrides, its repository statistics, its recent backup and maintenance activity, and when its repository was last inspected.

### Versions

**List versions** optionally includes drafts and returns the known Tamanu versions, each with its number, release status, head release date, a changelog summary, and how many live applications currently report running it.

**Get version** takes a version and returns its detail: its changelog, its known issues, the later versions available as updates from it, and which applications and groups currently run it.

### Planned upgrades

**List upgrade plans** takes no input and returns where every environment is going (see [UPG](upgrade-plans.md)): each open plan, named by its group and environment, with the version that environment runs now, the version it plans to move to, the planned date where there is one, and whether that date has passed unmet.
Separately it returns each group's headline environment that runs behind the newest published version with nothing recorded, and how far behind it is, since pre-upgrade testing does not cover it.

**Get upgrade plan history** takes a group identifier and returns every plan that group's environments have had, newest first, each with the environment it was for, its target, planned date, note, and how it stands: open, met, replaced by a later plan, or withdrawn.
It returns the group's environments alongside, each with what it runs now and how far behind the newest published version that is.
A plan carries the operator who recorded it, who last amended it, and who withdrew it, with the times of each.

### Fleet triage

**Fleet summary** takes no input and returns a fleet-wide overview: application counts by type and rank, machine counts, the distribution of deployed versions, a rollup of application health, the number of groups, and a rollup of backup health.

**Find backup problems** optionally narrows to one group, otherwise scans the whole fleet, and returns the current backup problems graded by urgency: machine-and-type pairs whose last successful backup is overdue against its schedule, types that have never reported a backup, groups whose backup repository is in an error state, recent failed backup runs, and maintenance runs that appear stuck.

### Incidents and issues

An issue is a degraded check state (see [CHK](../monitoring/checks.md)): a condition on an application, a machine, a group, or Canopy itself, reported by a known source under a stable check name, carrying its observed and effective results and a current active state; an incident aggregates the issues active on a target over a span of time, from when it opened until it closes or an operator resolves it (see [INC](../monitoring/incidents.md)).

**Find incidents** takes a look-back window (in days, defaulting to a week) and optionally one group, and returns the incidents that were open at any point within that window — those still open, plus those that closed no earlier than the window start.
Each is returned with its target, its status (open, closed, or operator-resolved), when it opened, closed, and was resolved, who resolved it and why, whether it ever escalated, how long it was open, whether it was published, and how many issues it covers.
A status filter can narrow the result to only open or only resolved incidents.

The window includes incidents that flapped open and shut within their group's grace period and so never surfaced to anyone.
Each incident therefore carries a **published** flag — true when it actually notified operators, which happens only when it stayed open past its target's grace period or it escalated (an escalating check's failure joined, which bypasses the grace) — and the result reports how many of the returned incidents were published.
A summary or ranking of incidents should count published incidents rather than raw rows unless raw activity is explicitly wanted.

**Get incident** takes an incident identifier and returns the incident with the issues attached to it: each issue's effective result, source, check name, message, the scope it is filed at and what that scope names, active state, and when it joined and (if applicable) left the incident.
So an issue about a box reads as the machine's and an issue about software as the application's, and a client can tell which of a group's grains a failure belongs to.

**Find issues** returns issues across the fleet, filtered by active state, by effective result, by group, by machine, by application, and by recency (issues last seen within a look-back window).
Filtering by application returns the application's own issues, matching what that application presents; its machine's are found by filtering by machine.

**Get issue** takes an issue identifier and returns the issue with the incidents it is or was part of.

**Get check documentation** takes a source and a check's presented name and returns the check's operator-authored markdown documentation (see [CHK](../monitoring/checks.md), "Documentation"), which by convention covers what the check observes, what each result means, and how to solve a failure.
The presented name is the one every other tool returns (see [CHK](../monitoring/checks.md), "Names"): `<type>:<check>` for an application type's check, with `<type>.<check>` accepted too, and the bare name for a machine's or Canopy's own.
A prefix is read as an application type only when it names one, so a check name that happens to contain the separator is read whole.
A bare name that matches only application types' checks is an error listing the presented names it could mean, so the client asks again with one of them.
A client investigating an issue consults this before deriving a check's meaning from other sources.

**Get check stability** takes a set of (source, check) pairs — optionally narrowed to one machine, one application, or one group — and returns each matching state's full stability record (see [CHK](../monitoring/checks.md), "Stability"): the observation counts, the transition ring, and the hour-of-week degradation profile, together with the derived flap statistics (recent flap counts, typical degraded-run and healthy-gap durations).
This is the raw material for analysing whether a check's noise is a flap, a load-dependent pattern, or a real change in behaviour.

### Vulnerabilities

An item is a vulnerability within one source, a tracked repository or Linux, as the operator interface presents it (see [VUL](../security/vulnerabilities.md)).

**Find vulnerabilities** takes optional filters and a free-text search, and returns a page of items.
The filters are source, triage status, minimum severity, scope, affected package, workspace package, and whether the item has a suggestion, plus a flag to include closed items.
The search matches as it does in the operator interface, and the order is the one the operator interface gives its list, or by relevance for a search.
Each item carries its identifier and aliases, summary, severity, source, affected packages with their scopes and minimum fix versions, how many machines it exposes for a Linux item, its triage status with the current decision and whether that decision was withdrawn or edited, and the model's suggestion where there is one.
A caller may name the fields it wants with `fields`, and receives only those, with the item's identifier and source always included.
The advisory's long details text is left out unless the caller names it in `fields`.
A result is a page with the number of items matching in all and a cursor for the next page, so the client does not mistake a partial list for the whole.

A caller may also ask for the result `group_by` package, scope, source or status.
The result is then each group with how many items fall in it, and no items, so an agent can size the work and then fetch a group by filtering on it.
An item with several scopes is counted in each, so the groups of a scope grouping can add up to more than the items matching.

**Get vulnerability** takes an item's identifier and source and returns the item in full: the advisory's details and affected ranges with the versions that fix it; for a repository, the affected packages on main with each one's version, the workspace packages that reach it, whether directly, the chain of packages and the scope of each path, and the commit of main it was read at; for Linux, the exposed machines with each one's group and installed version; its decision history; and the model's suggestion with its reasoning and basis (see [VTR](../security/vulnerability-triage.md)).

The model's suggestion is made without access to source code, and every place the interface presents one says so, in the description of the queries that return it and in the suggestion itself.
It is an inference from the advisory and the package's place in the dependency tree, and a caller with the source is expected to check it.

**Record vulnerability decisions** takes a list of items, each an identifier and source with a decision, whole or split, carrying the fields its outcome requires (see [VTR](../security/vulnerability-triage.md), "Decisions").
A decision may replace any current decision, an operator's included, and the history keeps the one it replaced.
Each item is judged on its own: those that are valid are recorded and those that are not, such as an unknown item or a split that leaves a workspace package uncovered, are refused with the reason, each audited as its own decision.
The result lists the outcome for every item in the order given.
The decisions are presented as made through this interface by their caller.

**Withdraw vulnerability decisions** takes a list of items and a reason, and withdraws each item's current decision on the same terms, returning each to open.
**Edit vulnerability decision** takes one item, and corrects the reason or issue link of its current decision (see [VTR](../security/vulnerability-triage.md), "Changing a decision").

All three writes are held by the triage grant.

**Triage vulnerabilities** is a prompt the interface offers, which guides an agent that has the repository's source through triage.
The agent lists open items, groups and orders them as it finds easiest for a person to review, and reads how the code uses each vulnerable package.
It then puts its proposed decisions to the user in batches of ten, each with a reason that says what was checked, and records a batch only once the user has confirmed it, so a person approves every decision.
The prompt tells the agent that the model's suggestions were made without source and are to be checked, and that an advisory it cannot verify from the code is left for a person.

### Audit log

**Find audit entries** takes a look-back window of up to ninety days and optional filters for actor, action, and a record acted on, and an option to include routine actions, which are left out unless asked for or named; it returns matching entries of the audit log newest first, as a bounded list (see [AUD](../platform/audit-log.md)).
Each entry carries when it happened, its actor, its action, the records it acted on, its detail, and, for an entry committed ahead of its action, whether it was done, failed, or is unconfirmed.
It reaches back ninety days and no further, even for entries Canopy keeps longer, and its description says so; older entries are read from the operator interface, which reaches into the archive.
A longer window is refused, naming the limit.

## Result semantics

A machine's or an application's reported status reflects reports received within that target's own down threshold; one silent beyond it reads as unreachable rather than as a stale "up" (see [CHK](../monitoring/checks.md), "Reachability").
An application's last known version is retained without that window, so a long-offline application still reports the last version it ran.

The health classifications this interface reports — a healthy / warning / unhealthy / unreachable state, and a backup's overdue, never-reported, failed, or stuck classifications — are the same classifications the operator web UI presents for the same data.
A client and an operator looking at the same machine, application or group reach the same conclusion about whether it is healthy and whether its backups are in good order.
An application's health is its own checks and a machine's is the box's, so a client and the UI agree on which of the two a box's disk filling is trouble for.

Version adoption counts and the version distribution count live applications by the version each currently reports, so they reflect what is deployed now rather than what has ever been seen.

An incident counts as open within a look-back window if it was open at any point in that window, not only if it opened within it: a long-running incident that opened earlier and is still open, or that closed during the window, is included.
