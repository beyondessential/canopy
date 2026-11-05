# OSV vulnerability data and triage: test cases

## Sources and exposure

- [ ] A Tamanu version's dependencies are the lockfile's packages at its release tag, with development-only packages left out (verifies spec: VUL)
- [ ] Each package records the workspaces that reach it and whether any depends on it directly (verifies spec: VUL)
- [ ] A version's lockfile is read once; a rescan does not refetch it (verifies spec: VUL)
- [ ] A version run only by non-production, pending or archived applications is not scanned (verifies spec: VUL)
- [ ] The newest published Tamanu version is scanned with nothing running it, and its items show zero applications exposed (verifies spec: VUL)
- [ ] A non-production or archived machine's packages expose nothing (verifies spec: VUL)
- [ ] Re-ranking an application out of production removes its exposure without a rescan (verifies spec: VUL)
- [ ] An application on a cluster has Tamanu exposure and no Linux exposure (verifies spec: VUL)
- [ ] An item nothing counts for leaves the open list, and returns when something counts for it again (verifies spec: VUL)
- [ ] Each distinct package version is queried once however many versions or machines carry it (verifies spec: VUL)
- [ ] OSV query results past a page are fetched through to the end (verifies spec: VUL)
- [ ] An advisory is refetched only when OSV reports a newer revision (verifies spec: VUL)
- [ ] Severity comes from CVSS where present, else the database's rating; unrated ranks below low (verifies spec: VUL)

## Package reporting

- [ ] A machine's report replaces its previous package list (verifies spec: PKG)
- [ ] An identical report changes nothing and records no audit entry (verifies spec: PKG)
- [ ] A changed report is audited as a routine action (verifies spec: PKG)
- [ ] A device with no machine is refused (verifies spec: PKG)
- [ ] Ubuntu LTS and non-LTS releases and Debian releases are placed in the right OSV ecosystem (verifies spec: PKG)
- [ ] A report from an unplaceable distribution is kept, matches nothing, and the machine flags it (verifies spec: PKG)
- [ ] The public API change passes the semver check against the previous client crate

## Range evaluation

- [ ] SEMVER ranges with `introduced`, `fixed`, `last_affected` and `limit` events evaluate as OSV defines
- [ ] ECOSYSTEM ranges for Ubuntu compare in Debian version order, including epochs and `~`
- [ ] Explicit `versions` lists match exactly

## Triage decisions

- [ ] Not affected needs a reason; accepted risk needs a reason and a review date; will fix takes an optional reason and issue link (verifies spec: VTR)
- [ ] A Tamanu decision covers a later Tamanu version carrying a package version inside the decision's ranges (verifies spec: VTR)
- [ ] A Linux decision covers every machine with an affected package version (verifies spec: VTR)
- [ ] Recording a decision in the operator interface needs an admin and a write-mode session (verifies spec: VTR, SAFE)
- [ ] Replacing a decision keeps the earlier one in the history (verifies spec: VTR)
- [ ] An accepted risk past its review date is open again and shows why (verifies spec: VTR)
- [ ] A revision raising severity reopens a decided item (verifies spec: VTR)
- [ ] A revision widening the range to reach a counted package version reopens a decided item (verifies spec: VTR)
- [ ] A revision that only adds a fixed version leaves the decision standing (verifies spec: VTR)
- [ ] Decisions and reopenings are audited with the right actor; suggestions are not (verifies spec: VTR)

## Model triage

- [ ] A new item is looked at once; a reopened item is looked at again (verifies spec: VTR)
- [ ] A not-affected answer on a listed ground becomes the model's decision (verifies spec: VTR)
- [ ] A not-affected answer naming no listed ground is kept as a suggestion (verifies spec: VTR)
- [ ] Any other outcome is kept as a suggestion, and the item stays open (verifies spec: VTR)
- [ ] On an item a person has decided before, the model only suggests (verifies spec: VTR)
- [ ] With no model configured, nothing is looked at and new items stay open (verifies spec: VTR)
- [ ] An invalid model reply leaves the item with no suggestion

## Fleet query interface

- [ ] Find vulnerabilities filters by source, status and minimum severity, and says when truncated (verifies spec: MCP)
- [ ] Get vulnerability returns exposure with dependency paths, history and suggestion (verifies spec: MCP)
- [ ] Recording a decision on the operator path needs an administrator (verifies spec: MCP)
- [ ] Recording a decision with a token needs the triage grant; a token without it is refused with no effect (verifies spec: MCP)
- [ ] A decision recorded over the interface is attributed to its caller and shown as made through it (verifies spec: MCP, VTR)
- [ ] A decision recorded over the interface can replace an operator's (verifies spec: MCP)
- [ ] A token minted without the grant cannot gain it, and the token list shows grants (verifies spec: MCP)

## Operator interface

- [ ] Vulnerabilities appears as a top-level sidebar section (verifies spec: VUL)
- [ ] The list shows open items by default, sorted by severity then exposure, with source and status filters and a show-all switch (verifies spec: VUL)
- [ ] An open item with a suggestion shows the suggested outcome under its status (verifies spec: VUL)
- [ ] A Tamanu item's page lists exposed versions with package version, workspaces and application count (verifies spec: VUL)
- [ ] A Linux item's page lists exposed machines with group and installed version (verifies spec: VUL)
- [ ] The decision panel is prefilled from the suggestion, so taking it is one step (verifies spec: VTR)
- [ ] The history names who made each entry and how (verifies spec: VTR)
- [ ] A machine's page shows when it last reported packages and how many (verifies spec: PKG)
