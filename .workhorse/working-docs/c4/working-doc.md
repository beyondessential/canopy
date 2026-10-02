---
status: draft
---

# Check list logic on the application page

Work out what the application page's list of checks does today, then decide what it should show and why.

## What the code does today

The page is `ServerDetail.tsx`, which hands `detail.checks` from `fleet/applications/get_detail` to `ChecksTable`.
The list is built in `consolidated_checks_for` / `checks_at_scope` in `crates/database/src/issues.rs`:

1. Read every row in `issues` filed against the application (exact match on all four scope columns) that has a `check_name` and an `effective_result`.
   No filter on `active`, `resolved_at`, `ref`, or how recently the row was updated.
2. Drop rows whose `(source, namespace, check)` has no live catalog row (decommissioned or orphaned).
3. Apply silences (application-scope plus group-scope, `ceiling = skipped`): a silenced row's effective result becomes `skipped`.
4. If no `canopy/reachability` row exists, add a synthetic passing one.
5. If the application sits on a machine, repeat steps 1 to 3 for the machine's own rows, without reachability, and append them marked `subject: machine`.
6. Sort by effective-result urgency, then `source`, then bare `check` name.

The headline health is computed separately (`health_from_check_state`), and that query *does* filter `active = true AND resolved_at IS NULL`.
So the list and the headline draw on different sets of rows.

`ChecksTable` shows the first five rows with a "Show N more" button, keyed in React by `subject:source:check`.

`active` means "degraded right now", not "current".
A passing check-state is `active = false`, so the list has to include inactive rows to show passing checks at all.
The list has no column that says a row is still being reported.

## What that produces on Central (ba4a5754…, tamanu-central)

Live payload as of 2026-10-02: 63 rows, headline unhealthy (from `sync_facility_stale` failed).

### Box checks listed twice

Every machine-subject check appears twice: once as `subject: application` and once as `subject: machine`.
These are `caddy_version`, `caddyfile_version`, `canopy_registration`, `disk_free`, `external_users`, `memory`, `tailscale_config`, `time_sync`, `billing_tags`, `btrfs`, `caddy_resolvers`, `held_captures`, `inodes` and `load`: 14 checks, 28 rows.
The application-scope copies were frozen when machine checks moved to the machine grain (the 2026-08-26 `machine_scoped_issues` migration and the later split of reported detail by grain).
They never update again.
`disk_free` shows it: 64% used on the application copy, 66% on the live machine copy.
Reporting semantics treat a check missing from the application's report as recovered, so these copies sit at `passed` with stale detail indefinitely.
Nothing retires them, and the catalog gate keeps them because the machine-namespace catalog row is live.

### `caddy_version` listed three times

There are two application-scope rows, both with empty detail, plus the live machine row.
The uniqueness key is `(application_id, source, ref)`, not `check_name`.
The `issues_check_state` migration backfilled `check_name` from both `health/<check>` and `health-broken/<check>` refs.
`merge_broken_thread` resolved the `health-broken/*` rows but left their `check_name` set.
The list doesn't filter resolved rows, so the retired broken-thread row still presents as a second state for the same check.
The CHK spec says "exactly one state" per (target, source, check).
The two rows also share a React key (`application:alertd:caddy_version`), which React rejects and can render wrongly.

### A source that stopped reporting still shows green

`tamanu-central.tasks` (source `tamanu`) shows passed.
The reachability detail says the `tamanu` source has been quiet for about 46 days (`stale_secs: 3991248`).
Its last result stays on the list unchanged, with nothing marking it as old.

### Reachability silenced while failing

`reachability` is observed failed, because of the quiet `tamanu` source and an `alertd` source quiet for about two hours.
It is silenced, so it presents as skipped.
The page header still reads "up", from `last_status`.
The silence is the only thing hiding the `tamanu` outage, and the list gives no hint of that.

### Backup checks frozen at the old grain

Backup checks have been filed at machine scope since the backups moved to the machine grain (2026-09-01): `Scope::Machine` throughout `database/src/backup/staleness.rs` and `reconcile.rs`.
The application-scope `backup-staleness` (observed failed, effective warning) and `backup-reconcile-recency` (observed warning, held at passed by its ceiling, no detail) are frozen copies from before the move.
Canopy's own checks have no report to be omitted from, so nothing ever closes them.
`backup-staleness` is active and unresolved, so it **counts against the application's headline** as a warning, permanently.
On Central the headline is already unhealthy for another reason, so it doesn't show here.

`tamanu-central.kopia_backup` (passed, no detail) is most likely a check `alertd` stopped reporting before the detail column existed.

### Ordering looks random

Rows are sorted by bare check name but displayed by qualified name.
So `tamanu-central.caddy_certs` sorts between `caddy_resolvers` and `caddy_version`, and the `tamanu` source's row sorts after every `alertd` and `canopy` row of the same result.

## Behaviour

Decided 2026-10-02.

### The list carries the application's own checks only

The application page lists the checks filed against the application, and none of its machine's.
Machine checks count towards the machine's health, not the application's, and the list exists to explain the application's headline.
Presenting the box's checks among the workload's was meant to make state easier to read in one place, and in practice it hasn't.
The machine's checks are read on the machine page, which the application page already links to ("Machine: This box").

