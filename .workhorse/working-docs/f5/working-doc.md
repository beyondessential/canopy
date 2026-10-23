---
status: draft
---

# Test review process (F5 as test bed)

F5 and its stack of PRs added 190 tests (as of `a476f366`).
This doc records what we want from a test-quality review pass and loop, runs the process over F5's tests, and collects what worked, so the process can be turned into a repo skill.

## Why

Agents write tests incrementally alongside incremental features, and nobody steps back.
Three recurring failure shapes, plus one open-ended lens:

1. Trivial or low-value tests, and over-engineered ones.
   The classic: mocking everything around one property so a very small thing costs a large setup and a slow run, when other tests already cover it cheaply.
2. Tests of someone else's behaviour.
   When a dependency is added, agents test what the dependency already guarantees (calling a time library and asserting the format it produces).
   Its authors test it; we don't.
3. Incremental sprawl.
   Many narrow tests accumulated feature by feature, where one general test walking the whole structure would cover the same ground and could not drift.
   In Rust the type system or a macro can force a test to cover every field or variant of a construction, so the compiler fails when the structure grows and the test doesn't.
4. Open-ended consolidation.
   Look at all the tests, including those outside the PR, and suggest ways to reduce the count without reducing coverage.
   Large features may legitimately need many tests, so the lens is "same coverage, fewer tests", not "fewer tests".

## Lenses

Each lens is one review pass over a set of tests. Working definitions, to be sharpened by the walkthrough.

### A. Trivial or over-built

- Asserts something the type system, a constructor or a derive already guarantees.
- Setup dwarfs the assertion (heavy fixtures or mocks for one small property).
- A cheaper test elsewhere already exercises the same property, so this one adds cost and no coverage.
- Question to ask of each test: what bug would make this fail that no other test would catch?

### B. Tests the dependency

- The assertion's truth comes from a library, not from our code.
- Signal: removing our code from the path (calling the library directly) would still pass the test.
- Keep a test only where our code makes a choice about the library (a format string, a configuration, a mapping), and assert that choice.
- Varies by dependency kind (formatting and parsing libraries, serialisation, database drivers, HTTP clients), so the walkthrough should note which kinds produced hits.

### C. Incremental sprawl

- Group tests by the structure they exercise (an enum, a struct, a catalogue of actions, a family of endpoints).
- Where several tests each touch one field or variant, propose one exhaustive test.
- Prefer mechanisms that fail to compile or fail loudly when the structure grows: exhaustive `match` with no wildcard, destructuring a struct without `..`, iterating a variant list the enum itself provides (a macro or strum-style iterator), table-driven cases keyed by the whole set.
- A test that cannot drift beats a test that is right today.

### D. Open-ended consolidation

- Read the PR's tests next to the existing tests for the same area.
- Propose merges, deletions and promotions (a unit test replacing an integration test, or the reverse where the unit cannot see the behaviour), each with the coverage argument for why nothing is lost.
- Each suggestion states what coverage is claimed to be preserved, so it can be checked.

## Process

1. Inventory the tests a card added (below), by PR, by file and by level (Rust unit, Rust integration, Vitest, Playwright).
2. Run lenses A to D over the inventory, one at a time, recording each finding as keep, delete, merge or rewrite with a one-line reason.
3. Walk the findings with the user. Accept, reject, or reshape. Record why for rejections.
4. Apply accepted changes. Run the affected tests by filter (`just test -p <pkg> <filter>`, targeted Playwright spec files only) to confirm nothing broke.
5. Repeat lens D for a second round on what remains, and stop when a round finds nothing worth acting on.
6. Score each lens on signal to noise (below), then write the skill from what held up.

## Inventory (as of the fourth PR)

Method: take each PR's diff and count added test functions.
For the large first PR GitHub refuses the diff (over 20k lines), so diff the merge commit against its first parent.
A test counts as new only if its function or `test(...)` line is new; tests that only had assertions extended are not counted.

| PR | State | Diff range | Added |
|---|---|---|---|
| #666 | merged | `73c085a8^1..73c085a8` | 170 |
| #670 | merged | `652be820^..46c077e2` | 3 |
| #671 | merged | `a64d7ed5^..d7147f9e` | 15 |
| #672 | open | `af82ff3c^..a476f366` | 2 |

By level: 40 Rust unit (inline in `src/`), 90 Rust integration (`tests/it/`), 37 Vitest, 23 Playwright.

By file (added counts):

- `crates/database/tests/it`: `ansible_catalogue.rs` 12, `ansible_leases.rs` 5, `ansible_variables.rs` 1, `maintenance_machine_sets.rs` 12, `maintenance_run_windows.rs` 13, `managed_by_ansible_migration.rs` 1
- `crates/private-server/tests/it`: `ansible.rs` 5, `ansible_import.rs` 14, `ansible_runs.rs` 15, `ansible_variables.rs` 8, `maintenance_machines.rs` 4
- `crates/private-server/src/fns/ansible` (inline unit): `inventory_file.rs` 17, `hosts.rs` 7, `limit.rs` 7
- `crates/jobs/src` (inline unit): `ansible/catalogue.rs` 4, `ansible/leases.rs` 2, `ansible/held.rs` 1, `backup/recovery_snapshot.rs` 2
- `private-web/src/components`: `ansible/yamlScope.test.ts` 16, `ansible/schema.test.ts` 11, `ansible/pyyaml.test.ts` 4, `AnsibleRunDialog.test.ts` 5
- `private-web/e2e`: `ansible.spec.ts` 22 (9 moved from the deleted `inventory.spec.ts`), `maintenance.spec.ts` 1

