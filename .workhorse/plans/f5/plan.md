# F5: test review process and skill

Establish a repeatable process for making a card's tests better, using F5's 190 added tests as the test bed, then capture it as a repo skill.
Lenses, inventory and the running findings live in `.workhorse/working-docs/f5/working-doc.md`.

## Steps

- [x] Inventory the tests F5 added across #666, #670, #671, #672
- [x] Record the lenses, process and inventory in the working doc
- [x] Settle the open questions that block the walkthrough: coverage is wanted but absent, lens D reads the tests and their neighbours only
- [x] Rust coverage tooling: `just coverage` (cargo-llvm-cov, contained RAM Postgres)
- [ ] Vitest coverage (no provider installed; needs a dev dependency, decide whether to add)
- [x] Lens A over the Rust tests: trivial or over-built (findings in the working doc, not yet applied)
- [x] Lens A over the Vitest and Playwright tests (findings in the working doc, not yet applied)
- [x] Lens B over the Rust tests: dependency behaviour (3 hits: jsonschema, yaml-rust2)
- [x] Lens B over the Vitest and Playwright tests (no hits)
- [x] Lens C analysis: incremental sprawl (findings in the working doc, not yet applied)
- [x] Settle lens C's design decisions and apply its high tier, running the affected tests (all passing)
- [x] Lens C medium tier and the stateful endpoint families, settled with the user and applied (all passing, no production line lost)
- [x] Settle the findings from that round (alert behaviour, release_unstarted reason, minor messages) and apply them
- [x] Lens D round 1: analysis and apply of the small folds and read-only families (coverage diff shows no production line lost)
- [x] Lens D round 2: fresh agents, applied to F5's own tests and the neighbours (all passing, no production function lost)
- [ ] Lens D round 3: fresh agents over F5's own tests only; stop if nothing is worth acting on
- [x] Apply the Rust findings of lenses A and B and run the affected tests (all passing)
- [x] Apply the Vitest and Playwright findings, including the vacuous assertions, and run the affected tests
- [ ] Walk the remaining lenses' findings with the user, apply the accepted ones, run affected tests by filter
- [ ] Fill in the signal-to-noise table and decide which lenses the skill keeps
- [ ] Write the skill at `.agents/skills/<name>/SKILL.md`: guidance, a review pass, a review loop
- [ ] Trial the skill on a later card, adjust, then unplan

## Notes

- This plan is about the process and the skill. Verification of the test changes themselves is just running the affected tests.
- Test changes land as ordinary F5 commits; only the process material (working doc, plan, skill) is new artefacts.