This reverses CHK's "A machine's checks present on its applications" section and the matching line under "Presentation", which change at the split.

### The list carries current states only

Each check presents as its source most recently reported it, as one row, and nothing else is on the list.
These are off it:

- resolved states, including the retired `health-broken/*` threads;
- states left at a grain Canopy no longer files them at: the application-scope copies of machine checks and of the backup checks;
- states a source has dropped from its report (probably `kopia_backup`).

### A quiet source's checks are greyed and aged

When a source hasn't reported on the target within the target's down threshold, its checks stay on the list at their last result.
They are greyed out and read "last reported N ago", so a stale pass can't pass for a live one.
"Quiet" uses the same per-source clock and threshold as reachability, so the greyed rows and the reachability check always agree.
On Central that greys `tamanu-central.tasks` (46 days) and, at the moment of reading, every `alertd` row (about two hours).

### Health keeps counting a quiet source's last result

A greyed row still counts towards the headline exactly as its last result does.
Last known bad stays bad until the source reports otherwise.
That the source is quiet is reachability's to say, not the rollup's.
A state left at an abandoned grain is not a quiet source, though: it is resolved (see below) and counts for nothing.

### Plain bugs fixed regardless

- Sort by the displayed qualified name, not the bare check name.
- Key React rows on source and qualified name, which are unique once each check is one row.

## Implementation options

### Dropping machine checks from the application list

`consolidated_checks_for` loses the step that appends the machine's `checks_at_scope`.
The point-in-time path (`consolidated_checks_at` in `private-server/src/fns/statuses.rs`) must match, since CHK holds the live and past views to the same rules.
In `ChecksTable`, the application case loses the `machineId` prop, the `list_for_machine` silence fetch, `fromMachine` row targeting and `MachineCheckChip`.
`ServerDetail.tsx` stops passing `machineId`.
The `subject` field on `ConsolidatedCheck` has no remaining reader on this path; drop it if nothing else needs it (private API only).
The e2e coverage for the machine chip on an application goes, and a test that a machine check *doesn't* appear on the application replaces it.

`external_users` is a machine check, so its formatted session list leaves the application page with it.
The "N operators in the machine right now" headline comes from `last_status.operators`, not the list, so it stays.

### Knowing whether a state is in its source's latest report

Recovery by omission (`public-server/src/statuses.rs`, "Unmentioned closes") only touches checks that were *active*.
A passing check that stops being reported is never touched again.
Either way, a state not in the latest report has a last-seen time earlier than its source's latest report for that target.
The per-(target, source) last-report times reachability already reads (`expected_sources` in `database/src/statuses.rs`) give that comparison without a new column.

Chosen: compare at read time, leaving filing as it is.
The cost is a one-push lag: on the push that closes an omitted active check, the close stamps the row with that push's time, so it shows as passed until the next push.
Rejected: also resolving the state on omission.
That would make "current" just "unresolved", but it changes what the omission close writes (and CHK's reporting-semantics wording), and it would need a new pass to close passing checks that vanish.

This comparison only works for reported sources.
Canopy's own determinations (reachability, backup checks) aren't reports, so for them "current" means unresolved.

### Resolving states at abandoned grains

This one is required, not just tidying.
The read-time comparison catches the frozen copies of reported machine checks, but not Canopy's own: the application-scope `backup-staleness` and `backup-reconcile-recency` stay unresolved, and `backup-staleness` still counts against health.
A migration resolves, with a reason naming the grain move:

- application-scope states in the machine namespace;
- application-scope states for every check in the backup sphere that is now filed at machine scope;
- any `health-broken/*` row not already resolved.

Resolving rather than deleting keeps their history on the check detail page and incident membership intact.
A read-time guard in `checks_at_scope` skips machine-namespace rows on an application target too, in case ingest ever files one there again.

Needs checking: nothing still files a backup or machine-subject check at application scope, including for an application hosted on a cluster.

### Ageing data on the wire

`ConsolidatedCheck` carries no timestamp.
It needs the state's last-reported time and a quiet flag, computed server-side against the target's threshold so the UI doesn't redo the reachability maths.
That is a private-API change (`just gen-openapi`), so it doesn't need the public-API compatibility rules.

## Open questions

- [x] Which rows belong on the list? Current states of the application's own checks.
- [x] Do machine checks present on the application? No.
- [x] How does a quiet source's check present? Greyed, with "last reported N ago".
- [x] Resolve on omission, or compare at read time? Compare at read time.
- [x] Does a quiet source's last failed state keep counting against health? Yes.
- [x] Frozen states at abandoned grains: resolved by migration, plus a read-time guard.
- [x] Machine and cluster pages share `checks_at_scope`, so they get the current-only and ageing rules too.
- [ ] Sort within a result: by qualified name alone, or grouped by source?
- [ ] Should reachability's silence on Central be revisited? It is hiding a 46-day `tamanu` outage. That's an operational matter rather than part of this card, but someone should know.
