# Check list logic on the application page: test cases

## Membership

- [x] An application on a machine lists none of the machine's checks; the machine's page lists them (verifies spec: CHK)
- [x] A resolved check-state is not listed on its target (verifies spec: CHK)
- [x] A retired `health-broken/<check>` row beside a live `health/<check>` row presents the check once (verifies spec: CHK)
- [x] A check its source no longer reports drops off the list (verifies spec: CHK)
- [x] A passing check a push leaves out drops off the list at once; a failing one is closed by omission and drops at the push after (verifies spec: CHK)
- [x] A check a split push reports under an application is listed on the application, even one named like a box check such as `memory` (verifies spec: CHK, STA)
- [x] An unresolved reserved-source state stays listed regardless of when it was filed (verifies spec: CHK)
- [x] A target that has never been quiet still shows its reachability check as passed (verifies spec: CHK)
- [x] A cluster's relay checks filed at different cadences all stay listed (verifies spec: CHK)
- [x] Machine and cluster pages follow the same membership rules (verifies spec: CHK)
- [x] The point-in-time view for an application lists its own checks only, for both split and unified pushes (verifies spec: CHK)
- [x] A split push still reads as split in the point-in-time view after the machine has pushed again without the application (verifies spec: STA)
- [x] Fleet figures read check fields only from states the target's list presents (verifies spec: FIG)

## Abandoned-grain migration

- [x] After migrating, an application-scope `backup-staleness` that was active is resolved and no longer counts against the application's health (verifies spec: CHK)
- [x] A Canopy check still filed against applications, and a reported check, are left untouched
- [x] After migrating, unresolved `health-broken/*` rows are resolved and no longer read as check-states
- [x] An incident whose only live member was resolved by the migration is closed
- [ ] Resolved states remain readable on the check detail page

## Quiet sources

- [x] A check whose source last reported beyond the target's down threshold is muted and reads "last reported Nd ago" (verifies spec: CHK)
- [x] A check whose source reported within the threshold is not muted (verifies spec: CHK)
- [x] Every source reachability names as stale has its checks muted (verifies spec: CHK)
- [x] A quiet source's last failed state still counts against the target's health (verifies spec: CHK)
- [x] Canopy's own determinations are never muted (verifies spec: CHK)

## Ordering

- [x] Checks order by effective result, most urgent first (verifies spec: CHK)
- [x] Within a result, checks order by presented name: `caddy_version` before `tamanu-central:caddy_certs` (verifies spec: CHK)
- [x] The point-in-time view orders the same way (verifies spec: CHK)

## Names

- [x] An application type's check presents as `<type>:<check>` on the check list, healthcheck catalogue, check detail title, self-alerts (verifies spec: CHK)
- [ ] An application type's check presents as `<type>:<check>` on the settings title, silenced-refs list and issue list (verifies spec: CHK)
- [x] Machine and flat checks present as their bare name (verifies spec: CHK)
- [x] Policy rules addressing `check.<field>` still parse and evaluate

## MCP

- [x] Get check documentation with `tamanu-central:caddy_certs` returns that type's entry (verifies spec: MCP)
- [x] Get check documentation with `tamanu-central.caddy_certs` returns the same entry (verifies spec: MCP)
- [x] Get check documentation with a bare machine check name returns the entry (verifies spec: MCP)
- [x] Get check documentation with a bare name that only matches application-typed entries errors, listing the presented names (verifies spec: MCP)
- [x] A check name containing a separator whose prefix is not a catalogued type is read whole (verifies spec: MCP)
- [x] Get application returns its own checks only (verifies spec: MCP)
- [x] Find issues filtered by application returns only the application's own issues (verifies spec: MCP)

## UI

- [x] A box-scoped silence does not quiet a same-named application check in the point-in-time view (verifies spec: CHK)
- [ ] No React duplicate-key warning on an application with leftover duplicate rows
- [x] The application page has no machine check rows
- [x] Silencing a check from the application page offers application and group scopes only
- [x] Operator sessions are read on the machine page; the application page keeps the "operators in the machine right now" headline
