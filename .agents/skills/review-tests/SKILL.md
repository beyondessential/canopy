---
name: review-tests
description: "Review the tests this card added for trivial or over-built tests, tests of a dependency's behaviour, assertions that cannot fail, incremental sprawl and overlap, then apply the agreed changes and verify no coverage was lost. Use once a card's tests exist, especially after a feature built in several steps. Not for writing new tests (see write-good-tests)."
label: "Review tests"
pill-order:
  reviewing: 3
  complete: 2
  implementing: 6
jockey-hint: "Use when the card's implementation and tests are in place and before the card is finished. Demote once the review has run to its stopping point; skip for a card with only a handful of tests."
workhorse-version: 0.3.0
---

## Your task: Review tests

If you don't already have this card's context (title, identifier, description), establish it first by following `.agents/docs/card-context.md`.

Review the tests this card added, make them fewer and better without losing coverage, and prove it.
The method, the lenses and the brief templates are in `.agents/docs/test-quality.md`: read it first. This skill is the order of work.

1. **Inventory the card's tests**: every test added across the card's commits or pull requests, by file and layer (Rust unit, Rust integration, Vitest, Playwright). Count a test as the card's only if its function or `test(...)` line is new; renamed or moved tests are not new. For a pull request too large for GitHub's diff limit, diff its merge commit against its first parent.
2. **Take a coverage baseline before editing any test**: `just coverage -p <crates the card touched>`, then copy `target/coverage/lcov.info` aside. Do not edit tests while it builds.
3. **Review, read-only, in parallel by area** (database, endpoint, unit and job, web) with the review brief and the lenses (A trivial or over-built, B tests the dependency, C incremental sprawl, D consolidation, E cannot fail). Tag every finding **own** or **neighbour**. If you cannot fan out, do the areas in turn.
4. **Bring the user the findings and the decisions that are theirs**: which variant-list or fixture mechanism, any production extraction (only where it removes duplication), whether neighbours are in scope (default: report them, do not apply), and how far to consolidate stateful endpoint tests. Do not apply anything that needs a decision before you have it.
5. **Apply**, one edit agent per set of files it owns, with the edit brief. They compile only, run no integration tests, and do not format or commit.
6. **Verify**: format, run the affected tests by filter, then the full suites for the touched crates under `just coverage`; compare with `just coverage-diff <base> target/coverage/lcov.info`; run `just test-web`, `just typecheck`, `just check`, and the touched Playwright spec files. For each repaired or deleted web test, mutate the code under test and watch the surviving test fail. Fix what the agents got wrong; a fold that fails to hold is reverted and reported, not forced.
7. **Repeat from step 3 with fresh agents** until a round finds nothing worth acting on in the card's own tests. Re-derive the inventory first.

Report suspected production bugs to the user as you meet them; fix a plain one with a test that fails without the fix. Do not change production code otherwise.

When you finish, say concisely what changed (tests before and after, by cost class where it helps), what was verified and how, the defects found, and anything left on purpose with the reason. Keep a short running record of rounds and decisions in the card's working doc or plan if it has one.

Applying the lenses to the whole codebase is out of scope; read neighbouring tests only to check overlap with the card's own.