Renames in #666 are not new tests: five Rust tests changed their inventory vocabulary, and nine e2e tests moved from `inventory.spec.ts` to `ansible.spec.ts` under the same names.
Two e2e tests from `inventory.spec.ts` were dropped.

The open PR kept taking commits while the review ran, so re-derive the inventory against the current tip before each lens rather than trusting a saved count.

Test names are not copied here; regenerate them from the diff ranges above when running a lens.

## Findings

Filled in as the walkthrough runs. One subsection per round, per lens.

### Round 1: lenses A and B over the Rust tests (130 tests)

Run as three read-only agents split by area (database crate, private-server integration, inline unit and job tests), each given the lens definitions above and the diff ranges.
Result: 25 of 130 flagged (4 delete, 6 rewrite, 15 merge into another test), 105 kept.

Applied by four edit agents split by file, then formatted and run by filter (`database` 34, `jobs` 16, `private-server` 121 tests, all passing).
Net 17 fewer tests: 16 of F5's 190, and one older sibling folded into a combined test.
One flagged item was refused on checking: `inventory_file.rs::refuses_what_is_not_an_inventory_naming_where` stays as it was, because the import spec requires a refused file to name where it fails, so the "line" assertion is the only cover of that requirement.
Items verified before acting: `jsonschema` does reject a bad hostname with formats on; every `host_names` call site gets its tailnet flag from the shared `reach()` helper; `ScanError` always prints a line.

