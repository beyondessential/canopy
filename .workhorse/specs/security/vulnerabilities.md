---
id: VUL
---

# Vulnerabilities

Canopy tracks the published vulnerabilities in what the production fleet runs: the dependencies of the Tamanu versions it runs, and the Linux packages installed on its boxes.
Vulnerability data comes from OSV, the open vulnerability database.
Each vulnerability is an item for operators to triage (see [VTR](vulnerability-triage.md)).
Vulnerabilities are presented on a page of their own and file no checks, so they raise no issues and no incidents (see [CHK](../monitoring/checks.md)).

## Sources

There are two sources of exposure, and every item belongs to one of them.

**Tamanu** is the dependencies of Tamanu versions.
A Tamanu version's dependencies are the packages in the lockfile at that version's release tag in the Tamanu repository, leaving out those the lockfile marks as development-only.
Every Tamanu application on a version shares that version's dependencies, whatever its type, so a package used by only one part of Tamanu counts against every Tamanu application on that version.
For each package Canopy also holds where it sits in Tamanu's dependency tree: which of Tamanu's workspaces reach it, and whether any of them depends on it directly.
A release tag does not move, so a version's dependencies are read once.

**Linux** is the packages installed on machines, as each machine reports them (see [PKG](../public-server/packages.md)).
A machine that has not reported its packages has no Linux exposure.
An application on a cluster has no box of its own, so it has Tamanu exposure and no Linux exposure (see [K8S](../monitoring/kubernetes.md)).

## What counts

Only production counts as exposure.
A Tamanu version is scanned when a live production application runs it, and a machine's packages are scanned when it is a live production machine (see [GRP](../servers/groups.md), "Environments").
Anything at another rank, pending, or archived exposes nothing.

The newest published Tamanu version is scanned as well, whether or not anything runs it, so what an upgrade would bring can be triaged before production moves to it.

Exposure is worked out from the fleet as it stands, so ranking, archiving or moving an application or machine changes what is exposed without waiting for a rescan.

## Matching

Canopy asks OSV which advisories affect each package version it holds, and keeps each matching advisory's record.
The question is asked once per distinct package version, however many versions or machines carry it.
Canopy asks again on a regular schedule, since advisories are published and revised after a version is first scanned, and promptly when a new version comes to count or a machine reports a changed package list.
An advisory's record is fetched again only when OSV reports it has been revised.

An item is one advisory within one source, identified by the advisory's OSV identifier and carrying its aliases, such as a CVE identifier.

An item's severity is the qualitative rating the advisory carries, from its CVSS score where it has one and otherwise the rating its database gives: critical, high, medium or low.
An advisory with neither is unrated, and ranks below low.

## Exposure

A Tamanu item exposes every live production application on a version whose dependencies include an affected package version.
A Linux item exposes every live production machine with an affected package version installed.

An item is listed while something counts for it: an application or machine it exposes, or the newest published Tamanu version carrying an affected package version.
An item nothing counts for any more leaves the list of open items without needing a decision, and returns if something comes to count for it again.

## Presentation

Vulnerabilities are a top-level section of the operator interface.

The list is one row per item, showing its severity, its summary and identifier, the affected package and its source, how many applications or machines it exposes, and its triage status (see [VTR](vulnerability-triage.md)).
An item that is open and has a suggestion shows the suggested outcome under its status.
The list shows open items by default, sorted by severity and then by how much each exposes.
It filters by source and by status, and a switch includes items nothing counts for any more.

An item's page shows the advisory's summary, details and affected ranges with the versions that fix it.
A Tamanu item lists the exposed Tamanu versions, each with the version of the package it carries, the workspaces that reach it, and how many applications run it; the newest published version is listed whether or not anything runs it.
A Linux item lists the exposed machines, each with its group and the version of the package installed.
Below that is the item's triage history, and beside it the decision panel (see [VTR](vulnerability-triage.md), "Presentation").
