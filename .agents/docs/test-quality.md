# Test quality

How to write tests that earn their place, and how to review a card's tests so there are fewer of them and each one is worth keeping.
It is read by the **Write good tests** skill (`write-good-tests`) before and while tests are written, and by the **Review tests** skill (`review-tests`) when a card's tests are reviewed.

## What goes wrong

Tests written by agents, and tests added feature by feature, drift the same ways:

- A test that asserts something the type system, a constructor or another test already guarantees.
- A test that asserts what a dependency already guarantees (a validation or parsing library's own behaviour).
- An assertion that cannot fail: true whatever the code does.
- Many narrow tests, each added with the slice of the feature that needed it, where one test walking the whole structure would cover the same ground and could not drift.
- A migrated database per small property, when the behaviour could be owned by a pure function and checked in a table.

## Write-time rules

Apply these before and while writing a test.

1. **Ask what bug would make this test fail that no other test would catch.** If the answer is none, do not write it, or fold it into the test that already has the setup.
2. **Test our code's choices, not the dependency's.** With a validation or parsing library, assert what we configure and map (the options we switch on, the paths we strip, the error shape we build), not that the library validates or parses. If removing our code from the path would leave the test passing, it tests the library.
3. **Make every assertion able to fail.**
   - Before asserting that something is absent, wait for a signal that the page or state has loaded (assert something present first).
   - Give a negative check a positive control in the same step (the filter hides X, and Y is shown).
   - Assert exact equality, not "contains" or `length > 0`, where the exact value is known.
   - Where a count is asserted, check it can tell the thing from its opposite (a count that would also hold if everything were removed and re-added).
   - Prove it: break the code under test on purpose and watch the test fail (see Evidence).
4. **Own a behaviour at one layer.**
   - Rules enforced by a pure function belong in a unit table of named rows, with one endpoint test for the wiring.
   - A database-backed or server-backed test costs a migrated database, so do not spend one on a property a cheaper test owns.
   - A browser test earns its place only for rendered behaviour nothing cheaper reaches; it costs its seed, table reset and page load.
5. **Prefer a table or an existing scenario to a new test** where the setup is shared, but do not stack unrelated flows into one long test, and name the step in every assertion message so a failure says which step.
6. **Choose a mechanism that fails when the structure grows.**
   - A fieldless enum a test walks: `all_variants!` (in `commons-types`) gives it `ALL` and an exhaustive-match guard, so a forgotten variant is a compile error.
   - A struct a test reads field by field: destructure it without `..`, so a new field is a compile error.
   - A TypeScript union: a `satisfies Record<Union, Row>` table, so a new member is a type error until it has a row.
   - A rule implemented twice, in Rust and in the SPA: one JSON fixture both read.
   - A string-coded enum behind a SQL CHECK: store every variant through the real API and read it back.
7. **Write each expectation from the code and the spec, not from a guess.** If the code's behaviour looks like a bug, do not assert it as right: report it, and fix a plain bug with a test that fails without the fix.
8. **Keep the `// spec: ID#section` tag with the test that survives.** Moving or folding a test moves its tags.
9. **Run what you write.** An unrun test can turn out never to have passed.

## The lenses

A review pass applies these to a card's own tests.
Each finding is tagged **own** (a test the card added) or **neighbour** (an older test beside it).

- **A. Trivial or over-built.**
  A test whose assertions the type system, a constructor, a derive or another test makes; setup that dwarfs the assertion; a property a cheaper test already exercises.
  Few tests are truly trivial; most findings are a fold into a neighbour that shares the setup, which is lens D's work.
- **B. Tests the dependency.**
  The assertion's truth comes from a library.
  Signal: calling the library directly, with our code out of the path, would still pass.
  Keep a test only where our code makes a choice about the library, and assert that choice.
  Hits are rare and confined to validation and parsing libraries.
  Check a hit against the spec first: an assertion may be there because the spec requires it.
- **C. Incremental sprawl.**
  Group tests by the structure they exercise (an enum, a struct, a rule table, a family of refusals); where each test touches one case, propose one test that walks the whole structure, using a mechanism from write-time rule 6.
  This needs the code under test, because the drift-proof form depends on whether the structure is a value, a type or a string.
  It has the most durable value (drift protection, new coverage rows, and it can surface real defects), but it often needs a small shared helper and sometimes a small production extraction where a rule is duplicated.
  Decline an extraction that removes no duplication.
- **D. Open-ended consolidation.**
  Read the card's tests next to the existing tests for the same area (tests only, not whole crates) and reduce their number or cost without reducing coverage: merge, delete a test whose every assertion another makes, or move a behaviour to the one layer that owns it with a thin wiring check at the next.
  Every suggestion names the tests, the behaviour or function and branch reached, which test survives and what it keeps, and a coverage claim that can be checked.
  It gives the biggest reduction in count, with returns that shrink each round.
  Run lens C and lens D as one pass: they are the same work seen by structure and by overlap.
- **E. Cannot fail.**
  An assertion true whatever the code does: an absence checked before the page loaded, a negative check with no positive control, a count that cannot tell the thing from its opposite, a race that usually goes one way.
  It is found by reading assertions, not by counting tests.
  A repair is proved by mutation.

Lens D stops when a round finds nothing worth acting on in the card's own tests.
A suggestion that saves a test but costs failure isolation, stacks unrelated flows, or makes a table harder to read than the tests it replaces is not worth acting on unless the reviewer says why it still is.

## Own and neighbour findings

- Apply own findings by default.
- Report neighbour findings (older tests that the card did not write) to the user and apply them only if asked; they may belong to someone else's card, and applying them widens the diff beyond the card.
- Judge the stopping rule on own findings only.
- On the first run over a codebase, expect neighbour slack that later runs will not find.

## Evidence

- **Baseline first.** Run `just coverage -p <crates>` before any edit and copy `target/coverage/lcov.info` aside (`cp target/coverage/lcov.info /tmp/base.info`). Do not edit tests while it builds: an instrumented build in progress can pick up half-edited tests.
- **After applying**, run `just coverage` again and compare with `just coverage-diff /tmp/base.info target/coverage/lcov.info`. It lists production functions the baseline reached and the new run does not, by name, so edits that move lines do not matter. A generic function can be listed because one type instance moved; check its body lines still run. Inline test modules shift line numbers, so never compare lines.
- **Web tests** have no coverage tool here. Prove a browser or Vitest change by mutation: say which line of the code under test to break and which test must then fail, break it, watch it fail, restore the file.
- **Run the affected tests by filter** (`just test -p <crate> <name>`; targeted Playwright spec files only, with `env -u PORT` under a Workhorse local agent; never the whole Playwright suite locally). Finish with the full suites for the touched crates under `just coverage`, `just test-web`, `just typecheck`, `just check`, and the touched Playwright spec files.
- **Fold means every assertion survives**, or is shown to be made by a named test elsewhere. A merged scenario with shared state needs its steps ordered so none masks another (a bad insert that should fail a CHECK must not fail on a foreign key instead), its groups kept apart, and global state respected (a query that lists every open row, a UNIQUE constraint, a rule of one open row per owner).

## The loop

1. **Inventory** the card's tests: per pull request, per file, per layer. A pull request too large for GitHub's diff limit is diffed from its merge commit against its first parent. Re-derive the inventory against the current tip each round.
2. **Baseline** coverage (above).
3. **Review**, read-only, one agent per area (database, endpoint, unit and job, web), each given the lens definitions and the brief below.
4. **Decide** with the user anything that is a design choice: a variant-list mechanism, a production extraction, whether neighbours are in scope, how far to consolidate stateful endpoint tests.
5. **Apply**, one edit agent per set of files it owns, compile only, no formatting, no tests, no commits.
6. **Verify**: format, run by filter then the full suites, diff coverage, run mutation checks, update any record the card keeps.
7. **Repeat** with fresh agents until a round finds nothing worth acting on in the card's own tests.

Defects found while applying changes, not by any lens, come from running the tests and writing expectations from the code. Report a suspected production bug to the user and, where it is plain, fix it with a test that fails without the fix.

## Brief templates

Fan out where subagents exist; otherwise do the passes yourself in the same order.

### Review brief (read-only)

> You are doing a READ-ONLY test review. Do not edit, create or delete any file, and do not run tests or builds. Repo: `<path>`. Read AGENTS.md first.
>
> Context: `<the card, how many tests, what earlier rounds already did, what was deliberately left standing>`. Read the CURRENT files, not diffs. Do not re-propose what is done.
>
> Lens `<definition from this doc>`. Scope: tests the card added in `<files>`; read neighbours only to check overlap, and tag every finding own or neighbour.
>
> Every suggestion names the tests, the function and branch reached, which test survives and what it keeps, and a coverage claim that can be checked. Say where you could not verify something. If you find nothing worth acting on, say so plainly and list what you checked and why it stands; do not manufacture suggestions.
>
> Output, compact: the verdict; each suggestion (tests, survivor, coverage claim and how to check it, saving, confidence); what you checked and found standing; what made judging hard.

### Edit brief

> You are applying agreed test changes in `<path>`. Read AGENTS.md first.
>
> Rules: other agents are editing other files at the same time. You own ONLY `<files>`. A compile error in a file you do not own is probably another agent's half-finished edit: wait and re-run, never touch it. Do NOT run tests (the RAM-backed Postgres uses a fixed port and would clash) except plain unit tests that need no database; compile only (`SKIP_FRONTEND_BUILD=1 cargo check -p <crate> --tests`). No formatting, no commit, no production changes unless an item says so.
>
> Fold means every assertion survives, or you name the test that still makes it. `// spec:` tags move to the survivor. Name each step in assertion messages. Order merged steps so none masks another; keep groups apart. Write each new expectation from the code under test, not a guess; if behaviour looks like a bug, report it and do not assert it as right. If an item turns out wrong, leave the tests as they were and say why. Confirm each item's claim against the current files before acting.
>
> Items: `<each item with its tests, survivor and what it keeps>`.
>
> Report, compact: per item what you did, assertions dropped (aim for none), anything left and why, suspected production bugs, compile result.