Applying found a defect that the review did not: the `a_scopes_history_holds_its_removed_variables…` test (added to #672 that same day) had never passed.
It aged audit entries with an `UPDATE`, which the audit guard trigger refuses once an entry is settled, into a week with no partition.
Folding it into `a_removal_is_listed_at_its_scope_and_can_be_undone` carried the defect along; fixed there by creating that week's partition and lifting the guard for the update.

Lens B (3 findings, all high confidence, two dependencies):

- `jsonschema` validation: the type and range assertions (`"2222"` as a string, `70000` over a maximum, a missing `required` property) are the library's. Our choices are format validation being on, the `{name: value}` wrapping, the path-prefix stripping and the message shape.
  - `database/tests/it/ansible_catalogue.rs`: `holds_a_value_to_its_type_and_format` (rewrite, drop the type and range rows), `names_where_in_a_structured_value_the_fault_is` (rewrite, drop the `missing` half)
  - `private-server/tests/it/ansible_variables.rs`: `a_variable_the_catalogue_describes_is_held_to_it` (rewrite to what is ours: no catalogue means no check, secrecy mismatch both ways, an undescribed name stays free, a violation maps to 400; the rest duplicates the database tests)
- `yaml-rust2`: `inventory_file.rs::refuses_what_is_not_an_inventory_naming_where` asserts the parser's own error text contains "line" (rewrite to `is_err`, unless the spec requires a position).
  `ansible_import.rs::refuses_a_file_that_is_not_an_inventory` repeats the unit tests for the same inputs through the endpoint (delete, keep one assertion that a parse error maps to 400).

Lens A, delete or rewrite:

- `inventory_file.rs::reads_the_variables_under_all_and_on_each_host`: plain happy path, each value asserted in a more specific test (delete, medium confidence)
- `inventory_file.rs::a_secret_field_inside_a_structured_value_is_carried_whole`: `parse` has no secrecy logic, so it only checks a nested mapping becomes JSON (delete; check for a `// spec:` annotation to move first)
- `private-server/tests/it/ansible.rs::names_machines_apart_where_their_first_labels_match`: same shape as the `hosts.rs` unit test, wiring covered by `ansible_runs::names_what_a_run_acts_on_as_the_limit_preview_does` (delete; confirm every `host_names` call site derives the tailnet flag the same way)
- `ansible_runs.rs::previews_the_machines_a_limit_matches`: re-tests the pattern language that `limit.rs` unit tests cover (rewrite to one pattern plus the response shape and error field)
- `jobs/src/backup/recovery_snapshot.rs::notices_when_a_secret_last_changed`: tests a database-crate query from the jobs crate, and only one branch of it; the secret-variable branch is never exercised (rewrite: move to the database crate tests and add that case). This one raises coverage rather than cutting it.

Lens A, merge into another test (15, all "same setup and same gate, one extra assertion"):

- database: `ansible_leases::the_first_read_starts_it_once` into `keeps_the_windows_its_run_declared`; `ansible_catalogue::a_problem_before_any_good_read_leaves_no_catalogue` into `keeps_the_last_copy_that_read_when_a_later_attempt_does_not`; `maintenance_machine_sets::a_set_naming_a_member_refuses_a_take_by_another_operator` into `a_set_is_not_its_environments_own_window`
- jobs: `leases.rs` `releases_a_lease_no_run_read_and_records_it_as_canopys_own` with `notices_a_lease_that_ran_out` (one database, both audit entries)
- private-server: `ansible.rs::refuses_a_read_claiming_another_intent_than_the_leases` as a row in `ansible_runs::refuses_a_read_started_otherwise_than_the_lease_says`; `ansible_import.rs` three tests (`a_machine_already_managed…`, `a_group_wide_variable…`, `leaves_out_the_variables_a_run_sets…`) into their neighbours; `ansible_runs.rs` `a_limited_run_declares_maintenance…` and `serves_the_lease_it_is_read_under` into `a_lease_records_the_run_it_was_taken_for`, and `refuses_an_intent_that_disagrees_with_the_playbook` into the shared refusal helper; `ansible_variables.rs` `refuses_managed_by_ansible_outside_machine_scope` tabled with its sibling, `offers_the_catalogue_it_holds` and `a_scopes_history_holds_its_removed_variables…` folded into neighbouring tests

Groupings the agents noticed for lens C (not analysed yet): `hosts.rs` (all 7 are members in, names or refusal out), `limit.rs` pattern cases, `inventory_file.rs` refusals and comment tests, the `take_lease` refusals and `check_claims` rows in `ansible_runs.rs`, import preview gates, the four variable-history tests, the three stub-driven `catalogue.rs` job tests.

What round 1 showed about the process:

- No test was the "mock everything for one tiny property" shape; these tests do not mock.
  The over-building here is cost: a throwaway migrated database per small property.
  Lens A's useful form is "fold this into the test that already has the setup", which is close to lens D.
- Lens B was rare but decisive, and only for validation and parsing libraries.
- Agents named tests by reading both levels; there is no cheap way to see which lower-level test covers a property. Coverage tooling would answer the "is anything lost" question directly.
- Agents did not check `// spec:` annotations on tests. A deletion or merge must carry the annotation to the surviving test, or the spec loses its trace.
- Three claims were inferred, not checked: `jsonschema` rejecting a bad hostname with format validation on, `ScanError`'s display always including the line, and the `host_names` tailnet flag. Verify before applying the related change.

### Round 1: lenses A and B over the Vitest and Playwright tests (50 tests)

Same method, two read-only agents.
Vitest: 36 reviewed, 3 flagged, no lens B hits.
Playwright: 14 reviewed, 5 flagged, none a delete.
Applied: Vitest by one agent (its four files and `just typecheck` pass), Playwright by another, then the six changed e2e tests run by name (all pass).
Left alone on purpose: the two optional e2e merges (the review itself advised against them), the `it.each` tabling of the PyYAML scalar and `check(...)` tests (lens C), and the whole-component import in `AnsibleRunDialog.test.ts` (a source refactor, not a test change).
Mutation check on the lens E repair: forcing the Tamanu check true in `AnsibleRuns.tsx` makes the repaired install test fail (expected 0, received 1), so it can fail now.

Vitest:

- `yamlScope.test.ts` "reads values as an inventory file would": same resolver path as the next test and `pyyaml.test.ts`; fold its two unique inputs (`x y`, `3`) into the PyYAML scalar test (merge)
- `schema.test.ts` "reads a plain type": only checks a `switch` passthrough that the `problems` tests already depend on; keep as a small table inside the fallback test, which also adds the untested `number` (merge)
- `pyyaml.test.ts` "leaves anything else as text": its inputs (`Pacific/Auckland`, `v2.64.3`) cannot match a numeric rule, so a bug would still pass; use dotted versions without a `v` such as `2.64.3`, and `1:30:99` (rewrite)
- Tidying the agent noticed: the three PyYAML scalar tests and the `check(...)` rejection tests are `[input, expected]` rows (`it.each`); one `not.toContain` is redundant after an exact `toBe`; `typeLabel`'s input would not catch a lost `/g` flag; `AnsibleRunDialog.test.ts` imports the whole component, and so MUI, to reach three pure functions

Playwright (a test here is the dearest kind; the rule is that rendered behaviour no cheaper test reaches earns one):

- "offers install and upgrade only where a Tamanu application is held": the `run-install` count of 0 is asserted straight after `goto`, before the page has loaded, so it likely passes whatever the code does (rewrite: wait for `run-any` to be enabled first)
- "starts work, shows the command, and gives the lease up" and "shows the run started once it has read the environment": trim what `AnsibleRunDialog.test.ts` and the Rust window tests already assert, keeping the clipboard, testids, lease row, poll and Hold form (rewrite, trim)
- "cannot start a run until a catalogue has been read" and "enters a list an item at a time": optional merges into a sibling with the same seed; the agent recommends leaving them (a merge saves a table reset and a page load, and costs failure isolation)
- Not flagged as lenses but real defects: "completes names…" has a negative check with no positive control; "edits a scope as text" says an unchanged variable is left alone, but `toHaveCount(2)` would pass if every variable were removed and re-set; "does not give up a lease whose run has started" races the first 2.5 s poll

What this showed:

- A fifth lens is needed: a test that cannot fail (an assertion true whatever the code does, a negative check with no positive control, a count that cannot tell the thing it claims from its opposite).
  It does not reduce the count; it makes a kept test worth keeping.
- Lens A on end-to-end tests becomes "how much of this repeats the cheaper layer", which gives trims, not deletions.
- Verify a verdict against the spec before applying it: one rewrite was wrong because a spec requirement made the assertion necessary.
- A repair to a test that could not fail is not proven by the test passing; mutate the code under test and watch it fail.
- Applying changes finds defects reading does not: the unrun test above only showed itself when its test was run.

### Round 1: lens C (incremental sprawl), analysis only

Four read-only agents (Rust unit and job tests, private-server integration, database, Vitest and Playwright), about 40 proposals in all.
Nothing applied; three design decisions come first (below).

What lens C turned out to be here:

- Almost none of F5's structures is an enum a test can iterate. Most are strings, inline conditionals, or data-carrying enums, so "fails to compile when it grows" needs a small change before the test can exist: a variant list for the plain enums (`ScopeKind`, `Problem`, `ReleaseReason`, `RunIntent`, `Preset`, `ServerRank`), or a pure function extracted from a handler so it can be tested as a table.
- The repo has no variant-list mechanism. `actions!` exposes names only (`audit::CATALOGUE`, `names::*`), and its variants carry data. Two ways to add one without a dependency: a 12-line `macro_rules!` in `commons-types` emitting `ALL` plus an exhaustive-match guard, or a hand-written `ALL` beside the guard. The guard stops a forgotten variant at compile time; it does not stop a forgotten `ALL` entry unless the macro generates both.
- Most proposals add coverage rather than cut the count: tables that carry the rows nobody wrote, round-trip tests, destructuring a struct without `..`. The count falls mainly where integration tests with identical setup become rows of one table (roughly 25 server or database spins into a handful, many in tests that pre-date F5), and in pure tables over the pattern language and job scaffold.

High value (new drift protection or new coverage, low cost):

- A shared fixture for the two PyYAML scalar tables and the variable-name rules, read by both the Rust tests and Vitest. Today the TS test says "the same cases as the server's" and is not; the Rust table lacks `0b101`, `-12`, `02:30`, `1:30.5`, `1:30:99`, `1:60`, `2.64.3` and the `Yes`/`ON` spellings.
- A `formatScope` then `parseScope` round trip, so opening a scope and saving it unedited changes nothing (may expose a real defect; untested).
- Round-trip tests over the string-coded enums and their SQL CHECKs (`ReleaseReason`, `Problem`, `ScopeKind`), driven by the variant list.
- `check_claims` and `runs::check` split into pure functions that destructure their argument struct without `..`, with a named-row table; endpoint tests keep one refusal each as wiring. The `runs::check` table also gains rows nobody wrote (unknown skip tag, assignment outside the run, configure labelled upgrade).
- The maintenance-window refusal cluster (13 tests, mostly pre-F5) as one table that also asserts `run_state.refuses == (take_lease is 409)` per row, the stated invariant that only 4 of the 13 check.
- Variable-setting rules shared between `set` and the import's inline copy of them (real duplication, different message text), with one unit table over `ScopeKind` and the restricted names.
- Playwright helper extraction (`seedProductionGroup`, `startRun`, and similar): no merging, so no loss of failure isolation. The group-plus-central seed block appears about 19 times.

Medium: a catalogue read compared whole (serde to a JSON literal), which also covers the never-parsed `upgrade` intent and preset; the placement checks over `ScopeKind::ALL`; the lease-state by operation matrix with a `require_held` extracted; environment-resolution and membership tables in `ansible.rs`; import cannot-apply gates as one table; the `schema.test.ts` kinds table via `satisfies Record<...>`; the catalogue job refusals as named rows; lens C for window lifecycle tests (`a_runs_window_over_its_leases_life`, hold and amend as one test with two ways).

Low, or leave alone (agents agreed, with reasons): the `inventory_file.rs` inheritance and comment tests, the `hosts.rs` partition tests, `maintenance_machines.rs` (one endpoint each), the three run-dialog e2e tests (helpers only), the history tests with exact per-machine spans, `managed_by_ansible_migration.rs`, and `refuses_what_is_not_a_catalogue` (add rows, do not restructure).

Possible defects the review surfaced (all unverified until read and run):

- `no_longer_held` keys on `released_by`, not the release reason: a lease the sweeper released as unstarted has `released_by = None`, so a late read or extend is told it expired. No test reads or extends a swept lease.
- `held_secret_name` adds `-held` to a name already near the 63-character limit (61 for an environment at `production`), and the test hard-codes "production is the longest rank".
- Whether `format` in a catalogue schema is declared but never enforced by the SPA (declared in `Schema`, excluded from the keyword table).
- `AnsibleLeaseTakenOver` and `AnsibleLeaseExtended` audit entries are never asserted.

Decisions (settled): a `macro_rules!` in `commons-types` emits each enum's `ALL` and an exhaustive-match guard together; extract a pure function from a handler only where it removes duplication (so the shared set and import variable rules, not `runs::check` or `check_claims`); one JSON fixture shared by the Rust and TS PyYAML and variable-name tests; verify the suspected defects, then apply the high tier, then revisit the medium tier after lens D.

Suspected defects, checked:

- Confirmed and fixed: a lease released unread by the sweeper has no `released_by`, so a late read was told it had expired. The refusal now names the reason; the existing take-over test also covers it, and fails without the fix.
- Not a defect: `held_secret_name` is 61 characters at `production` (limit 63). Only the test is thin: it covers `secret_name` alone and hard-codes the longest rank. Fixed by looping `ServerRank::ALL` over both names.
- Not a defect: `format` is declared in the SPA's `Schema` but not pre-checked; the server enforces it on set, so a bad value is refused with its message.
- A gap, not a defect: the take-over and extend audit entries are never asserted.

Lessons for the skill:

- Lens C needs the code under test, not just the tests: the drift-proof form depends on whether the structure is a value, a type, or a string, and the review has to say which.
- Judge a merge by whether the tests share a structure, not a setup. The agents kept splitting "same setup" (fold them) from "same structure" (table them), and refused to merge ordered state sequences.
- Lens C generates findings about production code (duplicated rules, a mismatched error check, a length limit); the skill needs a place to hand those off rather than burying them in test edits.
- Cost is overstated for Playwright here: the stack is worker-scoped and CI runs one worker, so a test costs its seed, table reset and page load. Prefer helper extraction to merging.

### Round 2: lens C high tier applied

Five edit agents split by file ownership, then formatted and run by filter: `database` 55, `commons-types` 113, `jobs` 16, `private-server` 114 (filtered), Vitest 448, typecheck, `just check` with no warnings, and the whole `ansible.spec.ts` plus the maintenance spec (23 e2e).

What landed:

- `all_variants!` in `commons-types` (emits `ALL` and an exhaustive-match guard from one list) on `ServerRank`, `ScopeKind`, `Preset`, `Problem`, `RunIntent`, `ReleaseReason`. Round-trip tests for each, plus one test per CHECK-backed enum that stores every variant through the real API and reads it back. `ScopeKind` and `Preset` have no public parser, so they go round through `Catalogue::parse`; visibility was not widened.
- One JSON fixture of 105 PyYAML scalar rows read by both the Rust resolver and Vitest, and a `formatScope` then `parseScope` round trip over every row. No disagreement and no round-trip failure was found, so the feared defects did not exist.
- `variable_problems` shared by `set` and the import, with a 32-row name fixture read by both languages. `set_value` now validates before checking the scope exists, so a catalogue violation against a missing scope is a 400 where it was a 404. The scope wording in messages is now per scope. The import never called the machine-only `managed_by_ansible` rule, but could not reach it; sharing the function makes that structural.
- The maintenance-window refusal cluster (12 tests, mostly older than F5) became one table of 11 named rows, each probing `run_state` and `take_lease` and asserting they agree, which only a few of the old tests did. Missing claim and `runs::check` rows were added to existing table-shaped tests.
- Playwright helper extraction in `ansible.spec.ts` (986 to 817 lines, six helpers, no merged tests, one duplicated assertion removed).

Effect on numbers (static count of Rust test functions): the total is unchanged at 2395, but 10 server or database backed tests became 10 pure unit tests (1900 to 1890 and 495 to 505). The count is the wrong measure for lens C; what moved is cost per test and the number of cases each test walks.

Left for the medium tier, after lens D: the placement table over `ScopeKind::ALL`, the lease state matrix, environment membership and resolution tables, import cannot-apply gates, the schema kinds table, catalogue job refusals, window lifecycle tests, and the `check_claims` and `runs::check` extractions (declined: they remove no duplication).

What round 2 showed about the process:

- The mechanism is the work. Without a variant list the drift-proof form cannot exist, so the review has to propose the smallest helper first and the tests second.
- A shared fixture across two languages is a stronger drift guard than either side's table, and costs one file; it also deletes a comment that had gone false.
- Applying in parallel needs file ownership stated in each brief; the agents' compile errors in each other's files cleared on their own, but only because each was told to wait and not touch them.
- Reporting a test count hides the effect. Report cost classes (database or server backed versus pure) and cases walked.

### Round 3: lens D, first round (analysis)

Four read-only agents (database and maintenance model, private-server endpoints, inline unit and job tests, Vitest and Playwright), each reading the current tests next to their neighbours and building a behaviour map by layer.
Each suggestion had to name the surviving test and a coverage claim that can be checked.
Nothing applied yet. A `just coverage` recipe (cargo-llvm-cov over the contained RAM Postgres) was added so the Rust claims can be checked by diffing line coverage of the F5 source files before and after; Vitest has no coverage provider and Playwright has none, so web claims rest on mutation checks.

What came back:

- The agents agree on the small folds (DB and jobs): the three direct window hook tests into the lease wiring test, `says_why_a_lease_was_released` into the wiring test (its `Released` half repeats the enum round trip), the two catalogue alert tests into one sequence, three constraint tests into one, two `maintenance_run_windows` pairs, `maintenance_machine_sets` merges (a, b, c), the two enum store tests, the jobs catalogue stub tests. About 15 tests, each saving a database spin, none dropping an assertion.
- Web: delete the machine-window run-holder e2e test (the SPA reads the window and `refuses`, never its kind; the endpoint table owns the kinds), move one text assertion into `groups.spec.ts`, fold the one-environment picker test into the ranked-machine test, fold the secret tail of the row-edit test into the secret test, drop a redundant Vitest name assertion. Folding the round-trip `it.each` pair and the three one-assertion `checkScope` tests is optional.
- The endpoint tests are the large remaining cost, and the agents propose scenario tests: several groups in one database, each scenario a row, failures collected (the precedent is `a_window_refuses_a_lease_exactly_where_the_page_reports_it`).
  Read-only families are safe to fold: import previews (11 to 2), environment-resolution refusals (8 to 1), served-hosts reads (8 to 1).
  Stateful families need care and give a bigger blast radius: lease-state refusals (11 to about 3), upgrade plans (5 to 1, the `versions` row is UNIQUE), the variable serve and secret family (13 to about 3), `ansible_runs` (11 to 5), `maintenance_machines` (4 to 2).
  Estimated saving about 50 endpoint tests and as many server spins, at the cost of failure isolation and, for the stateful ones, ordering coupling.
- Much of this overlaps the lens C medium tier (lease matrix, membership and resolution tables, import gates); the two lists should be decided together.
- Suggestions the agents themselves rated weak: moving the database sweep test into the jobs lease test (leaves the database crate with no direct test of `AnsibleLease::sweep`), count-only merges of the pure `hosts.rs`, `limit.rs` and `inventory_file.rs` tests, merging the two environment-picker e2e tests, and folding the catalogue-missing e2e test into the install test.
- Looked redundant but not, so not to be re-proposed: the window refusal table (the only owner of the `run_state` and `take_lease` agreement), `takes_a_lease_while_another_transaction_writes…` (the only owner of the lock behaviour), `names_what_a_run_acts_on…` (wiring across two handlers), the DB lease tests against the endpoint ones (different branches), `forgets_a_value_once_its_week_has_passed` (the only caller of `forget_due`), `managed_by_ansible_migration.rs`, and the one-test web cases that own a branch (null-rank machine, aria-expanded reload, run polling gate).
- Gaps seen, not redundancies: the `Some(_)` branch of `no_longer_held` (released by the same operator) is unasserted, `release_unstarted` has no database or unit test, and one e2e column of the map was inferred from titles.

Process notes from this round:

- Lens D and the C medium tier are the same work seen from two sides (by structure and by overlap); the skill should run them as one pass.
- The useful unit of a D suggestion is "behaviour, owning layer, what each other layer keeps"; the agents that built the behaviour map first produced the best-defended merges.
- Every merge among stateful tests needs its ordering and shared state stated (global `list_open`, UNIQUE rows, the open-lease limit); the claims that failed to verify were all of this kind.

### Round 4: lens D, first round applied

Scope decided: the small folds plus the read-only endpoint families; the stateful families (lease states, upgrade plans, variable serve and secrets, runs lifecycle, `maintenance_machines`) are left for the C medium review.

Applied by four edit agents split by file: 8 database and jobs folds (the window hook tests into the lease wiring test, `says_why_a_lease_was_released`, the two alert tests, three constraint tests, the `maintenance_machine_sets` merges, the enum store tests, the jobs catalogue stub tests); the three read-only endpoint families (import previews 8 to 1, environment-resolution refusals 8 to 1, served-hosts reads 8 to 1, each scenario in its own group with failures collected and named); the `ansible_runs` refusal and preview folds and two variable-test folds; the web folds (the machine-window run-holder e2e test deleted, the empty-group text moved to `groups.spec.ts`, the one-environment picker test and the secret tail folded in).

Evidence:

- Coverage: the baseline (1576 Rust tests) and the result (1536) were taken with `just coverage`, and `lcovdiff.py` compared line coverage across 141 source files in `database`, `jobs` and `private-server`. No production line lost coverage; the 15 lines it flagged were in `jobs/src/ansible/catalogue.rs`'s own test module (after the `#[cfg(test)]` line), which moved with the folded test.
- All 1536 Rust tests pass; Vitest 448; typecheck and `just check` clean; the three e2e spec files (58 tests) pass.
- Mutation check on the one deleted e2e test: making the SPA ignore the window's declarer fails the remaining holder test ("An operator has declared work here" instead of the declarer's name), so the survivor does carry the SPA branch.

