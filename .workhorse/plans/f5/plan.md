# F5: test review process and skill

Establish a repeatable process for making a card's tests better, using F5's 189 added tests as the test bed, then capture it as a repo skill.
Lenses, inventory and the running findings live in `.workhorse/working-docs/f5/working-doc.md`.

## Steps

- [x] Inventory the tests F5 added across #666, #670, #671, #672
- [x] Record the lenses, process and inventory in the working doc
- [ ] Settle the open questions in the working doc that block the walkthrough (coverage check, how wide lens D reads)
- [ ] Lens A over the inventory: trivial or over-built tests
- [ ] Lens B: tests of dependency behaviour, noting which dependency kinds produce hits
- [ ] Lens C: incremental sprawl, with drift-proof replacements where the type system or a macro can force full coverage
- [ ] Lens D: open-ended consolidation against the existing tests around each area, two rounds
- [ ] Walk each lens's findings with the user, apply the accepted ones, run affected tests by filter
- [ ] Fill in the signal-to-noise table and decide which lenses the skill keeps
- [ ] Write the skill at `.agents/skills/<name>/SKILL.md`: guidance, a review pass, a review loop
- [ ] Trial the skill on a later card, adjust, then unplan

## Notes

- This plan is about the process and the skill. Verification of the test changes themselves is just running the affected tests.
- Test changes land as ordinary F5 commits; only the process material (working doc, plan, skill) is new artefacts.
