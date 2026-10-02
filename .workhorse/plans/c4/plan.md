# Check list logic on the application page

A target's check list presents its own checks, current, once each, ordered by result then presented name, with a quiet source's checks muted and aged.
Application-typed checks present as `<type>:<check>`.
The reasoning and the live findings behind this are in the working doc at `.workhorse/working-docs/c4/working-doc.md`.

## Design notes

### Current means "in the source's latest report", judged at read time

A check-state carries no "still reported" flag, and `active` means degraded, not current.
Recovery by omission (`public-server/src/statuses.rs`, "Unmentioned closes") only touches checks that were active, so a passing check that disappears from a report is never written again.
In both cases a state no longer reported has a last-seen time earlier than its source's latest report for the target.
The per-(target, source) last-report times that reachability reads (`expected_sources` in `database/src/statuses.rs`) give the comparison without a new column.

The cost is a one-push lag: the omission close stamps the row with that push's time, so it reads as current until the next push.
Resolving the state on omission was rejected.
It changes what the omission close writes, and it needs a new pass for vanishing passing checks.

Canopy's own determinations (the `canopy` source: reachability, the backup checks) aren't reports.
For them, current means unresolved.

### Abandoned-grain states are resolved by migration

The read-time comparison catches the frozen application-scope copies of reported machine checks, but not Canopy's own.
The application-scope `backup-staleness` and `backup-reconcile-recency` are unresolved, and `backup-staleness` permanently counts against its application's health.
A migration resolves them, recording the grain move as the reason.
Resolving rather than deleting keeps check-detail history and incident membership intact.
A read-time guard backs it up.

### Quiet is reachability's clock

A source is quiet for a target when its last report is older than the target's down threshold, the same test `grade_reachability` uses.
It is computed server-side and sent per check, so the UI doesn't repeat it.
A quiet source's states keep counting towards health at their last effective result.

### Separator

`<type>:<check>` is built in two places: `Namespace::qualified_name` (Rust) and `qualifiedCheckName` (`private-web/src/types.ts`).
Policy rule paths (`check.<field>`) keep the dot; they aren't qualified names.
The only input that takes a presented name is the MCP check-doc tool.
It accepts `:` and `.`, reads a prefix as a type only when it is a known application type, and errors with the candidates on a bare name that only matches typed entries.

## Build

### Data

- [ ] Migration (`just migration resolve_abandoned_grain_check_states`): resolve unresolved application-scope states in the machine namespace, application-scope states of backup-sphere checks now filed at machine scope, and any `health-broken/*` row still unresolved, with a `resolved_by`/reason naming the grain move. Also close any open incident left with no live members, following `merge_broken_thread`'s pattern
- [ ] Confirm nothing still files a machine-subject or backup check at application scope, including for an application on a cluster; add a test pinning it if one is missing

### Consolidated checks (`database/src/issues.rs`)

- [ ] `consolidated_checks_for`: stop appending the machine's `checks_at_scope` for an application target
- [ ] `checks_at_scope`: present only current states; drop resolved rows; for reported sources, drop rows whose last-seen predates the source's latest report for the target; for `canopy`-source rows, unresolved only
- [ ] `checks_at_scope`: skip machine-namespace rows on an application target (guard)
- [ ] Carry `last_reported_at` and `quiet` on each `ConsolidatedCheck`, judged on the target's down threshold against the per-source clock reachability uses; reuse that lookup rather than re-querying
- [ ] Sort by effective urgency, then `qualified_name`, then `source`
- [ ] Remove `subject` from `ConsolidatedCheck` if nothing else reads it; otherwise update its doc comment
- [ ] Apply the same membership rule to `check_detail_at_grain` (fleet figures), which FIG holds to "what the target's own check list presents"
- [ ] Re-check `health_from_check_state` and its machine and cluster siblings agree with the list's membership: a dropped-from-report state is `active = false` already, so likely no change, but confirm with a test

### Point-in-time view (`private-server/src/fns/statuses.rs`)

- [ ] `consolidated_checks_at`: drop the machine-check merge for an application, and match the sort
- [ ] Remove the `machine_latest_per_source_at` path in `database/src/statuses.rs` if it has no other caller

### Names

- [ ] `Namespace::qualified_name` → `<type>:<check>`; update its unit test and the `check_policies.rs` doc comments
- [ ] `qualifiedCheckName` in `private-web/src/types.ts` → `:`; update `healthcheckPath.test.ts`
- [ ] Update `tests/it/healthchecks.rs` assertions on the dotted form
- [ ] MCP `get_check_documentation`: parse `check_name` as a presented name (`:` or `.`, known-type prefix only); bare name matching only typed entries → error listing presented names; update `CheckDocArgs` and `CheckDocOut` doc comments and the tool description

### MCP

- [ ] Get application: per-check health drops machine checks (follows from `consolidated_checks_for` if it reads through it; confirm)
- [ ] Find issues by application: return the application's own issues only; update the `application_id` filter's doc comment in `canopy-mcp/src/incidents.rs`

### UI

- [ ] `ChecksTable`: drop `machineId`, the `list_for_machine` fetch for application targets, `fromMachine`, `MachineCheckChip`; `ServerDetail.tsx` stops passing `machineId`
- [ ] `CheckRow`: muted row and "last reported Nd ago" (via `TimeAgo`) when `quiet`, per the application check list mockup
- [ ] React key from source and qualified name
- [ ] `just gen-openapi` after the `ConsolidatedCheck` change; `just typecheck`

### Spec references in code

- [ ] Replace every `// spec: CHK#a-machines-checks-present-on-its-applications` with the section that now governs (`CHK#each-check-is-held-at-one-grain` or `CHK#presentation`), or remove it with the code it annotated

### Tests

- [ ] Rust: list membership, ordering, quiet flag, abandoned-grain guard, MCP name parsing (see test cases)
- [ ] e2e: replace the two `machines.spec.ts` tests that assert machine checks on an application with ones asserting their absence and the machine page as where they're read; add muted/aged row and ordering coverage, extending `e2e/seed.ts` for per-source last-report times if needed
- [ ] `just test-package database`, `just test-package private-server`, `just test-e2e` (with `env -u PORT` under the local agent)
