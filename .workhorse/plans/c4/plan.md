# Check list logic on the application page

A target's check list presents its own checks, current, once each, ordered by result then presented name, with a quiet source's checks muted and aged.
Application-typed checks present as `<type>:<check>`.
The reasoning and the live findings behind this are in the working doc at `.workhorse/working-docs/c4/working-doc.md`.

## Design notes

### Current means "in the source's latest report", judged at read time

A check-state carries no "still reported" flag, and `active` means degraded, not current.
Recovery by omission (`public-server/src/statuses.rs`, "Unmentioned closes") only touches checks that were active, so a passing check that disappears from a report is never written again.
In both cases a state no longer reported has a last-seen time earlier than its source's latest report for the target.
A push stamps every check it carries with the push's `created_at`, so the newest stamp per (target, source) is that report: the same clock `Issue::source_freshness` gives reachability.

The comparison allows `REPORT_STAMP_SLACK` (10s) behind the newest stamp.
Pushes stamp one report identically, but `file_check` and test seeds stamp row by row with `now()`, and an exact comparison would drop all but the last of those.
A dropped check falls behind by a whole reporting interval, and an abandoned-grain copy by weeks.

The cost is a one-push lag: the omission close stamps the row with that push's time, so it reads as current until the next push.
Resolving the state on omission was rejected.
It changes what the omission close writes, and it needs a new pass for vanishing passing checks.

The latest-report test applies only to sources that push complete reports.
The reserved sources (`canopy`, `manual`, `kubernetes`) file each check on its own cadence, so for them current means unresolved.
Applying the latest-report test to the relay's `kubernetes` source would drop its slower checks.

### Abandoned-grain states are resolved by migration

The read-time comparison catches frozen application-scope copies of reported checks, but not Canopy's own.
The application-scope `backup-staleness` and `backup-reconcile-recency` are unresolved, and `backup-staleness` permanently counts against its application's health.
The migration resolves every unresolved `canopy` state on an application except those Canopy still files there: reachability, `certificate-expiry`, `certificate-issuance`, `dns-records`, `migration-test` and `reporting-schema`.
Resolving rather than deleting keeps check-detail history and incident membership intact.
It also resolves the `health-broken/*` rows `merge_broken_thread` missed (it only resolved the active ones) and clears `check_name` on all of them, so they stop reading as check-states while staying in issue and incident history.

There is no machine-namespace guard or migration criterion.
A split push may legitimately report a machine-set name such as `memory` under an application (STA, "Transitional unified pushes"); that state is held at application scope in the machine namespace.
The frozen copies of reported machine checks fall out by the read-time comparison instead.

### Quiet is reachability's clock

A source is quiet for a target when its newest stamp is older than the target's `alert_when_down_for`, the test `grade_reachability` uses.
It is computed server-side and sent per check (`quiet`, `last_reported_at`).
Quiet ignores reachability and ingest modes: a source switched off for reachability still has stale data, so its checks are muted too.
So every source reachability names as stale is muted, not the converse (CHK wording follows).
A quiet source's states keep counting towards health at their last effective result.

### Point-in-time view

`consolidated_checks_at` presents the application's own checks only.
A unified push's machine-subject checks sit in the application's status row and are skipped by name.
A split push's application row is taken as given, so a machine-set name reported under the application stays.
A split push records the machine's row and the application's in one transaction, so a machine row from the same source with the same `created_at` marks the push as split.
Quiet is judged against `at` (or now).
This also fixes the snapshot's health, which had counted the machine's checks.

### Separator

`<type>:<check>` is built in two places: `Namespace::qualified_name` (Rust) and `qualifiedCheckName` (`private-web/src/types.ts`).
Policy rule paths (`check.<field>`) keep the dot; they aren't qualified names.
The only input that takes a presented name is the MCP check-doc tool.
`Namespace::split_presented` reads either separator; the MCP tool treats the prefix as a type only when the catalogue holds that type's entry, since `ApplicationType` parses any slug.
A bare name that only matches typed entries is an invalid-params error listing the presented names.

## Build

### Data

- [x] Migration `resolve_abandoned_grain_check_states`: resolve `canopy` application-scope states not filed there any more; resolve and un-name `health-broken/*` rows; close orphaned incidents and cancel their pending Slack opens
- [x] Confirmed the backup checks file at machine and group scope only (`backup/staleness.rs`, `backup/reconcile.rs`, `restore.rs`, `jobs/src/backup/*`)

### Consolidated checks (`database/src/issues.rs`)

- [x] `consolidated_checks_for`: no machine checks on an application
- [x] `checks_at_scope`: current states only (resolved dropped; reported sources by latest report with slack; reserved sources while unresolved)
- [x] `ConsolidatedCheck` carries `last_reported_at` and `quiet`; `subject` removed
- [x] Shared `ConsolidatedChecks::sort`: urgency, presented name, source
- [x] `check_detail_at_grain` (fleet figures) applies the same currency rule
- [x] Health rollups unchanged: an omission close sets `active = false`, so they already agree

### Point-in-time view (`private-server/src/fns/statuses.rs`)

- [x] `consolidated_checks_at`: application's own checks, split/unified handling, quiet against `at`, shared sort
- [x] `machine_latest_per_source_at` kept for figures and split detection; comment updated

### Names

- [x] `Namespace::qualified_name` → `<type>:<check>`, `Namespace::split_presented`, unit tests, doc comments
- [x] `qualifiedCheckName` → `:`, `healthcheckPath.test.ts`
- [x] Rust and e2e assertions on the dotted form updated
- [x] MCP `get_check_documentation` takes the presented name; description and arg docs updated

### MCP

- [x] `get_server` returns the consolidated checks and their rollup
- [x] `find_issues` by application returns the application's own issues (`Issue::list` filter); instructions text no longer says machine checks count towards applications

### UI

- [x] `ChecksTable`: machine plumbing removed; silence fetches skipped for other grains
- [x] `CheckRow`: muted with "last reported Nd ago" when `quiet`; `data-testid="check-row"`, `data-quiet`
- [x] React key from source and qualified name
- [x] `just gen-openapi`; `just typecheck`

### Spec references in code

- [x] Every `CHK#a-machines-checks-present-on-its-applications` reference replaced or removed

### Tests

- [x] Rust: membership, retired thread, reserved sources, quiet, ordering, migration, MCP name parsing and `get_server`
- [x] e2e: machine checks absent from an application, silence scopes offered, quiet rows, current-only, ordering; operator sessions read on the machine page
- [ ] Full e2e suite green
- [ ] `just check-generated` after committing
