# OSV vulnerability data and triage: test cases

## Sources and exposure

- [x] A Tamanu version's dependencies are the lockfile's packages at its release tag, with development-only packages left out (verifies spec: VUL)
- [x] Each package records the workspaces that reach it and whether any depends on it directly (verifies spec: VUL)
- [x] A version's lockfile is read once; a rescan does not refetch it (verifies spec: VUL)
- [ ] A version run only by non-production, pending or archived applications is not scanned (verifies spec: VUL)
- [x] The newest published Tamanu version is scanned with nothing running it, and its items show zero applications exposed (verifies spec: VUL)
- [ ] A non-production or archived machine's packages expose nothing (verifies spec: VUL)
- [x] Re-ranking an application out of production removes its exposure without a rescan (verifies spec: VUL)
- [ ] An application on a cluster has Tamanu exposure and no Linux exposure (verifies spec: VUL)
- [x] An item nothing counts for leaves the open list, and returns when something counts for it again (verifies spec: VUL)
- [x] Each distinct package version is queried once however many versions or machines carry it (verifies spec: VUL)
- [x] OSV query results past a page are fetched through to the end (verifies spec: VUL)
- [x] An advisory is refetched only when OSV reports a newer revision (verifies spec: VUL)
- [x] Severity comes from CVSS where present, else the database's rating; unrated ranks below low (verifies spec: VUL)

## Package reporting

- [x] A machine's report replaces its previous package list (verifies spec: PKG)
- [x] An identical report changes nothing and records no audit entry (verifies spec: PKG)
- [x] A changed report is audited as a routine action (verifies spec: PKG)
- [x] A device with no machine is refused (verifies spec: PKG)
- [x] Ubuntu LTS and non-LTS releases and Debian releases are placed in the right OSV ecosystem (verifies spec: PKG)
- [x] A report from an unplaceable distribution is kept, matches nothing, and the machine flags it (verifies spec: PKG)
- [ ] The public API change passes the semver check against the previous client crate

## Range evaluation

- [x] SEMVER ranges with `introduced`, `fixed`, `last_affected` and `limit` events evaluate as OSV defines
- [x] ECOSYSTEM ranges for Ubuntu compare in Debian version order, including epochs and `~`
- [x] Explicit `versions` lists match exactly

## Triage decisions

- [x] Not affected needs a reason; accepted risk needs a reason and a review date; will fix takes an optional reason and issue link (verifies spec: VTR)
- [x] A Tamanu decision covers a later Tamanu version carrying a package version inside the decision's ranges (verifies spec: VTR)
- [ ] A Linux decision covers every machine with an affected package version (verifies spec: VTR)
- [x] Recording a decision in the operator interface needs an admin and a write-mode session (verifies spec: VTR, SAFE)
- [x] Replacing a decision keeps the earlier one in the history (verifies spec: VTR)
- [x] An accepted risk past its review date is open again and shows why (verifies spec: VTR)
- [x] A revision raising severity reopens a decided item (verifies spec: VTR)
- [x] A revision widening the range to reach a counted package version reopens a decided item (verifies spec: VTR)
- [x] A revision that only adds a fixed version leaves the decision standing (verifies spec: VTR)
- [x] Decisions and reopenings are audited with the right actor; suggestions are not (verifies spec: VTR)

## Model triage

- [x] A new item is looked at once; a reopened item is looked at again (verifies spec: VTR)
- [x] A not-affected answer on a listed ground becomes the model's decision (verifies spec: VTR)
- [x] A not-affected answer naming no listed ground is kept as a suggestion (verifies spec: VTR)
- [x] Any other outcome is kept as a suggestion, and the item stays open (verifies spec: VTR)
- [x] On an item a person has decided before, the model only suggests (verifies spec: VTR)
- [x] With no model configured, nothing is looked at and new items stay open (verifies spec: VTR)
- [x] An invalid model reply leaves the item with no suggestion

## Fleet query interface

- [ ] Find vulnerabilities filters by source, status and minimum severity, and says when truncated (verifies spec: MCP)
- [ ] Get vulnerability returns exposure with dependency paths, history and suggestion (verifies spec: MCP)
- [x] Recording a decision on the operator path needs an administrator (verifies spec: MCP)
- [x] Recording a decision with a token needs the triage grant; a token without it is refused with no effect (verifies spec: MCP)
- [x] A decision recorded over the interface is attributed to its caller and shown as made through it (verifies spec: MCP, VTR)
- [x] A decision recorded over the interface can replace an operator's (verifies spec: MCP)
- [ ] A token minted without the grant cannot gain it, and the token list shows grants (verifies spec: MCP)

## Operator interface

- [x] Vulnerabilities appears as a top-level sidebar section (verifies spec: VUL)
- [x] The list shows open items by default, sorted by severity then exposure, with source and status filters and a show-all switch (verifies spec: VUL)
- [x] An open item with a suggestion shows the suggested outcome under its status (verifies spec: VUL)
- [x] A Tamanu item's page lists exposed versions with package version, workspaces and application count (verifies spec: VUL)
- [x] A Linux item's page lists exposed machines with group and installed version (verifies spec: VUL)
- [x] The decision panel is prefilled from the suggestion, so taking it is one step (verifies spec: VTR)
- [x] The history names who made each entry and how (verifies spec: VTR)
- [x] A machine's page shows when it last reported packages and how many (verifies spec: PKG)
