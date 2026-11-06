---
id: VUL
---

# Vulnerabilities

Canopy tracks the published vulnerabilities in the dependencies of the repositories it tracks, and in the Linux packages installed on its production boxes.
Vulnerability data comes from OSV, the open vulnerability database.
Each vulnerability is an item for operators to triage (see [VTR](vulnerability-triage.md)).
Vulnerabilities are presented on a page of their own and file no checks, so they raise no issues and no incidents (see [CHK](../monitoring/checks.md)).

## Sources

There are two kinds of source, and every item belongs to one source.

**A repository** is one of the repositories Canopy tracks: Tamanu, Canopy, bestool, seedling, infra, Tupaia, bliti, workhorse, pgro and pollen.
A repository's dependencies are the packages in its npm and Cargo lockfiles on its main branch.
A repository can carry several lockfiles, such as infra's npm dependencies for Pulumi and its Cargo dependencies for pgsnap, and all of them count.
Only the main branch counts, whatever is released or deployed from it.

**Linux** is the packages installed on machines, as each machine reports them (see [PKG](../public-server/packages.md)).
A machine that has not reported its packages has no Linux exposure.
An application on a cluster has no box of its own, so it has no Linux exposure (see [K8S](../monitoring/kubernetes.md)).

Only production boxes count for Linux: a machine's packages are scanned when it is a live production machine (see [GRP](../servers/groups.md), "Environments").
Anything at another rank, pending, or archived exposes nothing, and ranking, archiving or moving a machine changes what is exposed without waiting for a rescan.

## Where a package sits

For each package in a repository, Canopy holds how main reaches it:

- the workspace packages that reach it, where a repository is a monorepo (npm workspaces, Cargo workspace members); a repository that is one package has that one;
- whether each of them depends on it directly;
- the chain of packages by which each reaches it;
- the scope of each path.

A path's scope names when the package is used, in the terms SPDX gives a dependency's lifecycle: **runtime**, **build**, **development**, **test**, **design** or **other**.
A path can also be **optional**, meaning the package may not be installed.
Each ecosystem supplies the scopes it distinguishes, and a path never takes one it does not: npm distinguishes runtime, development and optional dependencies, so a package only a build tool needs is development there; Cargo distinguishes runtime, build and development dependencies.
A package reached through paths of different scopes carries each of them.
Development and build paths count like any other: a flaw in a build tool can corrupt what it builds, so the scope is something to weigh in triage and not a reason to leave the package out.

A repository's lockfiles are read again whenever its main branch moves.

## Matching

Canopy asks OSV which advisories affect each package version it holds, and keeps each matching advisory's record.
The question is asked once per distinct package version, however many repositories or machines carry it.
Canopy asks again on a regular schedule, since advisories are published and revised after a version is first scanned, and promptly when a repository's main branch moves or a machine reports a changed package list.
An advisory's record is fetched again only when OSV reports it has been revised.

An item is one advisory within one source, identified by the advisory's OSV identifier and the source, and carrying the advisory's aliases, such as a CVE identifier.
The same advisory in two repositories is two items, each triaged for how that repository uses the package.

An item's severity is the qualitative rating the advisory carries, from its CVSS score where it has one and otherwise the rating its database gives: critical, high, medium or low.
An advisory with neither is unrated, and ranks below low.

Where the advisory names a version that fixes the flaw, an item carries the lowest such version above each affected package version, its minimum fix version.

## Exposure and closing

A repository item counts while main carries an affected package version.
A Linux item counts while a live production machine has an affected package version installed, and exposes each such machine.

An item nothing counts for is **closed**.
Nothing about closing needs a decision: when a fix lands on main, or a machine upgrades past the flaw, the item closes by itself.
A closed item keeps its decisions and history, and is listed only when asked for.
If something comes to count for it again, it returns to the triage status it had, subject to the reopening rules (see [VTR](vulnerability-triage.md), "Reopening").

A repository that cannot be read leaves its items as they were, and does not close them.

## Presentation

Vulnerabilities are a top-level section of the operator interface, and the sidebar entry carries the number of open items.
Closed items are not counted, and an item that is decided is not open.

The list is one row per item, showing its severity, its summary and identifier, the affected package with its source and scopes, and its triage status (see [VTR](vulnerability-triage.md)).
A Linux item also shows how many machines it exposes.
An item that is open and has a suggestion shows the suggested outcome under its status.

Every column sorts, ascending or descending.
Without a sort chosen, the list is ordered by severity and then by how recently the advisory was published.

The list shows open items by default.
It filters by source, by status (open, not affected, accepted risk, will fix, mixed, and closed), by scope, by severity and by whether it has a suggestion, and each filter's choices show how many items they would give with the other filters and the search applied.
The filter bar shows how many items are in the result as it stands, with every filter and the search applied.

A search box matches partially and tolerates small typos, across an advisory's identifier and aliases, summary, affected package name, the workspace packages that reach it, and source name.
It also searches the advisory's full details, where a match counts for less than one in the fields above.
While a search is entered, rows are ordered by how well they match until a column is chosen to sort by.

An item's page shows the advisory's summary, details and affected ranges with the versions that fix it.
A repository item lists the affected packages on main, each with its version, its minimum fix version, the workspace packages that reach it with the scope of each path and whether directly, and the commit of main it was read at.
A Linux item lists the exposed machines, each with its group and the version of the package installed.
Below that is the item's triage history, and beside it the decision panel (see [VTR](vulnerability-triage.md), "Presentation").