Net: 40 fewer Rust tests (1576 to 1536), about 40 fewer migrated databases per run, and one e2e test deleted and two folded.

What round 4 showed about the process:

- A coverage diff is cheap evidence once the recipe exists and answers the "same coverage" question for Rust directly. It needs two rules: compare only production lines, because inline test modules shift line numbers; and take the baseline before any edit starts, since a build in progress can pick up half-edited tests.
- The agents' claims about ordering and shared state held up; the folds that failed to be as simple as proposed were flagged by the agents themselves rather than found by running.
- Reporting the count of tests is a poor headline; "databases migrated per run" tracks cost better, and coverage tracks safety.

### Round 5: C medium tier and the stateful families, settled and applied

Decided together, since they were the same work: fill every untested branch the agents found (as rows in existing tests), fold all six stateful endpoint families, and restructure pure tests only where that adds rows. The `check_claims` and `runs::check` extractions stay declined.

Applied by seven edit agents split by file (database tests, jobs tests, `limit.rs` and `inventory_file.rs`, Vitest schema and `shellArg`, `ansible.rs` lease states and upgrade plans, variables and import and `maintenance_machines`, `ansible_runs` lifecycle). The baseline was the coverage run taken before any edit.

What landed:

- Stateful folds: lease-state refusals 11 to 4 (a table over five lease states checks `for_group` and `extend_lease` for each, covering all four `no_longer_held` arms), upgrade plans 5 to 1, the variable serve and secret family 13 to 3, the `ansible_runs` lifecycle 3 to 1, `maintenance_machines` 4 to 2, hold and amend 2 to 1, run-window cover kinds 4 tests to 1.
- Gap rows added: the same-operator release message, take-over and extend audit entries (with assignments), `release_unstarted`, `import_apply` refusing a preview that cannot apply, `declare_for_run` on a lease that no longer holds, archived and pending machines in a set, a lifted set refusing amendment, restoring a removed secret while held, `removed` listing a twice-removed name once, held-secret and lock round trips over every scope, the whole catalogue compared as JSON with the upgrade intent and preset parsed, placement of every catalogue variable at every scope, catalogue job failures (empty commits, file 404, file 503) and recovery, held secrets whose scope has gone.
- Pure tables that added rows: 58 `limit` rows, 23 inventory refusal rows plus a golden struct test and an `is_host_path` table, the schema kinds and constraint tables and a `ScopeKind` matrix (all `satisfies Record<...>`), `shellArg` metacharacters round-tripped through a real shell.

