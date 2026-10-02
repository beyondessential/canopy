# Check list logic on the application page: test cases

## Membership

- [ ] An application on a machine lists none of the machine's checks; the machine's page lists them (verifies spec: CHK)
- [ ] A resolved check-state is not listed on its target (verifies spec: CHK)
- [ ] A retired `health-broken/<check>` row beside a live `health/<check>` row presents the check once (verifies spec: CHK)
- [ ] A check its source omits from its latest report drops off the list from the next report on (verifies spec: CHK)
- [ ] A passing check that a source stops reporting, while the source keeps reporting others, drops off the list (verifies spec: CHK)
- [ ] An application-scope state in the machine namespace is never listed on the application, even if one is filed there (verifies spec: CHK)
- [ ] An unresolved `canopy`-source state (reachability, a backup check at its own grain) stays listed regardless of report times (verifies spec: CHK)
- [ ] A target that has never been quiet still shows its reachability check as passed (verifies spec: CHK)
- [ ] Machine and cluster pages follow the same membership rules (verifies spec: CHK)
- [ ] The point-in-time view for an application lists its own checks only (verifies spec: CHK)
- [ ] Fleet figures read check fields only from states the target's list presents (verifies spec: FIG)

## Abandoned-grain migration

- [ ] After migrating, an application-scope `backup-staleness` that was active is resolved and no longer counts against the application's health (verifies spec: CHK)
- [ ] After migrating, application-scope machine-namespace states and unresolved `health-broken/*` rows are resolved, with the grain move as the reason
- [ ] An incident whose only live members were resolved by the migration is closed without a Slack resolve
- [ ] Resolved states remain readable on the check detail page

## Quiet sources

- [ ] A check whose source last reported beyond the target's down threshold is muted and reads "last reported Nd ago" (verifies spec: CHK)
- [ ] A check whose source reported within the threshold is not muted (verifies spec: CHK)
- [ ] The sources whose checks are muted are exactly the stale sources named in the target's reachability detail (verifies spec: CHK)
- [ ] A quiet source's last failed state still counts against the target's health (verifies spec: CHK)

## Ordering

- [ ] Checks order by effective result, most urgent first (verifies spec: CHK)
- [ ] Within a result, checks order by presented name: `caddy_version` before `tamanu-central:caddy_certs` (verifies spec: CHK)
- [ ] The point-in-time view orders the same way (verifies spec: CHK)

## Names

- [ ] An application type's check presents as `<type>:<check>` on the check list, healthcheck catalogue, check detail title, settings title, silenced-refs list, self-alerts and issue list (verifies spec: CHK)
- [ ] Machine and flat checks present as their bare name (verifies spec: CHK)
- [ ] Policy rules addressing `check.<field>` still parse and evaluate

## MCP

- [ ] Get check documentation with `tamanu-central:caddy_certs` returns that type's entry (verifies spec: MCP)
- [ ] Get check documentation with `tamanu-central.caddy_certs` returns the same entry (verifies spec: MCP)
- [ ] Get check documentation with `disk_free` or `reachability` returns the entry (verifies spec: MCP)
- [ ] Get check documentation with a bare name that only matches application-typed entries errors, listing the presented names (verifies spec: MCP)
- [ ] A check name containing a separator whose prefix is not an application type is read whole (verifies spec: MCP)
- [ ] Get application returns its own checks only (verifies spec: MCP)
- [ ] Find issues filtered by application returns only the application's own issues (verifies spec: MCP)

## UI

- [ ] No React duplicate-key warning on an application with leftover duplicate rows
- [ ] The application page has no "machine" chip on any check row
- [ ] Silencing a check from the application page offers application and group scopes only
