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
Not yet applied; waiting on the user's call.

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
- Applying changes finds defects reading does not: the unrun test above only showed itself when its test was run.

## Signal to noise

For each lens, once run: how many findings, how many accepted, how many rejected and why, and a verdict on whether the lens belongs in the skill, as a separate pass, or as guidance only.

| Lens | Findings | Accepted | Rejected | Verdict |
|---|---|---|---|---|
| A. Trivial or over-built | Rust 25 (4 delete, 6 rewrite, 15 merge); Vitest 3; Playwright 5 | Rust 24 of 25, 17 fewer tests net | 1 (spec made the assertion necessary) | Provisional: finds few truly trivial tests, mostly folds that blur into D |
| B. Tests the dependency | Rust 3 (jsonschema, yaml-rust2); Vitest 0; Playwright 0 | 2 of 3 | 1 (spec) | Provisional: rare, only validation and parsing libraries, worth keeping as a short checklist |
| E. Cannot fail | 4 in Playwright, 1 in Vitest | not yet | | New lens from round 1; found by reading assertions, not counting tests |
| C. Incremental sprawl | | | | |
| D. Open-ended consolidation | | | | |

## Skill shape (to settle from the walkthrough)

The user wants a repo-specific skill that carries the whole process: guidance for writing tests, a review pass, and a review loop focused on making tests better, in the spirit of the existing review skills.
Candidate parts:

- Guidance read before writing tests (what is worth a test, what is the dependency's job, prefer a drift-proof test over a pile).
- A single review pass that applies lenses A to D to a card's added tests and reports keep, delete, merge and rewrite findings.
- A loop that applies accepted findings and reruns until a round is quiet.

## Open questions

- [ ] Add lens E (a test that cannot fail) to the lenses and to the skill's checklist.
- [ ] Skill name and whether it is one skill (guidance plus pass plus loop) or a pass and a loop kept apart. Decided to settle at the end, from the walkthrough.
- [ ] Where it lives: workspace skills go in `.agents/skills/<name>/SKILL.md`; check the Workhorse smart-merge leaves a non-shipped skill alone.
- [ ] Whether lens C's drift-proof tests need shared helpers per crate (a variant-list macro, say) that do not exist yet.

## Decisions

- Coverage: none is wired up today, and some should be added, for Rust and the web side.
  Coverage numbers do not tell the whole story (a line can be executed without anything checking it), so a finding still carries its own argument for what is preserved, and coverage is evidence for that argument, not a replacement for it.
- Lens D reads the tests themselves to start: the card's added tests and the existing tests alongside them in the same files and area.
  It does not read whole crates.
  Widen only when a specific finding needs a test elsewhere to justify a deletion.
- The skill's final shape (one skill or several, pass and loop) is decided at the end, after the lenses have been scored.