Evidence: 1509 Rust tests pass (1576 before lens D, 1536 before this round), Vitest 499 (more rows in `it.each` tables), typecheck and `just check` clean, the three e2e spec files (58) pass. The coverage diff against the baseline flags only lines in test modules and the block of `limit.rs` that was edited; no production line is reported lost.

A defect found and fixed: a limit exclusion or intersection with a regular expression (`!~name-.*`) was not refused, because the check looked at the whole token and the `~` sat after the prefix; it silently excluded nothing. The check now looks at the name after the prefix, with rows for exclusion, exclusion after a group and intersection.

Findings still to decide, none of them asserted by a test:

- After a bad publish (`Invalid`), a single `Unreachable` read clears the "does not read" alert until the 24-hour staleness window passes. The new jobs rows pass because of this behaviour.
- `AnsibleLease::release_unstarted` and its handler record `Released`, so `Unstarted` only ever comes from the sweep. Plausibly intended: the operator released it.
- A non-scalar key at the top level of an inventory gives a message with an empty path; nested keys name theirs.
- The `matching` doc comment says "in the order given", but the result is in member order.
- `facility:! central` does not trim after the `!`, so it matches nothing.
- The SPA refuses `managed_by_ansible` at machine scope in the text editor and the server allows it there; by design, because the machine's switch sets it.

