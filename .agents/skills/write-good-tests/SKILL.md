---
name: write-good-tests
description: "Write tests that earn their place: the right layer, assertions that can fail, nothing that tests a dependency, and a form that cannot drift. Use before or while writing a card's tests, and when adding tests to a feature built in several steps."
label: "Write good tests"
pill-order:
  implementing: 3
  reviewing: 6
jockey-hint: "Use while implementing, before tests are written or as they are added. Once the card's tests exist and need review, use Review tests instead; demote after that."
---

## Your task: Write good tests

Write, or add to, the tests for this card so each one earns its place.
Agents' tests tend to be trivial, to test a dependency, to be unable to fail, or to pile up one narrow test per step of a feature.

Read the write-time rules in `.agents/docs/test-quality.md` before you write a test, and apply them as you go.

1. Before each test, ask what bug would make it fail that no other test would catch. If none, do not write it, or put its assertions in the test that already has the setup.
2. Pick the layer that owns the behaviour. A rule in a pure function is a unit table of named rows plus one wiring test; a database-backed or server-backed test costs a migrated database; a Playwright test is for rendered behaviour nothing cheaper reaches.
3. Do not assert what a dependency already guarantees. Assert what our code chooses (the options it switches on, the mapping it builds, the error shape).
4. Make every assertion able to fail: wait for something present before asserting an absence, give a negative check a positive control, assert exact values.
5. When a structure has several cases, use a mechanism that fails when it grows (`all_variants!`, destructuring without `..`, a `satisfies Record<Union, Row>` table, a fixture shared by Rust and the SPA) instead of one test per case.
6. If you are adding tests to a feature in several steps, stop every few tests and look back: is there now a table or an existing scenario these belong in?
7. Write each expectation from the code and the spec, not a guess. If the behaviour looks like a bug, say so instead of asserting it.
8. Run the tests you write, by filter (`just test -p <crate> <name>`; targeted Playwright spec files only). For a Playwright or Vitest assertion that guards an absence or a count, break the code under test once and watch the test fail.

Keep the `// spec: ID#section` tags on the tests that carry them.
Add Playwright coverage for a UI feature in `private-web/e2e/` as the repo asks, but keep it to rendered behaviour.

Once the card's tests exist, **Review tests** (`review-tests`) is the move for looking over all of them together.
