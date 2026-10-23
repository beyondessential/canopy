---
status: draft
---

# Test review process (F5 as test bed)

F5 and its stack of PRs added 189 tests.
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
| #672 | open | `af82ff3c^..af82ff3c` | 1 |

By level: 40 Rust unit (inline in `src/`), 89 Rust integration (`tests/it/`), 37 Vitest, 23 Playwright.

By file (added counts):

- `crates/database/tests/it`: `ansible_catalogue.rs` 12, `ansible_leases.rs` 5, `ansible_variables.rs` 1, `maintenance_machine_sets.rs` 12, `maintenance_run_windows.rs` 13, `managed_by_ansible_migration.rs` 1
- `crates/private-server/tests/it`: `ansible.rs` 5, `ansible_import.rs` 14, `ansible_runs.rs` 15, `ansible_variables.rs` 7, `maintenance_machines.rs` 4
- `crates/private-server/src/fns/ansible` (inline unit): `inventory_file.rs` 17, `hosts.rs` 7, `limit.rs` 7
- `crates/jobs/src` (inline unit): `ansible/catalogue.rs` 4, `ansible/leases.rs` 2, `ansible/held.rs` 1, `backup/recovery_snapshot.rs` 2
- `private-web/src/components`: `ansible/yamlScope.test.ts` 16, `ansible/schema.test.ts` 11, `ansible/pyyaml.test.ts` 4, `AnsibleRunDialog.test.ts` 5
- `private-web/e2e`: `ansible.spec.ts` 22 (9 moved from the deleted `inventory.spec.ts`), `maintenance.spec.ts` 1

Renames in #666 are not new tests: five Rust tests changed their inventory vocabulary, and nine e2e tests moved from `inventory.spec.ts` to `ansible.spec.ts` under the same names.
Two e2e tests from `inventory.spec.ts` were dropped.

Test names are not copied here; regenerate them from the diff ranges above when running a lens.

## Findings

Filled in as the walkthrough runs. One subsection per round, per lens.

## Signal to noise

For each lens, once run: how many findings, how many accepted, how many rejected and why, and a verdict on whether the lens belongs in the skill, as a separate pass, or as guidance only.

| Lens | Findings | Accepted | Rejected | Verdict |
|---|---|---|---|---|
| A. Trivial or over-built | | | | |
| B. Tests the dependency | | | | |
| C. Incremental sprawl | | | | |
| D. Open-ended consolidation | | | | |

## Skill shape (to settle from the walkthrough)

The user wants a repo-specific skill that carries the whole process: guidance for writing tests, a review pass, and a review loop focused on making tests better, in the spirit of the existing review skills.
Candidate parts:

- Guidance read before writing tests (what is worth a test, what is the dependency's job, prefer a drift-proof test over a pile).
- A single review pass that applies lenses A to D to a card's added tests and reports keep, delete, merge and rewrite findings.
- A loop that applies accepted findings and reruns until a round is quiet.

## Open questions

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
