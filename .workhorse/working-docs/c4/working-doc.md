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

### Other leftovers with empty detail

`tamanu-central.kopia_backup` (passed) and `canopy/backup-reconcile-recency` (observed warning, effective passed) have no detail.
They look like states from before the detail column or the backup check renames that have stopped being written.
Not yet confirmed which.

### Ordering looks random

Rows are sorted by bare check name but displayed by qualified name.
So `tamanu-central.caddy_certs` sorts between `caddy_resolvers` and `caddy_version`, and the `tamanu` source's row sorts after every `alertd` and `canopy` row of the same result.

## Behaviour

Decided 2026-10-02.

### The list carries current states only

A target's check list presents each check as its source most recently reported it, and nothing else.
A state is current when it is in its source's latest report for the target, or, for Canopy's own determinations (reachability, the backup checks), when it is unresolved.
These are off the list:

- states that are resolved, including retired `health-broken/*` threads;
- the frozen application-scope copies of machine checks, which a machine's own rows supersede;
- states the source has dropped from its report (e.g. `kopia_backup`, if that is what it is).

One check presents as one row, so the list has no duplicate keys and no duplicate names.

### A quiet source's checks are greyed and aged

When a source hasn't reported on the target within the target's down threshold, its checks stay on the list at their last result.
They are greyed out and read "last reported N ago", so a stale pass can't pass for a live one.
"Quiet" uses the same per-source clock and threshold as reachability, so the greyed rows and the reachability check always agree.
On Central, that greys `tamanu-central.tasks` (46 days) and, at the moment of reading, every `alertd` row (about two hours).

### Plain bugs fixed regardless

- Retired `health-broken/*` rows no longer present (covered by "current only").
- Sort by the displayed qualified name, not the bare check name.
- The React key can't collide once each check is one row. It still needs to be built from the qualified name and subject, not the bare name.

## Implementation options

### Knowing whether a state is in its source's latest report

Recovery by omission (`public-server/src/statuses.rs`, "Unmentioned closes") only touches checks that were *active*.
A passing check that stops being reported is never touched again.
Either way, a state not in the latest report has a last-seen time earlier than its source's latest report for that target.
The per-(target, source) last-report times reachability already reads (`expected_sources` in `database/src/statuses.rs`) give that comparison without a new column.

One subtlety: on the push that closes an omitted active check, the close itself stamps the row with that push's time, so for one push it reads as current.
Two options:

- **Read-time comparison only.** Accept the one-push lag: the row shows as passed until the next push, then drops.
- **Resolve on omission.** The unmentioned-close pass also resolves the row, with "no longer reported" as the reason, so "current" is just "unresolved". This changes what the omission close writes, so the CHK reporting-semantics wording needs to change with it.

### Clearing the frozen machine-check copies

- A migration resolves (or deletes) application-scope states whose namespace is the machine namespace.
- A read-time guard in `checks_at_scope` for an application target skips machine-namespace rows, in case ingest ever files one there again.

Needs checking: ingest never files a machine-subject check at application scope, including for an application hosted on a cluster.

### Ageing data on the wire

`ConsolidatedCheck` carries no timestamp.
It needs the state's last-reported time and a quiet flag, computed server-side against the target's threshold so the UI doesn't redo the reachability maths.
That is a private-API change (`just gen-openapi`), so it doesn't need the public-API compatibility rules.

## Open questions

- [x] Which rows belong on the list at all? Current states only.
- [x] How should a check whose source has gone quiet present? Greyed with "last reported N ago".
- [ ] Resolve on omission, or compare against the source's last report at read time?
- [ ] Should the frozen copies be resolved by migration, deleted, or only filtered?
- [ ] Should the "current only" rule also apply to the health rollup? It already excludes resolved and inactive rows, but a stale *failed* state from a quiet source still counts against health.
- [ ] Machine and cluster pages share `checks_at_scope`. They get the same rules unless there's a reason not to.
- [ ] Confirm what `kopia_backup` and `backup-reconcile-recency` are on Central: dropped from reports, or something else.
- [ ] Sort: by qualified name within a result, or grouped by subject (application, then machine)?