Process notes:

- Several agents editing one test directory work if each is told the files it owns and that compile errors elsewhere are transient and not theirs; none touched another's file.
- A rows table run through a pure function turns up behaviour nobody wrote down: the table over `limit` found the `!~` defect only because it stated expected sets exactly instead of counting matches.
- A merged scenario test needs its steps named in its failure messages and its groups kept apart; the agents that did so reported no ordering problems, and none of the merged tests failed.
- A coverage diff stops being the check once a test module is edited, because inline test code moves; compare production lines only and expect noise from edited production blocks.

### Round 5, findings settled

Decided with the user, applied, and covered by tests and one spec line each:

- A catalogue read that cannot reach the repository no longer replaces a verdict that the published copy does not read; the alert stands until a good read (the self-alerts spec already said "clears on the next good read").
- An operator giving up a lease no run has read records the sweep's reason, `Unstarted`, and the audit entry says so; a late read after that still says "has been released", and only a sweep-released lease says "no run read the environment in time".
- A top-level non-scalar key names "the top level", the `matching` comment says the result is in the order of `members`, and the name after an exclusion or intersection mark is trimmed.

Final check of the round: 1509 Rust tests pass, Vitest 499, typecheck and `just check` clean, the three e2e spec files (58) pass. Comparing covered functions by name (not lines, which move when production code is edited) shows no production function lost coverage between the last two runs; against the baseline from before the stateful folds the only functions no longer covered are the 9 test functions that were folded away.

Process note: compare covered functions by name (`FNDA` in the LCOV file) once production code has been edited; a line diff then reports shifted lines as lost.

### First application, not the steady state

This was the first time the process ran on this codebase, and the card's tests sit beside older ones nobody had reviewed, so some of what it found is a one-off bootstrap cost.
Round 2 found about 10 folds in F5's own tests and about 15 in older maintenance, audit and e2e tests beside them; later runs should not expect that bleed.
Consequences for the skill:

- Tag every finding as own (a test the card added) or neighbour (an older test next to it). Apply own by default; report neighbours for their owners unless the user asks for them.
- Judge the stopping rule on own-test findings only.
- Put the per-test lenses (A, B, E) in the guidance read before writing tests, so C and D mostly meet what an incremental build leaves behind and not what a first pass missed.
- Keep this run's numbers as the reference: 190 tests added, about 40 folded in round 1 of lens D, about 10 more of F5's own in round 2, one coverage baseline and a function-level diff to check each apply.

### Round 6: lens D, second round applied

Three fresh agents read the current tests and reported 10 folds in F5's own tests and about 15 in older neighbours (maintenance moves and amend, audit, `maintenance.spec.ts`, the pre-F5 keyset snapshot tests); the user chose to apply F5's own plus the neighbours, then run a third round over F5's own tests only.

Applied by five edit agents split by file: database F5 files, `maintenance_window_moves.rs` and the database `audit.rs`, jobs, private-server tests, `maintenance.spec.ts` with two Vitest nits.
Two suggestions were left on purpose: the pre-F5 keyset snapshot tests (the reviewer judged them not F5's), and `a_moved_window_is_in_both_targets_histories` with `history_lists_what_still_covers_the_target_first` (a `limit 1` history ordering that could not be confirmed).
Where a fold dropped coverage at one layer it was shown to be made at another: the endpoint-level 400 and 409 for a refused move are now covered by the database test and by the endpoint tests of the two mappings, and the window identity check in the refused take is now the operator name in the endpoint table.

Evidence: 1485 Rust tests pass (1509 before), Vitest 499, typecheck and `just check` clean, the three e2e spec files pass (53, from 58). Comparing covered functions by name against the previous run, the functions no longer covered are all test functions or closures in test modules, plus one instance of the generic `Actor::canopy` whose body lines are still covered (21 hits against 26).

### Round 7: lens D, third round (F5's own tests only)

Two fresh agents (Rust, web) found nothing worth acting on and listed what they had checked and why it stands, so lens D's stopping rule is met for the card's own tests.
One aside became a test: nothing swept an expired lease that has a bound window, so `AnsibleLease::sweep`'s call to `end_for_lease` was untested; the sweep test now declares a window first and checks it ends once, and fails if the call is removed.
Left standing by the reviewers and not acted on: two `inventory_file.rs` comment tests that overlap an `is_host_path` table (a saving of two microsecond tests), the six group-precedence tests (six rules, six YAML bodies), and three Playwright merges priced only by judgment, because there are no per-step timings of the worker-scoped stack.

Totals over the whole review: Rust test functions 2412 to 2304 (integration-style 1915 to 1800, unit 497 to 504); the verified run is 1485 tests, all passing, from 1576 before lens D; Vitest 499 (many rows are `it.each` cases); e2e 53, from the 58 after lens C.

## Signal to noise

For each lens, once run: how many findings, how many accepted, how many rejected and why, and a verdict on whether the lens belongs in the skill, as a separate pass, or as guidance only.

| Lens | Findings | Accepted | Rejected | Verdict |
|---|---|---|---|---|
| A. Trivial or over-built | Rust 25, Vitest 3, Playwright 5. Only 2 were truly trivial; the rest were folds into a neighbour that shared the setup | 24 of 25 Rust, all Vitest, 3 of 5 Playwright | 1 (a spec made the assertion necessary), 2 optional merges the reviewer advised against | Guidance at write time ("what bug would fail this that no other test would catch"); the folds it finds are lens D's. Low yield as a separate pass |
| B. Tests the dependency | 3 (jsonschema twice, yaml-rust2 once), none on the web side | 2 of 3 | 1 (the spec required the assertion) | A checklist line in the write-time guidance and in the review prompt. Rare, high confidence per hit, only for validation and parsing libraries; not worth its own pass |
| C. Incremental sprawl | About 40 proposals; roughly 12 high, 14 medium, the rest low or leave alone | All 7 high-tier items, the medium tier and the stateful families as decided | 1 declined by decision (extractions that remove no duplication) | Keep as its own pass. Highest durable value: drift protection (variant lists, shared fixtures, exhaustive destructuring), new coverage rows, and it surfaced a real defect (`!~regex` in a limit). Needs the code under test and often a small shared helper |
| D. Open-ended consolidation | Round 1 about 40 tests; round 2 about 10 of the card's own plus about 15 in older neighbours; round 3 none | Rounds 1 and 2 applied (the weakest 3 of round 1 skipped) | The neighbour slack could have been left; a few folds judged not worth their isolation cost | Keep, as a loop that stops when a round finds nothing in the card's own tests. Biggest count reduction; the check is a coverage diff by function name. Overlaps C's medium tier, so run them as one |
| E. Cannot fail | 5 (a count taken before the page loaded, a negative check with no positive control, a count that cannot tell the thing from its opposite, a race on the first poll, and one Vitest input that could not match a rule) | All 5 | none | Keep as a lens in the review and as a rule: prove a repaired test by mutating the code under test and watching it fail. Found only by reading assertions |

Defects found while applying changes, none of them by a lens as such: a test added the same day that had never passed (it aged audit entries in a way the guard forbids); a late read of an unread lease called "expired"; a limit exclusion with a regular expression silently matching nothing; an unreachable catalogue read silencing an invalid-catalogue alert; the unread give-up recording a different reason from the sweep. The common cause was running the tests while applying, and writing each new expectation from the code and not from a guess; both should be rules in the skill.

## Skill shape (settled)

Decided with the user at the end of the walkthrough:

- Two skills, with Workhorse metadata (`name`, `description`, `label`, `pill-order` by card state, `jockey-hint`, `workhorse-version`): `write-good-tests` (write-time guidance, pill under implementing) and `review-tests` (the review pass and the apply-and-verify loop, pill under reviewing and complete).
- One reference doc, `.agents/docs/test-quality.md`, cited by both: the write-time rules, the five lenses with their yield, own versus neighbour findings, the evidence rules, the loop and stopping rule, and condensed review and edit brief templates.
- The coverage check is committed: `scripts/coverage-fn-diff.sh` (bash and awk, like the other scripts) behind `just coverage-diff BASE NEW`, next to `just coverage`.
- Not done, by choice: no line added to AGENTS.md pointing at the doc (the user chose two skills over a doc plus a pointer).

## Open questions

- [x] Add lens E (a test that cannot fail) to the lenses and to the skill's checklist.
- [x] Skill names and shape: `write-good-tests` and `review-tests`, one reference doc.
- [ ] Check that the Workhorse smart-merge leaves the two non-shipped skills and the doc alone when it next updates the shipped ones.
- [x] Lens C's helper: `all_variants!` in `commons-types`; the shared JSON fixtures live beside the SPA tests.

## Decisions

- Coverage: none is wired up today, and some should be added, for Rust and the web side.
  Coverage numbers do not tell the whole story (a line can be executed without anything checking it), so a finding still carries its own argument for what is preserved, and coverage is evidence for that argument, not a replacement for it.
- Lens D reads the tests themselves to start: the card's added tests and the existing tests alongside them in the same files and area.
  It does not read whole crates.
  Widen only when a specific finding needs a test elsewhere to justify a deletion.
- The skill's final shape (one skill or several, pass and loop) is decided at the end, after the lenses have been scored.
